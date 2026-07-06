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

/// Like [`Chain::default_for_dir`], but keeps each layer separate so it can
/// report *where* a `${REF}` resolves — shared by `secret list`, `explain`, and
/// the dashboard. Priority matches the chain: env → varlock → keychain → `.env`.
pub struct SecretSources {
    env: EnvResolver,
    varlock: Option<VarlockResolver>,
    keychain: KeychainResolver,
    dotenv: Option<DotEnvResolver>,
}

impl SecretSources {
    pub fn detect(dir: &Path) -> Self {
        SecretSources {
            env: EnvResolver,
            varlock: VarlockResolver::detect(dir),
            keychain: KeychainResolver,
            dotenv: DotEnvResolver::from_dir(dir),
        }
    }

    /// The layer a reference resolves from, or `None` if it doesn't resolve here.
    pub fn source_of(&self, name: &str) -> Option<&'static str> {
        if self.env.resolve(name).is_some() {
            Some("env")
        } else if self
            .varlock
            .as_ref()
            .and_then(|v| v.resolve(name))
            .is_some()
        {
            Some("varlock")
        } else if self.keychain.resolve(name).is_some() {
            Some("keychain")
        } else if self.dotenv.as_ref().and_then(|d| d.resolve(name)).is_some() {
            Some(".env")
        } else {
            None
        }
    }
}

/// Whether `s` is a valid reference name: `[A-Za-z_][A-Za-z0-9_]*`. Anything
/// else between `${` and `}` — e.g. shell fallback syntax like
/// `${VAR:-$OTHER}` inside a `zsh -lc` argument — is the shell's business,
/// not a secret reference.
pub fn is_ref_name(s: &str) -> bool {
    let mut chars = s.chars();
    chars
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Extract the `${NAME}` reference names from a string, in order of appearance.
/// `${…}` spans that are not valid names are skipped (their interior is still
/// scanned, so `${A:-${B}}` yields `B`).
pub fn refs_in(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'$' && i + 1 < bytes.len() && bytes[i + 1] == b'{' {
            if let Some(end) = s[i + 2..].find('}') {
                let name = &s[i + 2..i + 2 + end];
                if is_ref_name(name) {
                    out.push(name.to_string());
                    i = i + 2 + end + 1;
                    continue;
                }
            }
            // Not a reference — step past `${` and keep scanning the interior.
            i += 2;
            continue;
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
    fn refs_in_extracts_names_and_skips_shell_syntax() {
        assert_eq!(refs_in("Bearer ${TOKEN}"), vec!["TOKEN"]);
        assert_eq!(refs_in("${A} and ${B_2}"), vec!["A", "B_2"]);
        // Shell fallback syntax inside a command arg is not a reference.
        assert!(refs_in("x=${MIRO_ACCESS_TOKEN:-$MIRO_OAUTH_TOKEN}").is_empty());
        // …but a real reference nested in one still counts.
        assert_eq!(refs_in("${A:-${B}}"), vec!["B"]);
        // Other invalid names: empty, leading digit, spaces.
        assert!(refs_in("${}, ${1X}, ${A B}").is_empty());
    }

    #[test]
    fn ref_name_validity() {
        assert!(is_ref_name("GITHUB_TOKEN"));
        assert!(is_ref_name("_x9"));
        assert!(!is_ref_name(""));
        assert!(!is_ref_name("9X"));
        assert!(!is_ref_name("A:-B"));
        assert!(!is_ref_name("A$B"));
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
