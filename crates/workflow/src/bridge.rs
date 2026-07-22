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
use boa_engine::object::{FunctionObjectBuilder, IntegrityLevel, JsObject, ObjectInitializer};
use boa_engine::property::{Attribute, PropertyKey};
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

/// Rule-7 bounds on the `phase()` / `log()` progress surface: these strings
/// are script-controlled text destined for the invoker's terminal, so both
/// the per-line size and the buffered count are hard-capped. Overflow is
/// counted honestly, never silently discarded.
pub(crate) const MAX_PROGRESS_LINE_BYTES: usize = 2048;
pub(crate) const MAX_PROGRESS_EVENTS: usize = 1000;

/// One `phase(title)` / `log(msg)` call surfaced to the CLI. The strings are
/// SCRIPT-CONTROLLED and size-bounded here, but not otherwise sanitized — the
/// CLI must sanitize before printing (terminal escapes are its concern).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Progress {
    Phase(String),
    Log(String),
}

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
    /// Because it increments ONLY when a spawn is granted, it doubles as the
    /// spawned-so-far count — the single source `budget.spawned()`,
    /// `budget.remaining()`, and the exhaustion check below all derive from
    /// (the Stage D ceiling-identity requirement: one count, not two).
    pub(crate) next_id: u64,
    /// The granted effective `max_agents` ceiling (machine ∩ manifest ∩ script
    /// meta, computed by the CLI and re-clamped at construction). The SAME
    /// field feeds the exhaustion check and `budget` — pacing on
    /// `budget.remaining()` provably never trips exhaustion.
    pub(crate) max_agents: u32,
    /// Set ONLY by the `agent()` native when a call is refused at the ceiling.
    /// Host memory a script cannot reach — the non-forgeable exhaustion tag
    /// (same pattern as `compile_denied`): classification and the CLI's
    /// honesty line read this flag, never any script-controlled string.
    pub(crate) exhausted: bool,
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
    /// Buffered `phase()`/`log()` events since the last drain, capped at
    /// [`MAX_PROGRESS_EVENTS`]; overflow increments `progress_dropped`.
    pub(crate) progress: Vec<Progress>,
    pub(crate) progress_dropped: u64,
}

impl SpawnState {
    pub(crate) fn new(roles: Vec<String>, max_agents: u32) -> Self {
        Self {
            next_id: 0,
            max_agents,
            exhausted: false,
            roles,
            pending: Vec::new(),
            awaiting: HashMap::new(),
            undeclared: None,
            progress: Vec::new(),
            progress_dropped: 0,
        }
    }

    /// Remove and return all pending requests (the next batch).
    pub(crate) fn take_pending(&mut self) -> Vec<PendingSpawn> {
        std::mem::take(&mut self.pending)
    }

    /// Buffer one progress event under the rule-7 caps.
    fn push_progress(&mut self, event: Progress) {
        if self.progress.len() >= MAX_PROGRESS_EVENTS {
            self.progress_dropped = self.progress_dropped.saturating_add(1);
            return;
        }
        self.progress.push(event);
    }
}

/// Truncate to at most [`MAX_PROGRESS_LINE_BYTES`] on a char boundary. A
/// truncated line gets an ellipsis so the cut is visible, never silent.
fn bound_progress_line(s: &str) -> String {
    if s.len() <= MAX_PROGRESS_LINE_BYTES {
        return s.to_string();
    }
    let mut end = MAX_PROGRESS_LINE_BYTES;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
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

        // Stage D exhaustion, checked PER CALL (partial-batch semantics: in a
        // batch straddling the ceiling, the first K calls that fit spawn
        // normally and only the excess fails — a batch-boundary total check is
        // exactly the Stage C crude stop this replaces). The refused call
        // fails closed with a SYNCHRONOUS catchable throw — no promise is
        // ever created, so R1 ("the promise never rejects") holds untouched.
        // Through `parallel()`/`pipeline()` the throw becomes that slot's
        // `null`; uncaught, it fails the run as the distinct AgentsExhausted
        // kind via the non-forgeable `exhausted` flag set here.
        {
            let mut s = state.borrow_mut();
            if s.next_id >= u64::from(s.max_agents) {
                let granted = s.max_agents;
                s.exhausted = true;
                return Err(JsError::from(JsNativeError::error().with_message(format!(
                    "agent() refused: all {granted} granted agent slot(s) are spent — this call \
                     fails closed; pace with budget.remaining() to avoid exhaustion"
                ))));
            }
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

/// Install the global `phase(title)` / `log(msg)` progress functions. Same
/// capture-free thread-local pattern as `agent()`: each pushes one bounded
/// [`Progress`] event into the active `SpawnState`, for the CLI to drain via
/// `WorkflowRun::take_progress` after the drive returns. Strict string
/// arguments (like `agent`'s prompt): coercion would run script code
/// (`toString`) inside a host native for no benefit.
pub(crate) fn install_progress(context: &mut Context) -> JsResult<()> {
    fn progress_native(
        name: &'static str,
        make: fn(String) -> Progress,
    ) -> impl Fn(&JsValue, &[JsValue], &mut Context) -> JsResult<JsValue> + Copy {
        move |_this, args, _context| {
            let state = current_state().ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ()
                        .with_message(format!("{name}() called outside a workflow drive")),
                )
            })?;
            let text = string_arg(args, 0).ok_or_else(|| {
                JsError::from(
                    JsNativeError::typ()
                        .with_message(format!("{name}(text): text must be a string")),
                )
            })?;
            state
                .borrow_mut()
                .push_progress(make(bound_progress_line(&text)));
            Ok(JsValue::undefined())
        }
    }

    let phase = NativeFunction::from_copy_closure(progress_native("phase", Progress::Phase));
    context.register_global_builtin_callable(js_string!("phase"), 1, phase)?;
    let log = NativeFunction::from_copy_closure(progress_native("log", Progress::Log));
    context.register_global_builtin_callable(js_string!("log"), 1, log)
}

