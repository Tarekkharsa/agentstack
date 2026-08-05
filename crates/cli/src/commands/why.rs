//! `agentstack why <name>` — where one capability came from, and where it is now.
//!
//! Under the dynamic lane there is no file left on disk to open, so the
//! question "why can my agent do this, and who said yes?" has no other answer.
//! This command is that answer, and it is assembled entirely from layers that
//! already exist — it adds no store, no index, and no cache of its own:
//!
//! | row        | source                                                    |
//! |------------|-----------------------------------------------------------|
//! | `from`     | the manifest and the central library (via `crate::resolve`)|
//! | `pinned`   | `agentstack.lock`                                          |
//! | `approved` | this project's trust grant (`crate::trust`)                |
//! | `live`     | the planner **and** this harness's bridge state             |
//! | `written`  | what is on disk — the state ledger and the harness's files  |
//! | `scope`    | the declaration itself — hosts, secrets, commands          |
//! | `used`     | `~/.agentstack/usage.json` activation counts               |
//!
//! What it deliberately does NOT answer: a bare **tool** name. Mapping
//! `create_issue` back to its server needs a live `tools/list` from that
//! server, which is a connection, not a read — so `why` takes the name of a
//! capability and says so in its own help rather than guessing a mapping.

use std::path::Path;

use anyhow::Result;

use crate::cli::WhyArgs;
use crate::delivery::{Kind, Lane, Plan};
use crate::manifest::ServerType;
use crate::render::resolve_targets;
use crate::scope::Scope;
use crate::secret::refs_in;
use crate::state::{target_key, State};

pub fn run(args: &WhyArgs, manifest_dir: Option<&Path>) -> Result<()> {
    // Trust is keyed on the project BASE, which is NOT `ctx.dir` — the latter
    // is the manifest directory (`.agentstack/`). Reading the grant from
    // `ctx.dir` silently returns "never trusted" for every ordinary project,
    // which would put this row in direct contradiction with `doctor`, `status`
    // and `trust --preview`.
    let base = crate::commands::project_base(manifest_dir)?;
    let ctx = crate::commands::load(manifest_dir)?;
    let facts = Why::collect(&args.name, &base, &ctx)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&crate::ui_contract::envelope(facts.to_json()))?
        );
    } else {
        print!("{}", facts.render());
    }
    Ok(())
}

/// Everything `why` knows about one capability. Plain owned strings: the whole
/// point is that both renderings (text and `--json`) read the SAME collected
/// facts, so the two can never disagree about where something came from.
struct Why {
    name: String,
    /// The delivery planner's kind — this is what decides the lane rows, so it
    /// is stored rather than re-derived per row.
    kind: Kind,
    /// Singular noun for the header (`delivery::Kind::label` is plural).
    noun: &'static str,
    from: String,
    pinned: String,
    approved: String,
    /// One clause per harness the plan routes to the live lane, qualified with
    /// "planned live (not connected)" wherever that harness has no bridge —
    /// the same wording `delivery::harness_sentence` uses. Never a bare "live"
    /// for a harness nothing can actually reach (invariant 8).
    live: Vec<String>,
    /// Display names of live-routed harnesses with no bridge registered.
    live_unconnected: Vec<String>,
    /// One clause per harness that has this capability ON DISK — read from the
    /// state ledger and the harness's own files, never from the plan. A file
    /// the plan no longer renders is named as left over, which is the single
    /// most useful thing this row can say after a delivery change.
    written: Vec<String>,
    /// Display names where the file on disk is no longer part of the plan.
    abandoned: Vec<String>,
    scope: Vec<String>,
    activations: u64,
}

