//! The portable manifest: model and layered loading.
//!
//! Validation lives in the `cli` crate until the library/resolver it walks
//! are extracted; it is re-exported there as `manifest::validate` so callers
//! see one module.

pub mod load;
pub mod model;

pub use load::{
    discover_project_base, load_from_dir, machine_policy, machine_policy_health, merge_user_layer,
    new_manifest_dir, project_root_of, resolve_manifest_dir, LoadedManifest, MANIFEST_FILE,
    MANIFEST_SUBDIR, SUPPORTED_MANIFEST_VERSION,
};
pub use model::{
    egress_match, glob_match, FsPolicy, Hook, Instruction, Manifest, PluginRecipe, Policy, Profile,
    Server, ServerType, Skill, SkillSource,
};
