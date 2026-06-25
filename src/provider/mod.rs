//! Capability **providers** — the meta-layer (PLAN §9h). agentstack doesn't run
//! a registry; it *consumes* them. A `Provider` searches some source (our
//! embedded catalog, the official MCP Registry, later git marketplaces) and
//! returns normalized [`Candidate`]s that `add`/`search`/the dashboard render
//! into any CLI.

pub mod registry;

use indexmap::IndexMap;

use crate::catalog;
use crate::manifest::{Server, ServerType};

/// How to install a discovered capability.
#[derive(Debug, Clone)]
pub enum Install {
    Http {
        url: String,
        /// Header names that need a secret (e.g. `Authorization`).
        secret_headers: Vec<String>,
    },
    Stdio {
        command: String,
        args: Vec<String>,
        /// Env var names that need a secret.
        secret_env: Vec<String>,
    },
}

/// A normalized search result from any provider.
#[derive(Debug, Clone)]
pub struct Candidate {
    /// Full id (e.g. `io.github.user/server` or a catalog name).
    pub id: String,
    /// Clean, TOML-key-safe name to use in the manifest.
    pub name: String,
    pub description: String,
    /// Which provider surfaced it (`catalog`, `registry`, …).
    pub source: &'static str,
    pub install: Install,
}

/// Source-agnostic trust signals for a candidate (PLAN §9h). Not a verdict —
/// inputs a human (or `[policy]`) weighs before installing executable intent.
#[derive(Debug, Clone)]
pub struct Trust {
    /// Reverse-DNS namespace (registry rule: owner is verified for that name).
    pub namespaced: bool,
    /// stdio install runs a local command (e.g. `npx`) — higher risk than a
    /// remote HTTP endpoint.
    pub runs_code: bool,
    /// Requires a secret on this machine.
    pub needs_secret: bool,
}

impl Candidate {
    /// Compute trust signals from the candidate's id and install shape.
    pub fn trust(&self) -> Trust {
        let namespaced = self
            .id
            .split_once('/')
            .map(|(ns, _)| ns.contains('.'))
            .unwrap_or(false);
        match &self.install {
            Install::Http { secret_headers, .. } => Trust {
                namespaced,
                runs_code: false,
                needs_secret: !secret_headers.is_empty(),
            },
            Install::Stdio { secret_env, .. } => Trust {
                namespaced,
                runs_code: true,
                needs_secret: !secret_env.is_empty(),
            },
        }
    }

    /// Build a manifest [`Server`], lifting required secrets to `${REF}`s.
    pub fn to_server(&self) -> Server {
        match &self.install {
            Install::Http {
                url,
                secret_headers,
            } => {
                let mut headers = IndexMap::new();
                for h in secret_headers {
                    let reference = format!("{}_TOKEN", sanitize(&self.name));
                    let value = if h.eq_ignore_ascii_case("authorization") {
                        format!("Bearer ${{{reference}}}")
                    } else {
                        format!("${{{}}}", sanitize_env(h))
                    };
                    headers.insert(h.clone(), value);
                }
                Server {
                    server_type: ServerType::Http,
                    url: Some(url.clone()),
                    command: None,
                    args: vec![],
                    headers,
                    env: IndexMap::new(),
                }
            }
            Install::Stdio {
                command,
                args,
                secret_env,
            } => {
                let env = secret_env
                    .iter()
                    .map(|e| (e.clone(), format!("${{{e}}}")))
                    .collect();
                Server {
                    server_type: ServerType::Stdio,
                    url: None,
                    command: Some(command.clone()),
                    args: args.clone(),
                    headers: IndexMap::new(),
                    env,
                }
            }
        }
    }
}

/// Anything that can search a source for capabilities.
pub trait Provider {
    fn id(&self) -> &'static str;
    fn search(&self, query: &str, limit: usize) -> Vec<Candidate>;
}

/// Our embedded starter catalog as a provider.
pub struct CatalogProvider;

impl Provider for CatalogProvider {
    fn id(&self) -> &'static str {
        "catalog"
    }
    fn search(&self, query: &str, _limit: usize) -> Vec<Candidate> {
        catalog::search(query)
            .into_iter()
            .filter(|e| e.kind == "server")
            .map(|e| {
                let install = if e.transport.as_deref() == Some("http") {
                    Install::Http {
                        url: e.url.clone().unwrap_or_default(),
                        secret_headers: e.headers.clone(),
                    }
                } else {
                    Install::Stdio {
                        command: e.command.clone().unwrap_or_else(|| "npx".into()),
                        args: e.args.clone(),
                        secret_env: e.env.clone(),
                    }
                };
                Candidate {
                    id: e.name.clone(),
                    name: e.name.clone(),
                    description: e.description.clone(),
                    source: "catalog",
                    install,
                }
            })
            .collect()
    }
}

/// Search every enabled provider and return combined results (catalog first,
/// then network providers). De-duplicated by clean name.
pub fn search_all(query: &str, limit: usize) -> Vec<Candidate> {
    let providers: Vec<Box<dyn Provider>> = vec![
        Box::new(CatalogProvider),
        Box::new(registry::RegistryProvider::default()),
    ];
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for p in providers {
        for c in p.search(query, limit) {
            if seen.insert(c.name.clone()) {
                out.push(c);
            }
        }
    }
    out
}

/// Resolve an exact id/name to a single candidate across providers.
pub fn resolve(id: &str) -> Option<Candidate> {
    search_all(id, 30)
        .into_iter()
        .find(|c| c.id == id || c.name == id)
}

/// Uppercase, identifier-safe ref base from a name.
fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect::<String>()
        .to_ascii_uppercase()
}

fn sanitize_env(name: &str) -> String {
    sanitize(name)
}

/// A clean, TOML-key-safe manifest name from a (possibly reverse-DNS) id.
pub fn clean_name(id: &str) -> String {
    let last = id.rsplit('/').next().unwrap_or(id);
    let last = last.rsplit('.').next().unwrap_or(last);
    let cleaned: String = last
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    cleaned.trim_matches('-').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_provider_finds_servers() {
        let r = CatalogProvider.search("github", 10);
        assert!(r.iter().any(|c| c.name == "github"));
    }

    #[test]
    fn clean_name_strips_reverse_dns() {
        assert_eq!(
            clean_name("io.github.user/github-mcp-server"),
            "github-mcp-server"
        );
        assert_eq!(clean_name("com.example/my.server"), "server");
        assert_eq!(clean_name("plain"), "plain");
    }

    #[test]
    fn http_candidate_lifts_authorization() {
        let c = Candidate {
            id: "x".into(),
            name: "kibana".into(),
            description: "".into(),
            source: "catalog",
            install: Install::Http {
                url: "https://x".into(),
                secret_headers: vec!["Authorization".into()],
            },
        };
        let s = c.to_server();
        assert_eq!(s.headers["Authorization"], "Bearer ${KIBANA_TOKEN}");
    }
}
