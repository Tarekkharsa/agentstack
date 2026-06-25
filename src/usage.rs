//! Local usage analytics: `~/.agentstack/usage.json` (PLAN §9g, D21).
//!
//! v1 records **activation counts** — how often agentstack has materialized or
//! rendered each capability. Exact and cheap. Transcript-mined *invocation*
//! counts come later. Local-only; never leaves the machine.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::util::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Usage {
    pub version: u32,
    /// Capability name → number of times activated.
    #[serde(default)]
    pub activations: BTreeMap<String, u64>,
}

impl Default for Usage {
    fn default() -> Self {
        Usage {
            version: 1,
            activations: BTreeMap::new(),
        }
    }
}

impl Usage {
    pub fn path() -> PathBuf {
        paths::agentstack_home().join("usage.json")
    }

    pub fn load() -> Result<Self> {
        match fs::read_to_string(Self::path()) {
            Ok(text) => serde_json::from_str(&text).context("parsing usage.json"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Usage::default()),
            Err(e) => Err(e).context("reading usage.json"),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        fs::write(&path, text).context("writing usage.json")
    }

    pub fn count(&self, name: &str) -> u64 {
        self.activations.get(name).copied().unwrap_or(0)
    }
}

/// Increment activation counts for `names` (best-effort; never fails a command).
pub fn bump(names: &[String]) {
    if names.is_empty() {
        return;
    }
    if let Ok(mut usage) = Usage::load() {
        for n in names {
            *usage.activations.entry(n.clone()).or_insert(0) += 1;
        }
        let _ = usage.save();
    }
}