/// Install the global `budget` object (Stage D): the script-visible view of
/// the granted ceilings and consumption — **deterministic by construction**
/// (the §3 determinism rule; Stage F journal replay depends on it). It exposes
/// exactly four members and nothing time-derived:
///
/// - `maxAgents` / `maxWallSeconds` — granted static numbers. `maxWallSeconds`
///   is informational only: no clock exists in the runtime to pace against
///   (wall enforcement is the CLI's, live-run only).
/// - `spawned()` / `remaining()` — spawn-derived counts, read from the SAME
///   `SpawnState` fields the `agent()` exhaustion check enforces (`next_id`,
///   `max_agents`) — one source, so pacing on `remaining()` reliably never
///   trips exhaustion.
///
/// v1 budget is agent-count, not tokens: child token usage is `"unavailable"`
/// in the run evidence today, so no token view is faked here.
///
/// The object is FROZEN (non-extensible, every member non-writable and
/// non-configurable) and the `budget` global itself is
/// non-writable/non-configurable (like `args`) — the four-member shape is the
/// whole contract, and a script can neither replace it nor grow it.
pub(crate) fn install_budget(
    context: &mut Context,
    max_agents: u32,
    max_wall_seconds: u64,
) -> JsResult<()> {
    let spawned = NativeFunction::from_copy_closure(|_this, _args, _context| {
        let state = current_state().ok_or_else(|| {
            JsError::from(
                JsNativeError::typ()
                    .with_message("budget.spawned() called outside a workflow drive"),
            )
        })?;
        let n = state.borrow().next_id;
        Ok(JsValue::from(n as f64))
    });
    let remaining = NativeFunction::from_copy_closure(|_this, _args, _context| {
        let state = current_state().ok_or_else(|| {
            JsError::from(
                JsNativeError::typ()
                    .with_message("budget.remaining() called outside a workflow drive"),
            )
        })?;
        let s = state.borrow();
        let rem = u64::from(s.max_agents).saturating_sub(s.next_id);
        Ok(JsValue::from(rem as f64))
    });

    let realm = context.realm().clone();
    let spawned_fn = FunctionObjectBuilder::new(&realm, spawned)
        .name(js_string!("spawned"))
        .length(0)
        .build();
    let remaining_fn = FunctionObjectBuilder::new(&realm, remaining)
        .name(js_string!("remaining"))
        .length(0)
        .build();

    // ENUMERABLE alone = non-writable, non-configurable, visible to
    // Object.keys — the shape the determinism-probe witness asserts.
    // The u64→f64 cast is exact for any realistic wall ceiling (meta bounds
    // script requests at 2^53; machine/manifest values are human-written).
    let budget = ObjectInitializer::new(context)
        .property(
            js_string!("maxAgents"),
            f64::from(max_agents),
            Attribute::ENUMERABLE,
        )
        .property(
            js_string!("maxWallSeconds"),
            max_wall_seconds as f64,
            Attribute::ENUMERABLE,
        )
        .property(js_string!("spawned"), spawned_fn, Attribute::ENUMERABLE)
        .property(js_string!("remaining"), remaining_fn, Attribute::ENUMERABLE)
        .build();
    // Freeze: the attributes above already lock the four members; this also
    // makes the object non-extensible, so the four-member shape IS the
    // contract (`Object.isFrozen(budget)` holds — the probe witness asserts
    // it). Fail closed if the freeze reports false (it cannot for an
    // ordinary object, but a silent partial freeze must never ship).
    if !budget.set_integrity_level(IntegrityLevel::Frozen, context)? {
        return Err(JsError::from(
            JsNativeError::typ().with_message("failed to freeze the budget object"),
        ));
    }
    context.register_global_property(js_string!("budget"), budget, Attribute::empty())
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
