//! Rendering targets: per-format non-destructive mergers and the apply
//! orchestration that produces a read-only plan.

pub mod apply;
pub mod extensions;
pub mod gitignore;
pub mod hooks;
pub mod instructions;
pub mod merge_json;
pub mod merge_md;
pub mod merge_toml;
pub mod owned;
pub mod settings;
pub mod skills;

pub(crate) use apply::declared_host;
pub use apply::{
    effective_servers, failed_secret_line, plan_target, plan_target_with_servers,
    resolve_active_servers, resolve_targets, ruleset_for, Selection, TargetPlan,
};
pub use hooks::{plan_hooks, HooksPlan};
pub use owned::{refresh_owned_servers, OwnedStatus};
pub use settings::{plan_settings, SettingsPlan};
