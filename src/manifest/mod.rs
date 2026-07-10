//! The portable manifest: model, layered loading, and validation.

pub mod load;
pub mod model;
pub mod validate;

pub use load::{
    discover_project_base, load_from_dir, machine_policy, machine_policy_health, merge_user_layer,
    new_manifest_dir, project_root_of, resolve_manifest_dir, LoadedManifest, MANIFEST_FILE,
    MANIFEST_SUBDIR, SUPPORTED_MANIFEST_VERSION,
};
pub use model::{
    glob_match, Hook, Instruction, Manifest, PluginRecipe, Policy, Profile, Server, ServerType,
    Skill, SkillSource,
};
pub use validate::{
    validate, validate_with_context, validate_with_targets, Issue, IssueKind, ValidateCtx,
};
