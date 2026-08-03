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
    ///
    /// Resolution semantics are unchanged by [`health`]: both go through the
    /// one [`load`] below, so what `doctor` reports and what the chain does can
    /// never be two different answers.
    pub fn detect(dir: &Path) -> Option<Self> {
        match load(dir) {
            Load::Loaded(vars) => Some(VarlockResolver { vars }),
            _ => None,
        }
    }

    #[cfg(test)]
    fn from_json(json: &Value) -> Self {
        VarlockResolver { vars: parse(json) }
    }
}

/// What varlock can do for this project right now — what `doctor` reports.
///
/// It deliberately says nothing about VALUES. A count of available names is the
/// most it carries; no secret is read into this type, printed, or logged, and
/// `LoadFailed` carries varlock's own diagnostic line, bounded and sanitized,
/// never its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Health {
    /// No `.env.schema`: this project has not opted in. Not a defect — the
    /// chain simply has one fewer layer.
    NotOptedIn,
    /// Opted in, but the `varlock` binary could not be spawned. This is the
    /// silent-degradation case worth reporting: the project asked for varlock
    /// and every ref is quietly falling through to keychain or `.env`.
    NotInstalled,
    /// Opted in and installed, but `varlock load` failed or its output could
    /// not be parsed.
    LoadFailed(String),
    /// Opted in and working, serving this many names.
    Ready { names: usize },
}

/// Probe varlock for `dir` without activating it. Read-only: it runs the same
/// `varlock load` the chain runs and throws the values away.
pub fn health(dir: &Path) -> Health {
    match load(dir) {
        Load::NotOptedIn => Health::NotOptedIn,
        Load::NotInstalled => Health::NotInstalled,
        Load::Failed(why) => Health::LoadFailed(why),
        Load::Loaded(vars) => Health::Ready { names: vars.len() },
    }
}

/// The single varlock invocation, behind both [`VarlockResolver::detect`] and
/// [`health`]. Every non-`Loaded` arm is a reason the chain skips the layer.
enum Load {
    NotOptedIn,
    NotInstalled,
    Failed(String),
    Loaded(HashMap<String, String>),
}

fn load(dir: &Path) -> Load {
    if !dir.join(".env.schema").exists() {
        return Load::NotOptedIn;
    }
    let output = match Command::new("varlock")
        .args(["load", "--format", "json-full", "--compact"])
        .current_dir(dir)
        .output()
    {
        Ok(output) => output,
        // A spawn failure is "not installed" for every practical purpose here;
        // the distinction between ENOENT and EACCES does not change the fix.
        Err(_) => return Load::NotInstalled,
    };
    if !output.status.success() {
        return Load::Failed(first_line(&output.stderr));
    }
    match serde_json::from_slice::<Value>(&output.stdout) {
        Ok(json) => Load::Loaded(parse(&json)),
        Err(e) => Load::Failed(format!("could not parse `varlock load` output: {e}")),
    }
}

/// varlock's first diagnostic line, bounded and sanitized before it can reach a
/// terminal. Subprocess output is untrusted text like any other.
fn first_line(stderr: &[u8]) -> String {
    const CAP: usize = 200;
    let text = String::from_utf8_lossy(stderr);
    let line = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("`varlock load` exited nonzero");
    let clipped: String = line.chars().take(CAP).collect();
    crate::text::sanitize_line(&clipped)
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
