//! Capability **providers** — the meta-layer (PLAN §9h). agentstack doesn't run
//! a registry; it *consumes* them. A `Provider` searches some source (our
//! embedded catalog, the official MCP Registry, later git marketplaces) and
//! returns normalized [`Candidate`]s that `add`/`search`/the dashboard render
//! into any CLI.

pub mod discover;
pub mod gitpack;
pub mod registry;
pub mod source;

use indexmap::IndexMap;

use crate::catalog;
use crate::manifest::{Hook, Server, ServerType};

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

/// A native-extension reference inside a candidate. Extensions are harness-
/// specific executable add-ons; a library extension carries the one adapter it
/// `target`s. Discovery-only today — surfaced by `search`, not installed by
/// `add` (extensions are referenced by name in `[extensions.*]`).
#[derive(Debug, Clone)]
pub struct ExtensionRef {
    pub name: String,
    /// The one adapter id the extension's code is written against.
    pub target: String,
}

/// A declarative-hook reference inside a candidate. A hook is a flat definition
/// (event/command/args/…), so — unlike a server's `Install` shape — the candidate
/// carries the whole `Hook`, which `add` copies straight into `[hooks.<name>]`.
#[derive(Debug, Clone)]
pub struct HookRef {
    pub name: String,
    pub hook: Hook,
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
    /// A native harness extension in the central library — executable in-process
    /// code, discovery-only (referenced by name in `[extensions.*]`, not added
    /// through `add from`).
    Extension(ExtensionRef),
    /// A declarative lifecycle hook in the central library. `add from` copies the
    /// definition into the project's inline `[hooks.<name>]` table.
    Hook(HookRef),
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
            // Extensions are the strongest "runs code" signal of any kind: their
            // bytes execute in-process at full user permission, ungoverned at
            // runtime (design doc §7).
            CandidateKind::Extension(_) => Trust {
                namespaced,
                runs_code: true,
                needs_secret: false,
            },
            // A hook runs a command on a harness lifecycle event — always code.
            // It needs a secret when the command or an arg carries a `${REF}`.
            CandidateKind::Hook(h) => Trust {
                namespaced,
                runs_code: true,
                needs_secret: h.hook.command.contains("${")
                    || h.hook.args.iter().any(|a| a.contains("${")),
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
            CandidateKind::Extension(_) => panic!("to_server called on an extension candidate"),
            CandidateKind::Hook(_) => panic!("to_server called on a hook candidate"),
        };
        install.to_server_named(&self.name)
    }

    /// The manifest [`Hook`] this candidate installs. Only valid for hook
    /// candidates — `add from` writes it verbatim into `[hooks.<name>]`.
    pub fn to_hook(&self) -> Hook {
        match &self.kind {
            CandidateKind::Hook(h) => h.hook.clone(),
            _ => panic!("to_hook called on a non-hook candidate"),
        }
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
                    cwd: None,
                    integrity_roots: Vec::new(),
                    targets: crate::manifest::model::all_targets(),
                    owner: None,
                    headers,
                    env: IndexMap::new(),
                    extra: IndexMap::new(),
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
                    cwd: None,
                    integrity_roots: Vec::new(),
                    targets: crate::manifest::model::all_targets(),
                    owner: None,
                    headers: IndexMap::new(),
                    env,
                    extra: IndexMap::new(),
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
            .map(|e| {
                let kind = match e.kind {
                    crate::catalog::CatalogKind::Server => CandidateKind::Server(server_install(
                        e.transport.as_deref(),
                        e.url.as_deref(),
                        e.command.as_deref(),
                        &e.args,
                        &e.env,
                        &e.headers,
                    )),
                    crate::catalog::CatalogKind::Skill => CandidateKind::Skill(SkillRef {
                        name: e.name.clone(),
                        path: e.path.clone(),
                        git: None,
                        rev: None,
                    }),
                    crate::catalog::CatalogKind::Pack => CandidateKind::Pack(PackSpec {
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
                };
                Candidate {
                    id: e.name.clone(),
                    name: e.name.clone(),
                    description: e.description.clone(),
                    source: "catalog",
                    kind,
                }
            })
            .collect()
    }
}

/// The user's own central library (`~/.agentstack/lib`) as a provider. These are
/// the capabilities the maintainer already curated, so a hit here is the most
/// relevant result — `search_all` lists this provider first, and its clean name
/// wins de-dup against catalog/registry hits of the same name.
pub struct LibraryProvider {
    library: crate::library::Library,
    lib_home: std::path::PathBuf,
}

impl Default for LibraryProvider {
    fn default() -> Self {
        // A fresh machine has no library file; `load_default_or_warn` returns an
        // empty library there, so search simply finds no library hits (no error).
        LibraryProvider {
            library: crate::library::Library::load_default_or_warn(),
            lib_home: crate::util::paths::lib_home(),
        }
    }
}

impl Provider for LibraryProvider {
    fn id(&self) -> &'static str {
        "library"
    }
    fn search(&self, query: &str, _limit: usize) -> Vec<Candidate> {
        let q = query.to_ascii_lowercase();
        let mut out = Vec::new();
        // Skills: match the reference name OR the `SKILL.md` frontmatter
        // description. An empty query lists everything (like the catalog).
        for entry in &self.library.skills {
            let desc = entry.description(&self.lib_home);
            let name_hit = entry.name.to_ascii_lowercase().contains(&q);
            let desc_hit = desc
                .as_deref()
                .is_some_and(|d| d.to_ascii_lowercase().contains(&q));
            if q.is_empty() || name_hit || desc_hit {
                out.push(Candidate {
                    id: entry.name.clone(),
                    name: entry.name.clone(),
                    description: desc.unwrap_or_default(),
                    source: "library",
                    kind: CandidateKind::Skill(SkillRef {
                        name: entry.name.clone(),
                        path: entry.path.clone(),
                        git: entry.git.clone(),
                        rev: entry.rev.clone(),
                    }),
                });
            }
        }
        // Servers: match the reference name. The library index carries no
        // description, so the install shape (loaded from `servers/<name>.toml`)
        // is what we display; a definition that won't load is skipped.
        for entry in &self.library.servers {
            if q.is_empty() || entry.name.to_ascii_lowercase().contains(&q) {
                if let Some(install) = self.load_server_install(&entry.name) {
                    out.push(Candidate {
                        id: entry.name.clone(),
                        name: entry.name.clone(),
                        description: String::new(),
                        source: "library",
                        kind: CandidateKind::Server(install),
                    });
                }
            }
        }
        // Extensions: match name OR the stored one-line description (extensions
        // carry no `SKILL.md`, so the index field is the description), exactly
        // like skills.
        for entry in &self.library.extensions {
            let desc = entry.description.clone();
            let name_hit = entry.name.to_ascii_lowercase().contains(&q);
            let desc_hit = desc
                .as_deref()
                .is_some_and(|d| d.to_ascii_lowercase().contains(&q));
            if q.is_empty() || name_hit || desc_hit {
                out.push(Candidate {
                    id: entry.name.clone(),
                    name: entry.name.clone(),
                    description: desc.unwrap_or_default(),
                    source: "library",
                    kind: CandidateKind::Extension(ExtensionRef {
                        name: entry.name.clone(),
                        target: entry.target.clone(),
                    }),
                });
            }
        }
        // Hooks: match the reference name (the index carries no description, like
        // servers). A definition that won't load is skipped rather than failing
        // the whole search.
        for entry in &self.library.hooks {
            if q.is_empty() || entry.name.to_ascii_lowercase().contains(&q) {
                if let Some(hook) = self.load_hook_def(&entry.name) {
                    out.push(Candidate {
                        id: entry.name.clone(),
                        name: entry.name.clone(),
                        description: String::new(),
                        source: "library",
                        kind: CandidateKind::Hook(HookRef {
                            name: entry.name.clone(),
                            hook,
                        }),
                    });
                }
            }
        }
        out
    }
}

