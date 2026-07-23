//! Context-cost lens: what each MCP server costs a harness in context-window
//! tokens, per session. Every server's `tools/list` payload (names,
//! descriptions, input schemas) is injected into the model's context by the
//! harness — a server you never call still taxes every conversation.
//!
//! Measurement goes through the gateway (so HTTP and stdio servers both work)
//! and is cached in `~/.agentstack/footprint.json`; offline commands — `stats`,
//! `explain`, t3code — read the cache and never spawn or connect.
//! Measure with `agentstack report usage --live`.
//!
//! Token counts are the standard ~4-chars-per-token heuristic over the
//! serialized tool JSON — a ranking signal, not an exact bill. The namespaced
//! form adds a short `[via <server>]` provenance prefix per tool; the handful
//! of tokens it adds is noise at ranking granularity.

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::util::paths;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Footprints {
    pub version: u32,
    /// Server name → its last measured context cost.
    #[serde(default)]
    pub servers: BTreeMap<String, ServerFootprint>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerFootprint {
    /// Number of tools the server exposed when measured.
    pub tools: usize,
    /// Estimated tokens its tools/list payload occupies in context.
    pub est_tokens: u64,
    /// Unix epoch seconds of the measurement.
    pub measured_at: u64,
}

impl Default for Footprints {
    fn default() -> Self {
        Footprints {
            version: 1,
            servers: BTreeMap::new(),
        }
    }
}

impl Footprints {
    pub fn path() -> PathBuf {
        paths::agentstack_home().join("footprint.json")
    }

    pub fn load() -> Result<Self> {
        match fs::read_to_string(Self::path()) {
            Ok(text) => serde_json::from_str(&text).context("parsing footprint.json"),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Footprints::default()),
            Err(e) => Err(e).context("reading footprint.json"),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = Self::path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut text = serde_json::to_string_pretty(self)?;
        text.push('\n');
        fs::write(&path, text).context("writing footprint.json")
    }

    pub fn get(&self, server: &str) -> Option<&ServerFootprint> {
        self.servers.get(server)
    }
}

/// ~4 characters per token — the usual rough heuristic for English and JSON.
pub fn estimate_tokens(chars: usize) -> u64 {
    (chars as u64).div_ceil(4)
}

/// Compact human form: `320 tok`, `4.2k tok`.
pub fn fmt_tokens(t: u64) -> String {
    if t >= 1000 {
        format!("{:.1}k tok", t as f64 / 1000.0)
    } else {
        format!("{t} tok")
    }
}

/// `measured today` / `measured 3d ago`, from an epoch-seconds stamp.
pub fn fmt_age(measured_at: u64) -> String {
    let now = now_epoch();
    let days = now.saturating_sub(measured_at) / 86_400;
    if days == 0 {
        "measured today".to_string()
    } else {
        format!("measured {days}d ago")
    }
}

pub fn now_epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Group a gateway's namespaced tool list (`<server>__<tool>` entries) into
/// per-server footprints. Pure — callers do the live discovery.
pub fn measure(namespaced_tools: &[Value]) -> BTreeMap<String, ServerFootprint> {
    let now = now_epoch();
    let mut out: BTreeMap<String, ServerFootprint> = BTreeMap::new();
    for t in namespaced_tools {
        let Some(name) = t.get("name").and_then(Value::as_str) else {
            continue;
        };
        let Some((server, _)) = name.split_once("__") else {
            continue;
        };
        let chars = serde_json::to_string(t).map(|s| s.len()).unwrap_or(0);
        let e = out.entry(server.to_string()).or_insert(ServerFootprint {
            tools: 0,
            est_tokens: 0,
            measured_at: now,
        });
        e.tools += 1;
        e.est_tokens += estimate_tokens(chars);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn estimates_and_formats() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 2);
        assert_eq!(fmt_tokens(320), "320 tok");
        assert_eq!(fmt_tokens(4200), "4.2k tok");
    }

    #[test]
    fn measures_grouped_by_server() {
        let tools = vec![
            json!({ "name": "figma__get_file", "description": "d", "inputSchema": {} }),
            json!({ "name": "figma__create_frame", "description": "d", "inputSchema": {} }),
            json!({ "name": "github__list_issues", "description": "d", "inputSchema": {} }),
            json!({ "name": "not-namespaced", "description": "skipped" }),
        ];
        let m = measure(&tools);
        assert_eq!(m.len(), 2);
        assert_eq!(m["figma"].tools, 2);
        assert_eq!(m["github"].tools, 1);
        assert!(m["figma"].est_tokens > m["github"].est_tokens);
    }
}
