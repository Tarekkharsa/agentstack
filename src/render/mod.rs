//! Rendering targets: per-format non-destructive mergers and the apply
//! orchestration that produces a read-only plan.

pub mod apply;
pub mod hooks;
pub mod instructions;
pub mod merge_json;
pub mod merge_md;
pub mod merge_toml;
pub mod settings;
pub mod skills;

pub use apply::{
    effective_servers, plan_target, plan_target_with_servers, resolve_active_servers,
    resolve_targets, Selection, TargetPlan,
};
pub use hooks::{plan_hooks, HooksPlan};
pub use settings::{plan_settings, SettingsPlan};
