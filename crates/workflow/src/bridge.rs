//! The `agent()` promise bridge and the depth-bounded JSON boundary.
//!
//! ## Why a thread-local, not a captured `Rc<RefCell<…>>`
//!
//! `SpawnState` stores `ResolvingFunctions` (which hold `JsFunction` GC
//! pointers). The obvious design — capture a shared handle into the native
//! closure via `NativeFunction::from_copy_closure_with_captures` — requires the
//! capture to implement `boa_gc::Trace`, and the only safe way to get that impl
//! is `#[derive(Trace)]`, whose expansion hardcodes `::boa_gc` and therefore
//! needs `boa_gc` as a *direct* dependency (which the crate deliberately does
//! not take) — and `unsafe impl Trace` is barred by `#![forbid(unsafe_code)]`.
//!
//! So we do what the W3 spike did: keep `SpawnState` in plain Rust memory and
//! reach it from the capture-free native closure through a thread-local that is
//! set only for the duration of one synchronous `drive()`. A `JsFunction` held
//! in live Rust memory is *rooted* (boa_gc root-counts handles that live outside
//! the GC heap), so the resolvers cannot be collected while we hold them — no
//! tracing is required. The thread-local is a transient pointer, not ownership:
//! each `WorkflowRun` still owns its own `SpawnState`, and because a `drive()`
//! never nests, concurrent runs never observe each other's state.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use boa_engine::builtins::promise::ResolvingFunctions;
use boa_engine::object::builtins::{JsArray, JsPromise};
use boa_engine::object::JsObject;
use boa_engine::property::PropertyKey;
use boa_engine::{
    js_string, Context, JsError, JsNativeError, JsResult, JsString, JsValue, JsVariant,
    NativeFunction,
};

use crate::error::WorkflowError;

/// Hard ceiling on JSON nesting crossing the JS boundary in either direction.
/// A2: adversarial step results (untrusted model output) and script-produced
/// values are *refused* past this depth, never allowed to overflow the stack.
/// The recursion below is bounded by this constant, so it is stack-safe.
pub(crate) const MAX_JSON_DEPTH: usize = 128;

/// One brokered child-run request. Plain Rust — no GC pointers — so it can live
/// in ordinary heap memory alongside the resolver map.
#[derive(Debug, Clone)]
pub(crate) struct PendingSpawn {
    pub(crate) id: u64,
    pub(crate) role: String,
    pub(crate) prompt: String,
    pub(crate) opts: serde_json::Value,
}

/// Per-run bridge state. Owned by one `WorkflowRun`; shared with the native
/// `agent()` only transiently, via the thread-local below.
pub(crate) struct SpawnState {
    /// Monotonic request id; correlates a `SpawnRequest` with its `StepResult`.
    pub(crate) next_id: u64,
    /// Roles the script declared in `meta.roles`, for the consistency check.
    pub(crate) roles: Vec<String>,
    /// Requests issued since the last drain, awaiting fan-out by the CLI.
    pub(crate) pending: Vec<PendingSpawn>,
    /// Resolver handles for promises the CLI has not yet answered. Held in Rust
    /// memory, so the `JsFunction`s inside stay GC-rooted.
    pub(crate) awaiting: HashMap<u64, ResolvingFunctions>,
    /// Set when `agent()` named an undeclared role, so the drive loop can
    /// report `UndeclaredRole` after the resulting rejection.
    pub(crate) undeclared: Option<String>,
}

impl SpawnState {
    pub(crate) fn new(roles: Vec<String>) -> Self {
        Self {
            next_id: 0,
            roles,
            pending: Vec::new(),
            awaiting: HashMap::new(),
            undeclared: None,
        }
    }

    /// Remove and return all pending requests (the next batch).
    pub(crate) fn take_pending(&mut self) -> Vec<PendingSpawn> {
        std::mem::take(&mut self.pending)
    }
}

thread_local! {
    /// A stack of the states whose `drive()` is currently on this thread's call
    /// stack. `agent()` reads the top; `ActiveGuard` keeps it in sync. A stack
    /// (rather than a single slot) is defensive — a `drive()` should never nest,
    /// but if it ever did, each frame still sees its own state.
    static ACTIVE_STATE: RefCell<Vec<Rc<RefCell<SpawnState>>>> = const { RefCell::new(Vec::new()) };
}