impl Why {
    fn collect(name: &str, base: &Path, ctx: &crate::commands::Context) -> Result<Why> {
        let manifest = &ctx.loaded.manifest;
        let library = crate::library::Library::load_default().unwrap_or_default();
        let lib_home = crate::util::paths::lib_home();
        let lock = crate::lock::Lock::load(&ctx.dir).unwrap_or_default();

        // Same resolution order as `explain`, so the two never disagree about
        // what a name IS: inline manifest first, then the central library.
        let (kind, noun, from, pinned, scope) =
            if manifest.servers.contains_key(name) || library.get_server(name).is_some() {
                let report =
                    crate::resolve::server_lock_status(name, manifest, &library, &lib_home, &lock);
                (
                    Kind::Server,
                    "MCP server",
                    server_from(&report),
                    pin_line(
                        lock.get_server(name).map(|e| e.checksum.hex()),
                        "agentstack lock --write",
                    ),
                    server_scope(name, manifest, &library, &lib_home),
                )
            } else if manifest.skills.contains_key(name) || library.get(name).is_some() {
                let report = crate::resolve::skill_lock_status(
                    name,
                    manifest,
                    &ctx.dir,
                    &library,
                    &lib_home,
                    &crate::store::Store::default_store(),
                    &lock,
                    crate::resolve::ResolveMode::NoFetch,
                );
                (
                    Kind::Skill,
                    "skill",
                    skill_from(&report),
                    pin_line(
                        lock.get(name).map(|e| e.checksum.hex()),
                        "agentstack lock --write",
                    ),
                    vec![
                        "its body enters the agent's context on demand".to_string(),
                        "agentstack runs nothing for it".to_string(),
                    ],
                )
            } else if let Some(instr) = manifest.instructions.get(name) {
                let origin = if instr.from_user_layer {
                    "the machine layer — merged beneath every project here".to_string()
                } else {
                    format!(
                        "this project's manifest · {}",
                        crate::instructions::declared_label(name, instr)
                    )
                };
                (
                    Kind::Instruction,
                    "house rule",
                    origin,
                    pin_line(
                        lock.get_instruction(name).map(|e| e.checksum.hex()),
                        "agentstack lock --write",
                    ),
                    vec!["its text is merged into each CLI's instructions file".to_string()],
                )
            } else if let Some(hook) = manifest.hooks.get(name) {
                (
                    Kind::Hook,
                    "hook",
                    "this project's manifest".to_string(),
                    // Honest negative: a hook's DECLARATION is manifest bytes, but
                    // the script it names is not digested. `explain` says the same;
                    // omitting it here would be the easiest place to miss it.
                    "the declaration only — a local script it names is not pinned".to_string(),
                    vec![
                        format!("runs `{}` on {}", hook.command, hook.event),
                        "at your full user permission — no policy ceiling observes it".to_string(),
                    ],
                )
            } else if manifest.extensions.contains_key(name) {
                (
                    Kind::Extension,
                    "native extension",
                    "this project's manifest".to_string(),
                    pin_line(
                        // Extension pins are a plain hex `String` in the lock, not
                        // the `Sha256Hex` newtype the other kinds use.
                        lock.get_extension(name).map(|e| e.checksum.as_str()),
                        "agentstack lock --write",
                    ),
                    vec![
                        "loads into the harness process at your full user permission".to_string(),
                        "governed before delivery, not at runtime".to_string(),
                    ],
                )
            } else if manifest.settings.contains_key(name) {
                (
                    Kind::Setting,
                    "setting",
                    "this project's manifest".to_string(),
                    "nothing to pin — the value IS the manifest bytes".to_string(),
                    vec!["a value written into each CLI's own settings file".to_string()],
                )
            } else {
                // The house error voice: name what was looked in, then the command
                // that lists what actually exists. Both named commands are visible
                // on `agentstack --help`.
                anyhow::bail!(
                    "nothing named '{name}' in this project's setup or the central library.\n\
                 `agentstack why` takes the name of a server, skill, house rule, hook, \
                 extension, or setting — not a tool name, which needs a live connection \
                 to resolve.\n\
                 Run `agentstack search {name}` to find one to add, or `agentstack lib list` \
                 to see the central library."
                );
            };

        let plan = Plan::build(
            &manifest.delivery,
            &ctx.registry,
            &resolve_targets(manifest, &ctx.registry, &[], &ctx.dir)?,
        );
        let routed = |h: &crate::delivery::HarnessPlan, lane: Lane| -> bool {
            h.routes.iter().any(|r| r.kind == kind && r.lane == lane)
        };

        // LIVE. The plan says where a kind is *routed*; only the bridge says
        // whether anything can arrive. Read per harness through the one shared
        // definition every other surface uses, so `why` cannot disagree with
        // `doctor`, `status` or `delivery` about the same harness.
        let mut live = Vec::new();
        let mut live_unconnected = Vec::new();
        for h in plan.harnesses.iter().filter(|h| routed(h, Lane::Dynamic)) {
            if super::overview::bridge_registered(&ctx.registry, &h.id) {
                live.push(h.display.clone());
            } else {
                live.push(format!("{} — planned live (not connected)", h.display));
                live_unconnected.push(h.display.clone());
            }
        }

        // WRITTEN. Disk, not plan. The state ledger is what `apply`, `doctor`
        // and `status` read for servers, skills, settings and hooks; house
        // rules leave no ledger entry, so their managed region is read from the
        // harness's own file exactly as `overview` reads it.
        let state = State::load().unwrap_or_default();
        let mut written = Vec::new();
        let mut abandoned = Vec::new();
        for h in &plan.harnesses {
            if !on_disk_for(&state, ctx, &h.id, kind, name) {
                continue;
            }
            if routed(h, Lane::Rendered) {
                written.push(h.display.clone());
            } else {
                // A file the current plan would not write: the delivery change
                // moved this kind live and left the render behind.
                // The remedy is the one every other surface names. It was
                // `agentstack apply --write`, which cannot remove an abandoned
                // render: under the live lane that command writes nothing,
                // exits 1 ("nothing was delivered"), and leaves the file
                // exactly where it was. Rule (e) wants one runnable answer.
                //
                // The WORDING splits on whether the ledger claims the file,
                // because the remedy does: `x unrender` removes only entries
                // AgentStack recorded, so promising it for a cloned or
                // checked-out config would be a claim it cannot keep. Both
                // clauses come from the shared `AbandonedRender`, never from a
                // second opinion held here.
                match abandoned_render(&state, ctx, &h.id, kind) {
                    Some(found) => {
                        written.push(format!(
                            "{} — {} ↳ {}",
                            h.display,
                            if found.recorded {
                                "left over from an earlier render"
                            } else {
                                "on disk, AgentStack did not write it"
                            },
                            found.remedy()
                        ));
                        abandoned.push(h.display.clone());
                    }
                    // For SERVERS, `None` is now a positive answer, not an
                    // absence: the shared detector looked and deliberately said
                    // nothing. Two cases reach here — the harness's own global
                    // config (the file `init` imports FROM), and the gateway's
                    // own bridge entry. Neither is "left over from an earlier
                    // render", and `x unrender` would refuse both, so `why`
                    // reports the file honestly and names no command. Keeping
                    // the old wording here would have re-introduced, on one
                    // surface, exactly the dead end the detector's global gate
                    // removes — and `why` must not disagree with `doctor` and
                    // `status` about the same file.
                    None if kind == Kind::Server => written.push(format!(
                        "{} — in its own config, which AgentStack does not manage",
                        h.display
                    )),
                    // Every other kind still has no shared whole-file reading;
                    // the ledger-shaped answer is all there is.
                    None => {
                        written.push(format!(
                            "{} — left over from an earlier render ↳ {}",
                            h.display,
                            crate::commands::apply::AbandonedRender::REMOVE_IT
                        ));
                        abandoned.push(h.display.clone());
                    }
                }
            }
        }

        Ok(Why {
            name: name.to_string(),
            kind,
            noun,
            from,
            pinned,
            approved: approved_line(base),
            live,
            live_unconnected,
            written,
            abandoned,
            scope,
            activations: crate::usage::Usage::load().unwrap_or_default().count(name),
        })
    }

