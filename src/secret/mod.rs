//! Secret resolution.
//!
//! The manifest holds only `${NAME}` references; real values are resolved
//! per-machine through an ordered chain (first hit wins):
//!
//! 1. **process env** — explicit, wins over everything (handy for CI / one-offs)
//! 2. **varlock** — when the project opts in (a `.env.schema` is present and the
//!    `varlock` binary is installed); delegates 1Password/AWS/encrypted-local/…
//! 3. **OS keychain** — agentstack's own managed store (`secret set` writes here)
//! 4. **project `.env`** — plain-text fallback
//!
//! New resolvers slot in behind the same [`Resolver`] trait without touching
//! callers.

use std::collections::HashMap;
use std::path::Path;

pub mod keychain;
pub mod varlock;

pub use keychain::KeychainResolver;
pub use varlock::VarlockResolver;

/// Anything that can turn a reference name into its secret value.
pub trait Resolver {
    fn resolve(&self, name: &str) -> Option<String>;
}

/// Tries each resolver in order, returning the first hit.
pub struct Chain {
    links: Vec<Box<dyn Resolver>>,
}

impl Chain {
    pub fn new(links: Vec<Box<dyn Resolver>>) -> Self {
        Chain { links }
    }

    /// The default chain for a manifest directory: env → varlock → keychain →
    /// `.env`. Varlock and `.env` links are only added when present.
    pub fn default_for_dir(dir: &Path) -> Self {
        let mut links: Vec<Box<dyn Resolver>> = vec![Box::new(EnvResolver)];
        if let Some(vl) = VarlockResolver::detect(dir) {
            links.push(Box::new(vl));
        }
        links.push(Box::new(KeychainResolver));
        if let Some(dotenv) = DotEnvResolver::from_dir(dir) {
            links.push(Box::new(dotenv));
        }
        Chain { links }
    }
}

impl Resolver for Chain {
    fn resolve(&self, name: &str) -> Option<String> {
        self.links.iter().find_map(|l| l.resolve(name))
    }
}

/// Extract the `${NAME}` reference names from a string, in order of appearance.
pub fn refs_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = s[i + 2..].find('}') {
                out.push(s[i + 2..i + 2 + end].to_string());
                i = i + 2 + end + 1;
                continue;
            }
        }
        i += 1;
    }
    out
}

/// Process environment variables (`$NAME`).
pub struct EnvResolver;

impl Resolver for EnvResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        std::env::var(name).ok()
    }
}

/// A `.env` file (`NAME=value` lines, `#` comments, optional surrounding
/// quotes). Intentionally minimal — no interpolation.
pub struct DotEnvResolver {
    vars: HashMap<String, String>,
}

impl DotEnvResolver {
    pub fn from_dir(dir: &Path) -> Option<Self> {
        let path = dir.join(".env");
        let text = std::fs::read_to_string(path).ok()?;
        Some(Self::parse(&text))
    }

    pub fn parse(text: &str) -> Self {
        let mut vars = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let line = line.strip_prefix("export ").unwrap_or(line);
            if let Some((k, v)) = line.split_once('=') {
                let v = v.trim().trim_matches('"').trim_matches('\'');
                vars.insert(k.trim().to_string(), v.to_string());
            }
        }
        DotEnvResolver { vars }
    }
}

impl Resolver for DotEnvResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        self.vars.get(name).cloned()
    }
}

/// In-memory resolver for tests and deterministic rendering.
#[derive(Default)]
pub struct MapResolver {
    vars: HashMap<String, String>,
}

impl<const N: usize> From<[(&str, &str); N]> for MapResolver {
    fn from(pairs: [(&str, &str); N]) -> Self {
        MapResolver {
            vars: pairs
                .into_iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }
}

impl Resolver for MapResolver {
    fn resolve(&self, name: &str) -> Option<String> {
        self.vars.get(name).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dotenv_parses_lines() {
        let r = DotEnvResolver::parse("# comment\nexport A=1\nB=\"two\"\n\nC = 'three'\n");
        assert_eq!(r.resolve("A").as_deref(), Some("1"));
        assert_eq!(r.resolve("B").as_deref(), Some("two"));
        assert_eq!(r.resolve("C").as_deref(), Some("three"));
        assert_eq!(r.resolve("Z"), None);
    }

    #[test]
    fn chain_first_hit_wins() {
        let chain = Chain::new(vec![
            Box::new(MapResolver::from([("X", "first")])),
            Box::new(MapResolver::from([("X", "second")])),
        ]);
        assert_eq!(chain.resolve("X").as_deref(), Some("first"));
    }
}
