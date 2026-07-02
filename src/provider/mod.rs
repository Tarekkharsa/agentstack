//! Capability **providers** — the meta-layer (PLAN §9h). agentstack doesn't run
//! a registry; it *consumes* them. A `Provider` searches some source (our
//! embedded catalog, the official MCP Registry, later git marketplaces) and
//! returns normalized [`Candidate`]s that `add`/`search`/the dashboard render
//! into any CLI.

pub mod gitpack;
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

/// A bundled-skill reference inside a candidate (a pack member, or a standalone
/// `kind: skill`). `path` is an embedded asset path under `catalog/` (or a local
/// path for user skills); `git`/`rev` describe a remote source.
#[derive(Debug, Clone)]
pub struct SkillRef {
    pub name: String,
    pub path: Option<String>,
    pub git: Option<String>,
    pub rev: Option<String>,
}

/// An instruction-fragment reference inside a pack. `path` is an embedded asset
/// path under `catalog/`.
#[derive(Debug, Clone)]
pub struct InstrRef {
    pub name: String,
    pub path: String,
}

/// A vendor pack: an install-time composition of an MCP server + skill(s) +
/// house-rule instructions. After `add` each member lives in its normal
/// manifest section; this is NOT a runtime concept.
#[derive(Debug, Clone)]
pub struct PackSpec {
    pub server: Option<Install>,
    pub skills: Vec<SkillRef>,
    pub instructions: Vec<InstrRef>,
    pub targets: Vec<String>,
}

/// What kind of capability a candidate installs.
#[derive(Debug, Clone)]
pub enum CandidateKind {
    /// A single MCP server.
    Server(Install),
    /// A standalone skill (a `SKILL.md` directory).
    Skill(SkillRef),
    /// A vendor pack (server + skills + instructions installed as one unit).
    Pack(PackSpec),
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
    pub kind: CandidateKind,
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

impl Install {
    /// Trust signals for a single install shape, given a precomputed
    /// `namespaced` flag.
    fn trust(&self, namespaced: bool) -> Trust {
        match self {
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
}

impl Candidate {
    /// Compute trust signals from the candidate's id and shape. A pack
    /// aggregates its members: it runs code if any member would, and needs a
    /// secret if its server does.
    pub fn trust(&self) -> Trust {
        let namespaced = self
            .id
            .split_once('/')
            .map(|(ns, _)| ns.contains('.'))
            .unwrap_or(false);
        match &self.kind {
            CandidateKind::Server(install) => install.trust(namespaced),
            CandidateKind::Skill(_) => Trust {
                namespaced,
                // Skills carry no executable transport; they steer the agent.
                runs_code: false,
                needs_secret: false,
            },
            CandidateKind::Pack(spec) => {
                let server = spec.server.as_ref().map(|i| i.trust(namespaced));
                Trust {
                    namespaced,
                    runs_code: server.as_ref().map(|t| t.runs_code).unwrap_or(false),
                    needs_secret: server.as_ref().map(|t| t.needs_secret).unwrap_or(false),
                }
            }
        }
    }

    /// Build a manifest [`Server`], lifting required secrets to `${REF}`s. Only
    /// valid for server candidates and packs that carry a server.
    pub fn to_server(&self) -> Server {
        let install = match &self.kind {
            CandidateKind::Server(install) => install,
            CandidateKind::Pack(spec) => spec
                .server
                .as_ref()
                .expect("to_server called on a pack with no server"),
            CandidateKind::Skill(_) => panic!("to_server called on a skill candidate"),
        };
        install.to_server_named(&self.name)
    }
}

impl Install {
    /// Build a manifest [`Server`] for this install, lifting required secrets to
    /// `${REF}`s keyed off `name`.
    fn to_server_named(&self, name: &str) -> Server {
        match self {
            Install::Http {
                url,
                secret_headers,
            } => {
                let mut headers = IndexMap::new();
                for h in secret_headers {
                    let reference = format!("{}_TOKEN", sanitize(name));
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
            .filter_map(|e| {
                let kind = match e.kind.as_str() {
                    "server" => CandidateKind::Server(server_install(
                        e.transport.as_deref(),
                        e.url.as_deref(),
                        e.command.as_deref(),
                        &e.args,
                        &e.env,
                        &e.headers,
                    )),
                    "skill" => CandidateKind::Skill(SkillRef {
                        name: e.name.clone(),
                        path: e.path.clone(),
                        git: None,
                        rev: None,
                    }),
                    "pack" => CandidateKind::Pack(PackSpec {
                        server: e.server.as_ref().map(|s| {
                            server_install(
                                s.transport.as_deref(),
                                s.url.as_deref(),
                                s.command.as_deref(),
                                &s.args,
                                &s.env,
                                &s.headers,
                            )
                        }),
                        skills: e
                            .skills
                            .iter()
                            .map(|s| SkillRef {
                                name: s.name.clone(),
                                path: s.path.clone(),
                                git: s.git.clone(),
                                rev: s.rev.clone(),
                            })
                            .collect(),
                        instructions: e
                            .instructions
                            .iter()
                            .map(|i| InstrRef {
                                name: i.name.clone(),
                                path: i.path.clone(),
                            })
                            .collect(),
                        targets: if e.targets.is_empty() {
                            vec!["*".to_string()]
                        } else {
                            e.targets.clone()
                        },
                    }),
                    _ => return None,
                };
                Some(Candidate {
                    id: e.name.clone(),
                    name: e.name.clone(),
                    description: e.description.clone(),
                    source: "catalog",
                    kind,
                })
            })
            .collect()
    }
}

/// Build an [`Install`] from flat catalog server fields (shared by `kind:
/// server` entries and the nested `server:` of a pack).
fn server_install(
    transport: Option<&str>,
    url: Option<&str>,
    command: Option<&str>,
    args: &[String],
    env: &[String],
    headers: &[String],
) -> Install {
    if transport == Some("http") {
        Install::Http {
            url: url.unwrap_or_default().to_string(),
            secret_headers: headers.to_vec(),
        }
    } else {
        Install::Stdio {
            command: command.unwrap_or("npx").to_string(),
            args: args.to_vec(),
            secret_env: env.to_vec(),
        }
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
            kind: CandidateKind::Server(Install::Http {
                url: "https://x".into(),
                secret_headers: vec!["Authorization".into()],
            }),
        };
        let s = c.to_server();
        assert_eq!(s.headers["Authorization"], "Bearer ${KIBANA_TOKEN}");
    }

    #[test]
    fn catalog_provider_surfaces_pack_and_skill() {
        let pack = CatalogProvider
            .search("linear", 10)
            .into_iter()
            .find(|c| c.name == "linear-pack")
            .unwrap();
        match &pack.kind {
            CandidateKind::Pack(spec) => {
                assert!(spec.server.is_some());
                assert_eq!(spec.skills.len(), 1);
                assert_eq!(spec.instructions.len(), 1);
            }
            _ => panic!("expected a pack"),
        }
        // A pack with an http server that needs Authorization: needs a secret,
        // runs no code.
        let t = pack.trust();
        assert!(t.needs_secret);
        assert!(!t.runs_code);

        let skill = CatalogProvider
            .search("triage", 10)
            .into_iter()
            .find(|c| c.name == "pr-triage")
            .unwrap();
        assert!(matches!(skill.kind, CandidateKind::Skill(_)));
    }
}