    fn render(&self) -> String {
        let mut o = format!("\n  {}  ({})\n\n", self.name, self.noun);
        let mut row = |k: &str, v: &str| o.push_str(&format!("    {k:<10}{v}\n"));
        row("from", &self.from);
        row("pinned", &self.pinned);
        row("approved", &self.approved);
        row("live", &join_or_dash(&self.live));
        row("written", &join_or_dash(&self.written));
        row("scope", &join_or_dash(&self.scope));
        row(
            "used",
            &match self.activations {
                0 => "never activated from here yet".to_string(),
                1 => "activated once".to_string(),
                n => format!("activated {n} times"),
            },
        );
        o.push_str(&format!(
            "\n    full detail: agentstack explain {}\n\n",
            self.name
        ));
        o
    }

    fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "kind": self.kind.slug(),
            "noun": self.noun,
            "from": self.from,
            "pinned": self.pinned,
            "approved": self.approved,
            "live": self.live,
            "live_unconnected": self.live_unconnected,
            "written": self.written,
            "abandoned": self.abandoned,
            "scope": self.scope,
            "activations": self.activations,
        })
    }
}

/// The shared abandoned-file reading for `harness_id`, when this kind has one.
///
/// Servers are the only kind with a whole-file detector today; every other
/// kind keeps the ledger-shaped answer above. `None` means "no shared reading
/// applies", not "nothing is on disk" — the caller falls back to the generic
/// wording rather than inventing a second judgment.
fn abandoned_render(
    state: &State,
    ctx: &crate::commands::Context,
    harness_id: &str,
    kind: Kind,
) -> Option<crate::commands::apply::AbandonedRender> {
    if kind != Kind::Server {
        return None;
    }
    let desc = ctx.registry.get(harness_id)?;
    [Scope::Project, Scope::Global]
        .iter()
        .find_map(|scope| crate::commands::apply::abandoned_at(desc, *scope, &ctx.dir, state))
}

