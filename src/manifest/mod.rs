//! The portable manifest: model, layered loading, and validation.

pub mod load;
pub mod model;
pub mod validate;

pub use load::{load_from_dir, LoadedManifest};
pub use model::{
    glob_match, Hook, Instruction, Manifest, PluginRecipe, Policy, Profile, Server, ServerType,
    Skill, SkillSource,
};
pub use validate::{validate, validate_with_targets, Issue, IssueKind};