/// RAII guard: makes `state` the active bridge state for as long as it is held,
/// and pops it on drop — including during a panic unwind.
pub(crate) struct ActiveGuard;

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        ACTIVE_STATE.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

/// Push `state` as the active bridge state; the returned guard pops it.
pub(crate) fn activate(state: &Rc<RefCell<SpawnState>>) -> ActiveGuard {
    ACTIVE_STATE.with(|stack| stack.borrow_mut().push(Rc::clone(state)));
    ActiveGuard
}

fn current_state() -> Option<Rc<RefCell<SpawnState>>> {
    ACTIVE_STATE.with(|stack| stack.borrow().last().map(Rc::clone))
}

/// Install the global `agent(prompt, opts)` function. The closure captures
/// nothing (so it is `Copy`, as `from_copy_closure` requires); it reaches the
/// active `SpawnState` through the thread-local.
///
/// AL1: the v1 signature is `agent(prompt, opts)` with the role read from
/// `opts.role` — the ONLY documented change from Claude Code is `opts.model` →
/// `opts.role` (design doc §3), so a Claude Code script imports with a
/// mechanical edit. The full `opts` object rides along in the spawn request;
/// `role` is *also* extracted from it for the consistency check and fan-out.
pub(crate) fn install_agent(context: &mut Context) -> JsResult<()> {
    let agent = NativeFunction::from_copy_closure(|_this, args, context| {
        let state = current_state().ok_or_else(|| {
            JsError::from(
                JsNativeError::typ().with_message("agent() called outside a workflow drive"),
            )
        })?;

        // First arg: the required prompt string.
        let prompt = string_arg(args, 0).ok_or_else(|| {
            JsError::from(
                JsNativeError::typ().with_message("agent(prompt, opts): prompt must be a string"),
            )
        })?;

        // Second arg: the optional opts object, converted depth-bounded (A2) so
        // the whole object can ride in the spawn request. A missing or non-object
        // arg leaves `opts` as JSON null, which fails the `role` check below.
        let opts = match args.get(1) {
            Some(v) if !v.is_null_or_undefined() => js_to_value(v, context, 0).map_err(|_| {
                JsError::from(
                    JsNativeError::typ().with_message("agent() opts is nested too deeply"),
                )
            })?,
            _ => serde_json::Value::Null,
        };

        // `opts.role` is required and must be a string. A missing or non-string
        // value is a clear TypeError naming `opts.role`. `Value::get` returns
        // `None` for a non-object `opts`, so that path also lands here.
        let role = opts
            .get("role")
            .and_then(|r| r.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ()
                        .with_message("agent(prompt, opts): opts.role must be a string"),
                )
            })?;

        // Script-internal consistency (R2): the call must name a role the
        // script's own meta.roles declared. This is NOT an authority gate —
        // manifest/profile authorization stays in the CLI (Stage C/D).
        let declared = state.borrow().roles.iter().any(|r| r == &role);
        if !declared {
            state.borrow_mut().undeclared = Some(role.clone());
            return Err(JsError::from(JsNativeError::typ().with_message(format!(
                "agent() called role {role:?}, which this workflow's own meta.roles did not declare"
            ))));
        }

        // Hand back a genuinely pending promise; the VM never blocks.
        let (promise, resolvers) = JsPromise::new_pending(context);
        {
            let mut s = state.borrow_mut();
            let id = s.next_id;
            s.next_id = s.next_id.saturating_add(1);
            s.pending.push(PendingSpawn {
                id,
                role,
                prompt,
                opts,
            });
            s.awaiting.insert(id, resolvers);
        }
        Ok(promise.into())
    });
    context.register_global_builtin_callable(js_string!("agent"), 1, agent)
}

fn string_arg(args: &[JsValue], index: usize) -> Option<String> {
    args.get(index)
        .and_then(|v| v.as_string())
        .map(|s| s.to_std_string_lossy())
}

/// serde_json -> JsValue, depth-bounded (A2). Refuses past [`MAX_JSON_DEPTH`].
pub(crate) fn value_to_js(
    value: &serde_json::Value,
    context: &mut Context,
) -> Result<JsValue, WorkflowError> {
    value_to_js_depth(value, context, 0)
}

