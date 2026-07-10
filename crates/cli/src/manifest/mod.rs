//! The portable manifest, re-exported from `agentstack-core`, plus
//! validation — which stays here because it walks the central library and
//! resolver (not yet extracted). Callers keep seeing one `manifest` module.

pub use agentstack_core::manifest::*;

pub mod validate;

pub use validate::{
    validate, validate_with_context, validate_with_targets, Issue, IssueKind, ValidateCtx,
};