/// Is this one capability actually on disk for `harness_id`?
///
/// Both scopes are consulted because a project can be rendered globally
/// (`apply --scope global`) or locally, and `why` must name the file wherever
/// it really is. Servers come from the DISK reading every surface shares
/// ([`crate::commands::apply::servers_on_disk`]), because a server config can
/// exist with no ledger entry at all and the harness still reads it. Skills,
/// settings and hooks still come from the state ledger — the same record
/// `apply`, `doctor` and `diff` read. House rules leave no ledger entry;
/// their managed region is a marker in the harness's own instructions file.
///
/// Minimal, local implementation on purpose: the shared per-capability disk
/// reading does not exist yet (see the handoff note), and `why` must not grow a
/// second bridge or delivery reading while waiting for it.
fn on_disk_for(
    state: &State,
    ctx: &crate::commands::Context,
    harness_id: &str,
    kind: Kind,
    name: &str,
) -> bool {
    if kind == Kind::Instruction {
        let Some(spec) = ctx
            .registry
            .get(harness_id)
            .and_then(|d| d.instructions.as_ref())
        else {
            return false;
        };
        return [Scope::Project, Scope::Global].iter().any(|scope| {
            spec.path_for(*scope, &ctx.dir)
                .as_deref()
                .is_some_and(crate::render::instructions::manages_file)
        });
    }
    // Servers are read from DISK, never from the ledger. A config file can be
    // on disk without a ledger entry — a clone of a repo with `.mcp.json`
    // committed, a `git checkout`, an `x restore` — and in every one of those
    // cases the harness spawns those servers at startup. Answering from the
    // record would print `written: —` beside a file the harness is reading,
    // which is invariant 8. This calls the SAME reading `apply`, `doctor` and
    // `status` use (`apply::servers_on_disk`), so `why` cannot disagree with
    // them; it does not add a second one.
    if kind == Kind::Server {
        let Some(desc) = ctx.registry.get(harness_id) else {
            return false;
        };
        return [Scope::Project, Scope::Global].iter().any(|scope| {
            crate::commands::apply::servers_on_disk(desc, *scope, &ctx.dir)
                .is_some_and(|(_, servers)| servers.iter().any(|n| n == name))
        });
    }
    [Scope::Project, Scope::Global].iter().any(|scope| {
        let key = target_key(harness_id, *scope, &ctx.dir);
        let recorded = match kind {
            // Handled above, from disk.
            Kind::Server => Vec::new(),
            Kind::Skill => state.managed_skills(&key),
            Kind::Setting => state.managed_settings(&key),
            Kind::Hook => state.managed_hooks(&key),
            // Extensions are not tracked in the ledger; nothing on disk can be
            // claimed for them, and claiming one would be invariant 8 again.
            Kind::Extension | Kind::Instruction => Vec::new(),
        };
        recorded.iter().any(|n| n == name)
    })
}

