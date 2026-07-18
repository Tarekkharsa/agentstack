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
use std::sync::Mutex;

pub mod env_file;
pub mod keychain;
pub mod varlock;

pub use keychain::KeychainResolver;
pub use varlock::VarlockResolver;

// TODO(phase-1): shim — the resolution *contract* (Lookup, Resolver,
// MapResolver) lives in core; point callers at agentstack_core::secret and
// drop. Mechanisms below stay here.
pub use agentstack_core::secret::{Lookup, MapResolver, Resolver};

/// Tries each resolver in order, returning the first hit.
pub struct Chain {
    links: Vec<Box<dyn Resolver>>,
    /// One lookup per distinct name per `Chain` (≈ per command run). Rendering
    /// re-resolves the same `${REF}` for every target × server; without the
    /// cache a single flaky keychain read mid-run makes the ref resolve for
    /// some targets and count as unresolved for others.
    cache: Mutex<HashMap<String, Lookup>>,
}

impl Chain {
    pub fn new(links: Vec<Box<dyn Resolver>>) -> Self {
        Chain {
            links,
            cache: Mutex::new(HashMap::new()),
        }
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
        Chain::new(links)
    }
}

impl Resolver for Chain {
    fn resolve(&self, name: &str) -> Option<String> {
        self.lookup(name).found()
    }

    fn lookup(&self, name: &str) -> Lookup {
        if let Some(hit) = self
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(name)
        {
            return hit.clone();
        }
        // A failed link doesn't stop the walk — a later store may still have
        // the value. Only when nothing has it does the failure win over
        // `Missing`, so the caller reports "read failed", not "not set".
        let mut failure: Option<Lookup> = None;
        let mut out = Lookup::Missing;
        for link in &self.links {
            match link.lookup(name) {
                Lookup::Found(v) => {
                    out = Lookup::Found(v);
                    break;
                }
                Lookup::Failed(e) => {
                    failure.get_or_insert(Lookup::Failed(e));
                }
                Lookup::Missing => {}
                // A policy denial is terminal: no later store may satisfy a
                // ref the policy refuses. (Backing stores never produce
                // Denied — only the per-server scoping wrapper does — but the
                // chain must stay fail-closed if one ever lands here.)
                denied @ Lookup::Denied(_) => {
                    out = denied;
                    break;
                }
            }
        }
        if out == Lookup::Missing {
            if let Some(f) = failure {
                out = f;
            }
        }
        self.cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(name.to_string(), out.clone());
        out
    }
}

/// A [`Resolver`] that gates every `${REF}` through the compiled
/// `[policy.secrets]` for ONE server, then delegates to the real resolver.
/// A denied ref never reaches any backing store — not even to learn whether
/// it exists — and surfaces as [`Lookup::Denied`], which blocks the write or
/// call on the same fail-closed path as an unresolved secret (rule 5).
///
/// (TS mental model: a decorator — `class ScopedResolver implements Resolver
/// { constructor(private inner: Resolver, …) }`. `&dyn Resolver` is a trait
/// object, i.e. a reference typed by interface; the `'a` lifetimes just say
/// the wrapper can't outlive what it borrows.)
pub struct ScopedResolver<'a> {
    inner: &'a dyn Resolver,
    ruleset: &'a agentstack_policy::CompiledRuleset,
    server: &'a str,
}

impl<'a> ScopedResolver<'a> {
    pub fn new(
        inner: &'a dyn Resolver,
        ruleset: &'a agentstack_policy::CompiledRuleset,
        server: &'a str,
    ) -> Self {
        ScopedResolver {
            inner,
            ruleset,
            server,
        }
    }
}

impl Resolver for ScopedResolver<'_> {
    fn resolve(&self, name: &str) -> Option<String> {
        self.lookup(name).found()
    }

    fn lookup(&self, name: &str) -> Lookup {
        if let Err(rule) = self.ruleset.secret_decision(self.server, name) {
            return Lookup::Denied(format!(
                "server '{}' may not resolve ${{{name}}} — {rule}",
                self.server
            ));
        }
        self.inner.lookup(name)
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

// TODO(phase-1): shim — point callers at agentstack_core::refs and drop.
pub use agentstack_core::refs::{is_ref_name, refs_in};

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

    /// Counts lookups delegated to an inner resolver.
    pub(crate) struct CountingResolver<R> {
        pub inner: R,
        pub calls: std::rc::Rc<std::cell::Cell<usize>>,
    }

    impl<R: Resolver> Resolver for CountingResolver<R> {
        fn resolve(&self, name: &str) -> Option<String> {
            self.lookup(name).found()
        }
        fn lookup(&self, name: &str) -> Lookup {
            self.calls.set(self.calls.get() + 1);
            self.inner.lookup(name)
        }
    }

    #[test]
    fn chain_resolves_each_name_once() {
        let calls = std::rc::Rc::new(std::cell::Cell::new(0));
        let chain = Chain::new(vec![Box::new(CountingResolver {
            inner: MapResolver::from([("X", "v")]),
            calls: calls.clone(),
        })]);
        assert_eq!(chain.resolve("X").as_deref(), Some("v"));
        assert_eq!(chain.resolve("X").as_deref(), Some("v"));
        assert_eq!(chain.lookup("X"), Lookup::Found("v".into()));
        assert_eq!(calls.get(), 1, "hit cached after the first lookup");

        // Misses are cached too — a consistent outcome per name per run.
        calls.set(0);
        assert_eq!(chain.resolve("MISSING"), None);
        assert_eq!(chain.resolve("MISSING"), None);
        assert_eq!(calls.get(), 1, "miss cached after the first lookup");
    }

    struct FailingResolver;
    impl Resolver for FailingResolver {
        fn resolve(&self, name: &str) -> Option<String> {
            self.lookup(name).found()
        }
        fn lookup(&self, _name: &str) -> Lookup {
            Lookup::Failed("store read failed".into())
        }
    }

    #[test]
    fn chain_failed_link_falls_through_but_wins_over_missing() {
        let chain = Chain::new(vec![
            Box::new(FailingResolver),
            Box::new(MapResolver::from([("X", "v")])),
        ]);
        // A later store can still satisfy the lookup…
        assert_eq!(chain.lookup("X"), Lookup::Found("v".into()));
        // …but when nothing has it, the failure is reported, not a miss.
        assert_eq!(
            chain.lookup("Z"),
            Lookup::Failed("store read failed".into())
        );
        assert_eq!(chain.resolve("Z"), None);
    }
}
