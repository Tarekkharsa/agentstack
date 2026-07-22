//! Error taxonomy for the workflow engine.
//!
//! Mirrors `executor`'s error pattern (a `Copy` category enum + a struct whose
//! `Display` leaks nothing, plus a `public_message()` that maps each category
//! to a fixed, safe string). Internal detail lives in a private `message`
//! field for logs; it never reaches an untrusted caller.

/// The class of a workflow failure. `Copy` so it can be compared and matched
/// freely (like a TypeScript union tag).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkflowErrorKind {
    /// The script (wrapped as an async function body) did not parse.
    InvalidScript,
    /// `meta` is absent, is not an object literal, or has a non-literal value.
    MetaViolation,
    /// `agent()` named a role the script's own `meta.roles` did not declare.
    /// This is *script-internal consistency*, not an authority check — the
    /// manifest/profile authorization gate lives in the CLI (Stage C/D).
    UndeclaredRole,
    /// A Boa `RuntimeLimit` (loop / recursion / stack) fired. Non-catchable
    /// from JavaScript, so a script can never swallow its own ceiling.
    IterationLimit,
    /// The script tried to turn a string into code (`eval` / `Function(str)`);
    /// denied by the compile-strings host hook.
    CompileDenied,
    /// A native-function panic was caught. The Boa `Context` it unwound through
    /// is discarded and the `WorkflowRun` refuses further steps.
    Panicked,
    /// An uncaught JavaScript throw settled the root promise as rejected.
    RuntimeError,
    /// A host/driver invariant broke (unknown step id, JSON too deep, etc.).
    Internal,
}

/// A workflow failure. `Display` prints only the internal `message`; callers
/// that must not see detail use [`WorkflowError::public_message`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct WorkflowError {
    pub kind: WorkflowErrorKind,
    message: String,
}

impl WorkflowError {
    /// A fixed, allocation-free string safe to surface to any caller. The
    /// private `message` (which may name internal specifics) is never returned.
    pub fn public_message(&self) -> &'static str {
        match self.kind {
            WorkflowErrorKind::InvalidScript => "the workflow script could not be parsed",
            WorkflowErrorKind::MetaViolation => {
                "the workflow meta block is missing or is not static literals"
            }
            WorkflowErrorKind::UndeclaredRole => {
                "the script called a role it did not declare in meta.roles"
            }
            WorkflowErrorKind::IterationLimit => {
                "the workflow exceeded an interpreter execution limit"
            }
            WorkflowErrorKind::CompileDenied => "the workflow tried to compile a string into code",
            WorkflowErrorKind::Panicked => "the workflow interpreter aborted and was discarded",
            WorkflowErrorKind::RuntimeError => "the workflow script raised an uncaught error",
            WorkflowErrorKind::Internal => "the workflow engine hit an internal error",
        }
    }

    pub(crate) fn invalid_script(message: impl Into<String>) -> Self {
        Self {
            kind: WorkflowErrorKind::InvalidScript,
            message: message.into(),
        }
    }

    pub(crate) fn meta_violation(message: impl Into<String>) -> Self {
        Self {
            kind: WorkflowErrorKind::MetaViolation,
            message: message.into(),
        }
    }

    pub(crate) fn undeclared_role(role: &str) -> Self {
        Self {
            kind: WorkflowErrorKind::UndeclaredRole,
            message: format!("agent() named role {role:?}, absent from meta.roles"),
        }
    }

    pub(crate) fn iteration_limit(message: impl Into<String>) -> Self {
        Self {
            kind: WorkflowErrorKind::IterationLimit,
            message: message.into(),
        }
    }

    pub(crate) fn compile_denied() -> Self {
        Self {
            kind: WorkflowErrorKind::CompileDenied,
            message: "dynamic string compilation is denied".into(),
        }
    }

    pub(crate) fn panicked() -> Self {
        Self {
            kind: WorkflowErrorKind::Panicked,
            message: "a native function panicked; context discarded".into(),
        }
    }

    pub(crate) fn runtime_error(message: impl Into<String>) -> Self {
        Self {
            kind: WorkflowErrorKind::RuntimeError,
            message: message.into(),
        }
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self {
            kind: WorkflowErrorKind::Internal,
            message: message.into(),
        }
    }
}