/// `sha256:1f4a…` for a pinned digest, or the honest negative plus the command
/// that pins it. `fix` is a runnable, discoverable command by construction.
fn pin_line(hex: Option<&str>, fix: &str) -> String {
    match hex {
        Some(h) => format!("sha256:{}…", &h[..h.len().min(12)]),
        None => format!("not pinned yet ↳ {fix}"),
    }
}

fn server_from(report: &crate::resolve::ServerLockReport) -> String {
    use crate::resolve::ServerOrigin as O;
    let origin = match report.origin {
        Some(O::Inline) => "this project's manifest",
        Some(O::Library) => "the central library",
        Some(O::Package) => "a package a toolset selected",
        None => "unresolved",
    };
    match &report.provenance {
        Some(p) => format!("{origin} · {p}"),
        None => origin.to_string(),
    }
}

fn skill_from(report: &crate::resolve::SkillLockReport) -> String {
    use crate::resolve::SkillOrigin as O;
    let origin = match report.origin {
        Some(O::Inline) => "this project's manifest",
        Some(O::Library) => "the central library",
        None => "unresolved",
    };
    match &report.provenance {
        Some(p) => format!("{origin} · {p}"),
        None => origin.to_string(),
    }
}

/// What a server reaches: the host it contacts or the command it runs, plus
/// every `${REF}` it reads. Read off the resolved definition, never guessed.
fn server_scope(
    name: &str,
    manifest: &crate::manifest::Manifest,
    library: &crate::library::Library,
    lib_home: &Path,
) -> Vec<String> {
    let Ok(resolved) = crate::resolve::resolve_server(manifest, library, lib_home, name) else {
        return vec!["unresolved — nothing can be said about what it reaches".to_string()];
    };
    let server = resolved.server;
    let mut out = Vec::new();
    match server.server_type {
        ServerType::Http => {
            if let Some(url) = &server.url {
                out.push(format!("contacts {}", host_of(url)));
            }
        }
        ServerType::Stdio => {
            if let Some(cmd) = &server.command {
                out.push(format!("runs `{cmd}`"));
            }
        }
    }
    let mut refs: Vec<String> = Vec::new();
    if let Some(u) = &server.url {
        refs.extend(refs_in(u));
    }
    for v in server.headers.values().chain(server.env.values()) {
        refs.extend(refs_in(v));
    }
    refs.sort();
    refs.dedup();
    if !refs.is_empty() {
        out.push(format!("reads {}", refs.join(" · ")));
    }
    if out.is_empty() {
        out.push("nothing declared".to_string());
    }
    out
}

/// The trust grant, in one clause. `trust --preview`, `doctor` and `status`
/// all read the same [`crate::trust::check`], so this row cannot contradict
/// them: it renders that one verdict rather than a second opinion.
fn approved_line(base: &Path) -> String {
    match crate::trust::check(base) {
        crate::trust::TrustState::Trusted => {
            let at = crate::trust::TrustStore::load()
                .trusted
                .get(&crate::trust::key_for(base))
                .map(|e| e.trusted_at);
            match at {
                Some(t) => format!("yes · you said yes {}", age(t)),
                None => "yes".to_string(),
            }
        }
        crate::trust::TrustState::Changed => {
            "⚠ the project changed since your last yes ↳ agentstack trust .".to_string()
        }
        crate::trust::TrustState::Untrusted => "not yet ↳ agentstack trust .".to_string(),
    }
}

fn age(time_unix: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let s = now.saturating_sub(time_unix);
    match s {
        0..=59 => format!("{s}s ago"),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86_399 => format!("{}h ago", s / 3600),
        _ => format!("{}d ago", s / 86_400),
    }
}

fn join_or_dash(items: &[String]) -> String {
    if items.is_empty() {
        "—".to_string()
    } else {
        items.join(" · ")
    }
}

/// Host portion of a URL — the glanceable "where does this connect" signal.
fn host_of(url: &str) -> String {
    let after = url.split_once("://").map(|(_, r)| r).unwrap_or(url);
    after.split(['/', '?']).next().unwrap_or(after).to_string()
}
