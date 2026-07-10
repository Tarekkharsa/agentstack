//! Varlock integration (<https://varlock.dev>).
//!
//! When a project uses varlock (a `.env.schema` is present and the `varlock`
//! binary is installed), we delegate the entire secret-provider matrix —
//! 1Password, AWS/Azure/GCP secret managers, Bitwarden, device-local
//! encryption — to it by shelling out to:
//!
//! ```text
//! varlock load --format json-full --compact
//! ```
//!
//! and parsing the resolved values. We never pass `--agent` (which would redact
//! sensitive values); we need the real values to write into the target configs.
//! Resolution happens once at construction and is cached.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use serde_json::Value;

use super::Resolver;

pub struct VarlockResolver {
    vars: HashMap<String, String>,
}

impl VarlockResolver {
    /// Activate varlock for `dir` if it both opts in (`.env.schema` present) and
    /// has the binary installed and loading succeeds. Returns `None` otherwise,
    /// so the chain silently skips varlock when it isn't in use.
    pub fn detect(dir: &Path) -> Option<Self> {
        if !dir.join(".env.schema").exists() {
            return None;
        }
        let output = Command::new("varlock")
            .args(["load", "--format", "json-full", "--compact"])
            .current_dir(dir)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let json: Value = serde_json::from_slice(&output.stdout).ok()?;
        Some(VarlockResolver { vars: parse(&json) })
    }

    #[cfg(test)]
    fn from_json(json: &Value) -> Self {
        VarlockResolver { vars: parse(json) }
    }
}

impl Resolver for VarlockResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        self.vars.get(name).cloned()
    }
}

/// Extract a flat `NAME -> value` map from varlock's JSON, tolerating both the
/// plain (`{ "NAME": "v" }`) and rich (`{ "NAME": { "value": "v", ... } }`)
/// shapes that `json-full` may produce across versions.
fn parse(json: &Value) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let Some(obj) = json.as_object() else {
        return out;
    };
    for (key, val) in obj {
        if let Some(v) = extract(val) {
            out.insert(key.clone(), v);
        }
    }
    out
}

fn extract(val: &Value) -> Option<String> {
    match val {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        Value::Object(o) => o.get("value").and_then(extract),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_plain_shape() {
        let r = VarlockResolver::from_json(&json!({ "KIBANA_TOKEN": "abc", "PORT": 8080 }));
        assert_eq!(r.resolve("KIBANA_TOKEN").as_deref(), Some("abc"));
        assert_eq!(r.resolve("PORT").as_deref(), Some("8080"));
    }

    #[test]
    fn parses_rich_shape() {
        let r = VarlockResolver::from_json(&json!({
            "KIBANA_TOKEN": { "value": "abc", "isSensitive": true },
            "API": { "value": "https://x" }
        }));
        assert_eq!(r.resolve("KIBANA_TOKEN").as_deref(), Some("abc"));
        assert_eq!(r.resolve("API").as_deref(), Some("https://x"));
        assert_eq!(r.resolve("MISSING"), None);
    }
}