fn value_to_js_depth(
    value: &serde_json::Value,
    context: &mut Context,
    depth: usize,
) -> Result<JsValue, WorkflowError> {
    if depth > MAX_JSON_DEPTH {
        return Err(WorkflowError::internal(
            "step result JSON exceeds the maximum nesting depth",
        ));
    }
    use serde_json::Value as V;
    match value {
        V::Null => Ok(JsValue::null()),
        V::Bool(b) => Ok(JsValue::from(*b)),
        V::Number(n) => {
            let f = n
                .as_f64()
                .ok_or_else(|| WorkflowError::internal("JSON number is not representable"))?;
            Ok(JsValue::from(f))
        }
        V::String(s) => Ok(JsValue::from(JsString::from(s.as_str()))),
        V::Array(items) => {
            let arr = JsArray::new(context);
            for item in items {
                let v = value_to_js_depth(item, context, depth + 1)?;
                arr.push(v, context)
                    .map_err(|_| WorkflowError::internal("failed to build JS array"))?;
            }
            Ok(arr.into())
        }
        V::Object(map) => {
            let obj = JsObject::with_object_proto(context.intrinsics());
            for (k, v) in map {
                let jv = value_to_js_depth(v, context, depth + 1)?;
                obj.set(JsString::from(k.as_str()), jv, false, context)
                    .map_err(|_| WorkflowError::internal("failed to build JS object"))?;
            }
            Ok(obj.into())
        }
    }
}

/// JsValue -> serde_json, depth-bounded (A2). Refuses past [`MAX_JSON_DEPTH`],
/// mirroring `JSON.stringify` semantics (functions/undefined/symbol-keys
/// dropped, non-finite numbers become `null`).
pub(crate) fn js_to_value(
    value: &JsValue,
    context: &mut Context,
    depth: usize,
) -> Result<serde_json::Value, WorkflowError> {
    use serde_json::Value as V;
    if depth > MAX_JSON_DEPTH {
        return Err(WorkflowError::internal(
            "value JSON exceeds the maximum nesting depth",
        ));
    }
    match value.variant() {
        JsVariant::Null | JsVariant::Undefined => Ok(V::Null),
        JsVariant::Boolean(b) => Ok(V::Bool(b)),
        JsVariant::Integer32(i) => Ok(V::from(i)),
        JsVariant::Float64(f) => Ok(json_number(f)),
        JsVariant::String(s) => Ok(V::String(s.to_std_string_lossy())),
        // JSON has no representation for BigInt or Symbol values.
        JsVariant::BigInt(_) | JsVariant::Symbol(_) => Ok(V::Null),
        JsVariant::Object(obj) => {
            if obj.is_callable() {
                return Ok(V::Null);
            }
            if obj.is_array() {
                let arr = JsArray::from_object(obj)
                    .map_err(|_| WorkflowError::internal("array coercion failed"))?;
                let len = arr
                    .length(context)
                    .map_err(|_| WorkflowError::internal("array length read failed"))?;
                let mut out = Vec::new();
                for i in 0..len {
                    let elem = arr
                        .at(i as i64, context)
                        .map_err(|_| WorkflowError::internal("array element read failed"))?;
                    out.push(js_to_value(&elem, context, depth + 1)?);
                }
                Ok(V::Array(out))
            } else {
                let keys = obj
                    .own_property_keys(context)
                    .map_err(|_| WorkflowError::internal("property enumeration failed"))?;
                let mut map = serde_json::Map::new();
                for key in keys {
                    let name = match &key {
                        PropertyKey::String(s) => s.to_std_string_lossy(),
                        PropertyKey::Index(i) => i.get().to_string(),
                        // JSON.stringify skips symbol-keyed properties.
                        PropertyKey::Symbol(_) => continue,
                    };
                    let v = obj
                        .get(key, context)
                        .map_err(|_| WorkflowError::internal("property read failed"))?;
                    if v.is_undefined() {
                        continue;
                    }
                    map.insert(name, js_to_value(&v, context, depth + 1)?);
                }
                Ok(V::Object(map))
            }
        }
    }
}

fn json_number(f: f64) -> serde_json::Value {
    if !f.is_finite() {
        // JSON.stringify(NaN) === JSON.stringify(Infinity) === "null".
        return serde_json::Value::Null;
    }
    if f.fract() == 0.0 && f >= i64::MIN as f64 && f <= i64::MAX as f64 {
        return serde_json::Value::from(f as i64);
    }
    serde_json::Number::from_f64(f)
        .map(serde_json::Value::Number)
        .unwrap_or(serde_json::Value::Null)
}
