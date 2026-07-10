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
