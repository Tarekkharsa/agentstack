//! Capability scope: **global** (personal — active in every project) vs
//! **project** (active only inside a repo). Scope decides which config/skills
//! locations a target writes to. See PLAN §9b.

use std::fmt;

use clap::ValueEnum;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Global,
    Project,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Scope::Global => "global",
            Scope::Project => "project",
        }
    }

    /// The default write scope for the manifest at `manifest_dir`: **global**
    /// only when it is the machine/personal manifest (the machine home —
    /// `~/.agentstack`, or a relocated `AGENTSTACK_HOME`), **project** for any
    /// manifest discovered in a repo. Same machine-home rule
    /// [`crate::manifest::discover_project_base`] uses, compared canonicalized
    /// so symlinked spellings still match. An explicit `--scope` always
    /// overrides this.
    pub fn default_for(manifest_dir: &std::path::Path) -> Scope {
        let machine_home = crate::util::paths::agentstack_home();
        if manifest_dir == machine_home
            || matches!(
                (manifest_dir.canonicalize(), machine_home.canonicalize()),
                (Ok(a), Ok(b)) if a == b
            )
        {
            Scope::Global
        } else {
            Scope::Project
        }
    }
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for Scope {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "global" => Ok(Scope::Global),
            "project" => Ok(Scope::Project),
            other => Err(format!("unknown scope '{other}' (expected global|project)")),
        }
    }
}