impl LibraryProvider {
    /// Load a central-library hook definition (`<lib_home>/hooks/<name>.toml`).
    /// Best-effort: an unreadable or invalid definition yields `None`.
    fn load_hook_def(&self, name: &str) -> Option<Hook> {
        let path = self.lib_home.join("hooks").join(format!("{name}.toml"));
        let text = std::fs::read_to_string(path).ok()?;
        toml::from_str(&text).ok()
    }

    /// Load a central-library server definition (`<lib_home>/servers/<name>.toml`)
    /// and map it to an [`Install`] for display. Best-effort: an unreadable or
    /// invalid definition yields `None`, so the server is omitted rather than
    /// failing the whole search.
    fn load_server_install(&self, name: &str) -> Option<Install> {
        let path = self.lib_home.join("servers").join(format!("{name}.toml"));
        let text = std::fs::read_to_string(path).ok()?;
        let server: Server = toml::from_str(&text).ok()?;
        Some(install_from_server(&server))
    }
}

/// Map a stored [`Server`] definition back to an [`Install`] for discovery
/// display. A header/env is treated as secret-bearing when its value carries a
/// `${REF}` placeholder — the only way secrets are ever written to a definition.
fn install_from_server(server: &Server) -> Install {
    let secret_refs = |map: &IndexMap<String, String>| -> Vec<String> {
        map.iter()
            .filter(|(_, v)| v.contains("${"))
            .map(|(k, _)| k.clone())
            .collect()
    };
    match server.server_type {
        ServerType::Http => Install::Http {
            url: server.url.clone().unwrap_or_default(),
            secret_headers: secret_refs(&server.headers),
        },
        ServerType::Stdio => Install::Stdio {
            command: server.command.clone().unwrap_or_default(),
            args: server.args.clone(),
            secret_env: secret_refs(&server.env),
        },
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

/// Search every enabled provider and return combined results. The user's own
/// central library ranks first (their curated capabilities are the most
/// relevant), then the embedded catalog, then network providers.
/// De-duplicated by clean name — a library hit shadows a same-named catalog or
/// registry hit.
pub fn search_all(query: &str, limit: usize) -> Vec<Candidate> {
    let providers: Vec<Box<dyn Provider>> = vec![
        Box::new(LibraryProvider::default()),
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
    fn library_provider_matches_name_and_description() {
        use crate::library::{Library, LibrarySkill};

        // Seed a temp library: one path skill whose SKILL.md description carries
        // a unique word, and whose body lives at `<lib_home>/skills/<path>/`.
        let dir = assert_fs::TempDir::new().unwrap();
        let body = dir.path().join("skills/quokka-lint");
        std::fs::create_dir_all(&body).unwrap();
        std::fs::write(
            body.join("SKILL.md"),
            "---\nname: quokka-lint\ndescription: Guards against zzquokkaword drift.\n---\nbody\n",
        )
        .unwrap();

        let mut library = Library::default();
        library.upsert(LibrarySkill {
            name: "quokka-lint".into(),
            source: "path".into(),
            path: Some("quokka-lint".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: Some("manual".into()),
        });
        let provider = LibraryProvider {
            library,
            lib_home: dir.path().to_path_buf(),
        };

        // Found by exact name, labeled `[library]` (via `source`).
        let by_name = provider.search("quokka-lint", 10);
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].source, "library");
        assert_eq!(by_name[0].name, "quokka-lint");

        // Found by a unique word in its SKILL.md description.
        let by_desc = provider.search("zzquokkaword", 10);
        assert_eq!(by_desc.len(), 1);
        assert_eq!(by_desc[0].name, "quokka-lint");
        assert_eq!(by_desc[0].source, "library");

        // No spurious hits.
        assert!(provider.search("no-such-token", 10).is_empty());
    }

    #[test]
    fn library_provider_surfaces_extensions_by_name_and_description() {
        use crate::library::{Library, LibraryExtension};

        let mut library = Library::default();
        library.upsert_extension(LibraryExtension {
            name: "checkpoint".into(),
            source: "path".into(),
            target: "pi".into(),
            path: Some("checkpoint".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            // A unique word only in the stored description.
            description: Some("Guards against zzquokkaword drift each turn.".into()),
            version: None,
            provenance: Some("manual".into()),
        });
        let provider = LibraryProvider {
            library,
            lib_home: std::path::PathBuf::from("/does/not/matter"),
        };

        // By name, as an Extension candidate carrying its target.
        let by_name = provider.search("checkpoint", 10);
        assert_eq!(by_name.len(), 1);
        assert_eq!(by_name[0].source, "library");
        match &by_name[0].kind {
            CandidateKind::Extension(ext) => assert_eq!(ext.target, "pi"),
            other => panic!("expected an extension candidate, got {other:?}"),
        }
        // Extensions run code: the trust signal must say so.
        assert!(by_name[0].trust().runs_code);

        // By a unique word in the stored description, and no spurious hits.
        assert_eq!(provider.search("zzquokkaword", 10).len(), 1);
        assert!(provider.search("no-such-token", 10).is_empty());
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
