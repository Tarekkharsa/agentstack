//! `agentstack trust` — the human gate every activation path checks.
//!
//! `connect` registers one global gateway per harness; `mcp --auto-project`
//! then discovers whatever manifest the current repo carries. This command is
//! what stands between "cloned a repo" and "that repo's manifest spawns stdio
//! servers and receives secrets": trust is granted per project, pinned to the
//! manifest's content digest, and shown to the human as the list of things the
//! manifest would actually run.

use std::collections::{HashMap, HashSet};
use std::io::IsTerminal;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::TrustArgs;
use crate::manifest::ServerType;
use crate::trust::{self, PriorSurface, SurfaceItem, TrustState, TrustStore};

/// Threads the P14 re-trust diff through the consent review. In diff mode it
/// holds the last consented surface keyed by `(kind, name)`; [`mark`] returns
/// the two-char marker to print before each item's line — `"+ "` added,
/// `"~ "` changed, `"  "` unchanged — and remembers which prior items it saw so
/// [`removed`] can report the rest as `- removed`. In flat mode (`prior` is
/// `None`: first-ever trust, or an older entry with no snapshot) every marker is
/// the plain two-space indent, so the review reads exactly as it did before
/// P14. Either way it accumulates the CURRENT surface, which the caller then
/// persists so the *next* re-trust has something to diff against.
///
/// [`mark`]: ReviewDiff::mark
/// [`removed`]: ReviewDiff::removed
struct ReviewDiff {
    /// `(kind, name) -> identity` from the last consented surface, or `None` in
    /// flat mode.
    prior: Option<HashMap<(String, String), String>>,
    /// The prior surface in its recorded order, for a stable `removed` pass.
    prior_order: Vec<SurfaceItem>,
    seen: HashSet<(String, String)>,
    /// The surface being reviewed now — handed to `trust_with_snapshot`.
    current: Vec<SurfaceItem>,
}

impl ReviewDiff {
    fn new(prior: PriorSurface) -> Self {
        // Only a recorded prior turns on diff markers; NeverTrusted and
        // Untracked both render flat.
        let (map, order) = match prior {
            PriorSurface::Recorded(items) => {
                let map = items
                    .iter()
                    .map(|it| ((it.kind.clone(), it.name.clone()), it.identity.clone()))
                    .collect();
                (Some(map), items)
            }
            _ => (None, Vec::new()),
        };
        Self {
            prior: map,
            prior_order: order,
            seen: HashSet::new(),
            current: Vec::new(),
        }
    }

    fn diffing(&self) -> bool {
        self.prior.is_some()
    }

    /// Record a reviewed item and return its two-char line marker. Called
    /// exactly once per item, in render order.
    fn mark(&mut self, kind: &str, name: &str, identity: &str) -> &'static str {
        self.mark_pinned(kind, name, identity, None)
    }

    /// `mark`, additionally recording the content digest this item is pinned
    /// to. Only the kinds whose bytes live outside the manifest carry one —
    /// skills and instructions — because they are the kinds a re-gate has to
    /// diff. The pin never affects the marker: it is not part of the diff key,
    /// so a re-lock that changes only the pin is not a `~ changed` surface.
    fn mark_pinned(
        &mut self,
        kind: &str,
        name: &str,
        identity: &str,
        pin: Option<String>,
    ) -> &'static str {
        self.current.push(SurfaceItem {
            kind: kind.to_string(),
            name: name.to_string(),
            identity: identity.to_string(),
            pin,
        });
        let Some(prior) = &self.prior else {
            return "  ";
        };
        let key = (kind.to_string(), name.to_string());
        self.seen.insert(key.clone());
        match prior.get(&key) {
            None => "+ ",
            Some(prev) if prev != identity => "~ ",
            Some(_) => "  ",
        }
    }

    /// Prior items no marker was requested for — removed since the last trust.
    /// Empty in flat mode (`prior_order` is empty there).
    fn removed(&self) -> Vec<&SurfaceItem> {
        self.prior_order
            .iter()
            .filter(|it| !self.seen.contains(&(it.kind.clone(), it.name.clone())))
            .collect()
    }
}

pub fn run(args: &TrustArgs) -> Result<()> {
    if args.list {
        return list();
    }
    let base = resolve_base(args.path.as_deref())?;
    if args.preview {
        return preview(&base);
    }
    if args.revoke {
        return revoke(&base);
    }
    grant(&base, args.yes, args.consented_digest.as_deref())
}

/// Read-only: emit the runtime surface a human would consent to, as JSON,
/// granting nothing. This is the summary an external UI (the t3code trust
/// dialog) shows before the user consents; the AUTHORITATIVE line-by-line
/// review and the consent gate stay in `grant_gated`, and the grant itself
/// (`trust --yes`) still self-gates on an unpinned surface — so this preview
/// deliberately shows the surface + category counts, not a re-derived blocker
/// verdict. Nothing here writes or fetches.
fn preview(base: &Path) -> Result<()> {
    let out = preview_value(base)?;
    println!("{}", serde_json::to_string_pretty(&out)?);
    Ok(())
}

/// Build the same read-only, enveloped trust preview emitted by
/// `trust --preview`.
pub fn preview_value(base: &Path) -> Result<serde_json::Value> {
    let dir = crate::manifest::resolve_manifest_dir(base);
    // §7.2: ONE immutable read of the consent surface. The parsed display and
    // the digest below both derive from this snapshot, so an edit landing
    // mid-preview can never pair one file state's display with another's
    // digest — whatever the interleaving (including A→B→A), display and
    // digest describe the same bytes.
    let Some(snapshot) = trust::ConsentSnapshot::read(base) else {
        // No readable manifest: surface the same friendly first-contact error
        // the disk load path gives.
        crate::manifest::load_from_dir(&dir)?;
        anyhow::bail!("manifest disappeared while previewing {}", base.display());
    };
    let loaded = load_snapshot_manifest(&snapshot, &dir)?;
    let m = &loaded.manifest;
    // The lock pins are part of the consented surface: parse them from the
    // SAME snapshot bytes the digest covers, never a second disk read.
    let lock = lock_from_snapshot(&snapshot, &dir)?;

    // State is judged against the SNAPSHOT digest, not a fresh disk read, so
    // the state chip describes the same bytes as the display and the digest.
    let surface_digest = snapshot.digest();
    let state = match trust::check_digest(base, Some(&surface_digest)) {
        trust::TrustState::Trusted => "trusted",
        trust::TrustState::Changed => "drifted",
        trust::TrustState::Untrusted => "untrusted",
    };
    let re_trust = !matches!(
        trust::prior_surface(base),
        trust::PriorSurface::NeverTrusted
    );

    // The gateway's actual runtime surface — library refs resolve exactly as
    // they will at gateway time. Display strings are sanitized (hostile input).
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    let effective_servers = crate::resolve::effective_runtime_servers(m, &library, &lib_home, None);
    let mut server_blockers: Vec<serde_json::Value> = Vec::new();
    let servers: Vec<serde_json::Value> = effective_servers
            .iter()
            .map(|(name, resolved)| match resolved {
                Ok(r) => {
                    // A library-backed definition resolves from the LIVE
                    // central library, but the digest binds only the lock
                    // pin. Displaying a definition that doesn't match the
                    // pin would show the consenting human content the digest
                    // does not cover (an external UI would then bind consent
                    // to bytes nobody is granting) — so an unpinned or
                    // drifted library server renders as unverified instead
                    // of leaking the live definition into the surface.
                    let pinned_ok = match r.origin {
                        crate::resolve::ServerOrigin::Inline => true,
                        crate::resolve::ServerOrigin::Library => lock
                            .get_server(name)
                            .is_some_and(|entry| entry.checksum.hex() == r.checksum),
                    };
                    if !pinned_ok {
                        server_blockers.push(serde_json::json!({
                            "name": crate::text::sanitize_line(name),
                            "reason": "library definition does not match the lockfile pin",
                            "fix": "agentstack lock",
                        }));
                        return serde_json::json!({
                            "name": crate::text::sanitize_line(name),
                            "kind": "unverified",
                            "target": "library definition does not match the lockfile pin — run `agentstack lock`, review the change, and re-run the preview",
                        });
                    }
                    let (kind, target) = match r.server.server_type {
                        crate::manifest::ServerType::Stdio => (
                            "stdio",
                            format!(
                                "{} {}",
                                r.server.command.as_deref().unwrap_or("?"),
                                r.server.args.join(" ")
                            )
                            .trim()
                            .to_string(),
                        ),
                        crate::manifest::ServerType::Http => {
                            ("http", r.server.url.clone().unwrap_or_default())
                        }
                    };
                    serde_json::json!({
                        "name": crate::text::sanitize_line(name),
                        "kind": kind,
                        "target": crate::text::sanitize_line(&target),
                    })
                }
                Err(e) => {
                    server_blockers.push(serde_json::json!({
                        "name": crate::text::sanitize_line(name),
                        "reason": crate::text::sanitize_line(&e.to_string()),
                        "fix": "edit-manifest",
                    }));
                    serde_json::json!({
                        "name": crate::text::sanitize_line(name),
                        "kind": "unresolvable",
                        "target": crate::text::sanitize_line(&e.to_string()),
                    })
                }
            })
            .collect();

    // The trust grant also verifies repository-local executable content. Carry
    // server-specific failures in the machine preview so an external consent
    // screen can disable a grant that is known to fail and point at the exact
    // declaration. This is read-only and uses the same resolver/verdict path as
    // the authoritative grant review below.
    let executable_servers: Vec<(String, crate::manifest::Server)> = effective_servers
        .iter()
        .filter_map(|(name, resolved)| {
            resolved
                .as_ref()
                .ok()
                .map(|resolved| (name.clone(), resolved.server.clone()))
        })
        .collect();
    for (name, status) in
        crate::executable::executable_lock_statuses(&dir, &executable_servers, &lock)
    {
        match crate::verify::executable_verdict(&status) {
            crate::verify::Verdict::Ok => {}
            crate::verify::Verdict::Unpinned => {
                server_blockers.push(serde_json::json!({
                    "name": crate::text::sanitize_line(&name),
                    "reason": "local executable content is not pinned yet",
                    "fix": "agentstack lock",
                }));
            }
            crate::verify::Verdict::Block(reason) => {
                server_blockers.push(serde_json::json!({
                    "name": crate::text::sanitize_line(&name),
                    "reason": crate::text::sanitize_line(&reason),
                    "fix": "edit-manifest",
                }));
            }
        }
    }

    let secrets: Vec<String> = m.referenced_secrets();

    // The COMPLETE reviewed surface, by name — not just counts. What an
    // external consent screen renders must be the same item list the
    // interactive review prints; a preview that collapsed workflows or
    // extensions into a number would let a user consent to code they never
    // saw named. All names arrive from repo content — hostile input — so
    // display copies are sanitized.
    let skills: Vec<String> = review_skill_names(m)
        .iter()
        .map(|n| crate::text::sanitize_line(n))
        .collect();
    let workflows: Vec<serde_json::Value> = m
        .workflows
        .iter()
        .map(|(name, w)| {
            serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "roles": w.roles.iter().map(|r| crate::text::sanitize_line(r)).collect::<Vec<_>>(),
            })
        })
        .collect();
    let extensions: Vec<serde_json::Value> = m
        .extensions
        .iter()
        .map(|(name, e)| {
            serde_json::json!({
                "name": crate::text::sanitize_line(name),
                "target": crate::text::sanitize_line(&e.target),
            })
        })
        .collect();
    let instructions: Vec<String> = m
        .instructions
        .iter()
        .filter(|(_, i)| !i.from_user_layer)
        .map(|(name, _)| crate::text::sanitize_line(name))
        .collect();

    // §7.2: `surface_digest` (computed above, from the same snapshot the
    // display was parsed from) is exactly what a later grant must present as
    // `--consented-digest` — so "the surface shown" and "the bytes granted"
    // can never diverge without the digest flipping.
    let out = serde_json::json!({
        "path": base.display().to_string(),
        "state": state,
        "re_trust": re_trust,
        "surface_digest": surface_digest,
        "servers": servers,
        "server_blockers": server_blockers,
        "secrets": secrets,
        "skills": skills,
        "workflows": workflows,
        "extensions": extensions,
        "instructions": instructions,
        "counts": {
            "skills": skills.len(),
            "workflows": workflows.len(),
            "extensions": extensions.len(),
            "instructions": instructions.len(),
        },
    });
    Ok(crate::ui_contract::envelope(out))
}

/// Resolve the project base to act on: walk up from the given path (or cwd) so
/// `agentstack trust` works from a subdirectory too.
fn resolve_base(path: Option<&Path>) -> Result<PathBuf> {
    let start = match path {
        Some(p) => p
            .canonicalize()
            .with_context(|| format!("no such directory: {}", p.display()))?,
        None => std::env::current_dir()?,
    };
    crate::manifest::discover_project_base(&start).with_context(|| {
        format!(
            "no agentstack manifest at or above {} — run `agentstack init` first",
            start.display()
        )
    })
}

/// Parse the manifest layers out of a [`trust::ConsentSnapshot`]'s captured
/// bytes — the only way the review may load them, so what the human reads and
/// what the digest identifies are always the same bytes.
fn load_snapshot_manifest(
    snapshot: &trust::ConsentSnapshot,
    dir: &Path,
) -> Result<crate::manifest::LoadedManifest> {
    let manifest_text = std::str::from_utf8(&snapshot.manifest).with_context(|| {
        format!(
            "{} is not valid UTF-8",
            dir.join("agentstack.toml").display()
        )
    })?;
    let local_text = snapshot
        .local
        .as_deref()
        .map(std::str::from_utf8)
        .transpose()
        .with_context(|| {
            format!(
                "{} is not valid UTF-8",
                dir.join("agentstack.local.toml").display()
            )
        })?;
    crate::manifest::load_from_contents(dir, manifest_text, local_text)
}

/// Parse the lockfile from the same snapshot (absent → empty default lock),
/// mirroring [`load_snapshot_manifest`]: the pins the review verifies against
/// are exactly the pin bytes the consent digest covers.
fn lock_from_snapshot(snapshot: &trust::ConsentSnapshot, dir: &Path) -> Result<crate::lock::Lock> {
    let path = crate::lock::Lock::path(dir);
    match snapshot.lock.as_deref() {
        None => Ok(crate::lock::Lock::default()),
        Some(bytes) => {
            let text = std::str::from_utf8(bytes)
                .with_context(|| format!("{} is not valid UTF-8", path.display()))?;
            crate::lock::Lock::parse(text, &path)
        }
    }
}

fn grant(base: &Path, yes: bool, consented: Option<&str>) -> Result<()> {
    grant_gated(base, yes, consented, std::io::stdin().is_terminal(), None)
}

/// Extra review lines and the one question, supplied by the funnel. Held by
/// reference through the single grant path — it never becomes a second one.
pub(crate) struct ConsentCard {
    pub lines: Vec<String>,
    pub question: String,
    /// The answer, when it is supplied instead of read from stdin — the same
    /// kind of injected probe as `grant_gated`'s `interactive`, and for the
    /// same reason: a consent gate whose refusal path cannot be exercised in a
    /// test is a consent gate whose refusal path is unverified. Production
    /// always passes `None` and prompts; only the funnel's test seam sets it.
    pub answer: Option<bool>,
}

/// Review-and-grant with the funnel's card folded into the same screen. The
/// only entry point besides `trust` itself, and it reaches the identical
/// [`grant_gated`] — same surface, same digest, same recorded event.
pub(crate) fn grant_with_card(
    base: &Path,
    yes: bool,
    interactive: bool,
    card: &ConsentCard,
) -> Result<()> {
    grant_gated(base, yes, None, interactive, Some(card))
}

/// The grant path with the TTY probe injected, so the non-interactive consent
/// gate is testable without a real terminal. `interactive` is whether stdin is
/// a TTY; production passes `std::io::stdin().is_terminal()`.
///
/// Typing `agentstack trust` at a terminal IS the consent (direnv-allow style),
/// so an interactive session is unchanged. When stdin is NOT a terminal — a
/// pipe, a here-string, or an agent driving the shell — the command refuses
/// unless `--yes` explicitly acknowledges the review AND `--consented-digest`
/// binds that acknowledgement to the exact previewed bytes (§7.2): `--yes`
/// alone would let any RPC caller grant without anyone having seen the
/// surface, which is precisely the UI-enforcement gap this closes.
///
/// Honesty about the probe (independent review, 2026-07-23): `isatty(stdin)`
/// proves stdin is a terminal DEVICE, not that a human is attending it — a
/// process that allocates a PTY (`script`, `expect`, Python's `pty`) reads as
/// interactive. That is accepted, not overlooked: the trust store is a plain
/// file under the user's own account, so any same-user process able to stage
/// a PTY could equally write `trust.json` directly. The gate's enforceable
/// job is narrower and holds — headless callers (RPC servers, plain shell
/// pipes) cannot grant without presenting the reviewed digest — and the real
/// boundary against a hostile same-user process is the OS user account, as
/// `docs/ENFORCEMENT.md` states.
///
/// The entire review below renders from ONE [`trust::ConsentSnapshot`], and
/// the no-digest grant records that snapshot's digest — never a re-read — so
/// bytes swapped in mid-review are not blessed: the store then holds the
/// reviewed digest, the project reads `Changed`, and use sites fail closed.
/// `card` extends this one consent screen for the Phase 1 funnel instead of
/// giving the funnel a screen of its own: extra review lines are printed with
/// the surface (so the combined preview shows everything the separate steps
/// show, and never less), and the funnel's single confirmation is asked HERE,
/// after the whole review and before any grant. There is exactly one place a
/// human says yes to a project, whichever verb brought them to it.
fn grant_gated(
    base: &Path,
    yes: bool,
    consented: Option<&str>,
    interactive: bool,
    card: Option<&ConsentCard>,
) -> Result<()> {
    grant_probed(base, yes, consented, interactive, card, None)
}

/// The `trust` path (no funnel card) with re-gate answers injected — the entry
/// integration tests drive. Kept card-free so [`ConsentCard`], which belongs to
/// the funnel, stays crate-private.
pub fn grant_with_answers(
    base: &Path,
    yes: bool,
    consented: Option<&str>,
    interactive: bool,
    probe: Option<&ReGateProbe>,
) -> Result<()> {
    grant_probed(base, yes, consented, interactive, None, probe)
}

/// [`grant_gated`] with the re-gate answers injectable. See [`ReGateProbe`].
pub(crate) fn grant_probed(
    base: &Path,
    yes: bool,
    consented: Option<&str>,
    interactive: bool,
    card: Option<&ConsentCard>,
    probe: Option<&ReGateProbe>,
) -> Result<()> {
    let dir = crate::manifest::resolve_manifest_dir(base);
    let Some(snapshot) = trust::ConsentSnapshot::read(base) else {
        // No readable manifest: surface the same friendly first-contact error
        // the disk load path gives.
        crate::manifest::load_from_dir(&dir)?;
        anyhow::bail!("manifest disappeared while reviewing {}", base.display());
    };
    let loaded = load_snapshot_manifest(&snapshot, &dir)?;
    let m = &loaded.manifest;
    let surface_digest = snapshot.digest();

    // Name the whole consequence, not one consumer. The gateway used to be the
    // only thing this gate fed, but `session start` (and every other activation
    // path) refuses on an untrusted project too — describing it as gateway-only
    // made that refusal read as a bug.
    println!(
        "Reviewing {} — approving this lets its capabilities activate.\n",
        base.display().to_string().bold()
    );

    // P14: when this project was trusted before, mark the review against the
    // surface it last consented to — so a `git pull`'s new `evil` server reads
    // as `+ added` instead of hiding in a flat re-list. First-ever trust (and
    // an older entry that recorded no snapshot) stays the flat full review.
    let prior = trust::prior_surface(base);
    let untracked = matches!(prior, PriorSurface::Untracked);
    // Kept alongside the diff machinery: the re-gate card reads each item's
    // recorded PIN from here, which is what lets it diff against the bytes the
    // human approved rather than against the lock that drifted.
    let prior_items: Vec<SurfaceItem> = match &prior {
        PriorSurface::Recorded(items) => items.clone(),
        _ => Vec::new(),
    };
    let mut diff = ReviewDiff::new(prior);
    if diff.diffing() {
        println!(
            "Re-trust — marking what changed since you last trusted this ({} added, {} changed, {} removed):\n",
            "+".green(),
            "~".yellow(),
            "-".red()
        );
    } else if untracked {
        println!(
            "Re-trust — no reviewed-surface snapshot was recorded last time, so this is a full re-review, not a diff.\n"
        );
    }

    // Preview the gateway's actual runtime surface, not just the inline
    // `[servers.*]` tables: library name refs resolve here exactly like they
    // will at gateway time, so the human reviews everything auto-mode may run.
    let library = crate::library::Library::load_default_or_warn();
    let lib_home = crate::util::paths::lib_home();
    // A broken lockfile must fail the trust review loudly: its pins are part
    // of what the human is consenting to, and the gateway will refuse
    // library-backed servers under an unreadable lock anyway. Parsed from the
    // snapshot bytes, so the pins reviewed are the pins the digest covers.
    let lock = lock_from_snapshot(&snapshot, &dir)?;
    let servers = crate::resolve::effective_runtime_servers(m, &library, &lib_home, None);
    // Phase 2: the review is COMPOSED into `body` first and rendered after, so
    // the glanceable card can lead with what this project runs, contacts, and
    // may read — facts that are only known once the whole surface has been
    // walked. Nothing is dropped in the process: every line that printed before
    // is pushed here and printed below, which is what makes "the card never
    // discloses less than the old preview" a structural property of the
    // rendering order rather than a claim about the copy.
    let mut body: Vec<String> = Vec::new();
    macro_rules! say {
        ($($t:tt)*) => { body.push(format!($($t)*)) };
    }
    say!("This project declares — review what auto-mode may run/contact:");
    if servers.is_empty() {
        say!("  (no servers)");
    }
    // Trusting pins the lock bytes into the trust digest, so trusting over a
    // drifted or unpinned surface would bless pins that don't match content
    // (or bless no pin at all). Everything that must be lock-verified at use
    // time therefore has to be pinned and matching BEFORE trust is granted:
    // `agentstack lock` is a prerequisite of `agentstack trust`.
    let mut blockers: Vec<(String, String)> = Vec::new();
    // Re-gate questions the walk stages but does not ask; see PendingAnswer.
    let mut pending: Vec<PendingAnswer> = Vec::new();
    for (name, resolved) in &servers {
        // This review is the consent screen for content that may be hostile —
        // display copies are sanitized; diff identities and lookups stay RAW
        // (two different hostile values must never collide after cleaning).
        let disp = crate::text::sanitize_line(name);
        let r = match resolved {
            Ok(r) => r,
            Err(e) => {
                let mk = diff.mark("server", name, "unresolvable");
                say!(
                    "{mk}{} {disp}: unresolvable ({})",
                    "✗".red(),
                    crate::text::sanitize_line(&e.to_string())
                );
                blockers.push((name.clone(), format!("broken server ref — {e}")));
                continue;
            }
        };
        let origin = match r.origin {
            crate::resolve::ServerOrigin::Inline => String::new(),
            crate::resolve::ServerOrigin::Library => match lock.get_server(name) {
                Some(entry) if entry.checksum.hex() == r.checksum => {
                    "   [library, pinned]".to_string()
                }
                Some(_) => {
                    blockers.push((
                        name.clone(),
                        "library server definition DRIFTED from lock".to_string(),
                    ));
                    format!("   [library, {}]", "DRIFTED from lock".red())
                }
                None => {
                    blockers.push((
                        name.clone(),
                        "library server unpinned — run `agentstack lock`".to_string(),
                    ));
                    format!("   [library, {}]", "unpinned".red())
                }
            },
        };
        match r.server.server_type {
            // A stdio server is arbitrary local code execution — the thing the
            // trust gate exists for. Call it out explicitly. The diff identity
            // is the command line (what actually runs), not the pin/origin
            // annotation — pin drift is already a hard blocker below.
            ServerType::Stdio => {
                let command = r.server.command.as_deref().unwrap_or("?");
                let args = r.server.args.join(" ");
                let mk = diff.mark("server", name, &format!("{command} {args}"));
                say!(
                    "{mk}{} {disp}: runs `{}`{origin}",
                    "▶".yellow(),
                    crate::text::sanitize_line(&format!("{command} {args}"))
                );
            }
            ServerType::Http => {
                let url = r.server.url.as_deref().unwrap_or("?");
                let mk = diff.mark("server", name, url);
                say!(
                    "{mk}{} {disp}: contacts {}{origin}",
                    "→".cyan(),
                    crate::text::sanitize_line(url)
                );
            }
        }
    }
    let refs = m.referenced_secrets();
    if !refs.is_empty() {
        // Secrets are one aggregate line; its identity is the (sorted, from
        // `referenced_secrets`) set, so adding or dropping any ref flips the
        // whole line to `~ changed`.
        let joined = refs.join(", ");
        let mk = diff.mark("secrets", "", &joined);
        say!(
            "{mk}secrets referenced: {}",
            crate::text::sanitize_line(&joined)
        );
    }

    // D3 (contract §8): the repository-local executable surface, pinned by
    // current bytes. Ruling: an unpinned repo-relative executable BLOCKS
    // trust — the lock is a prerequisite of trust, so an unpinned declared
    // executable means the lock is incomplete, and trusting would bless
    // ungoverned local code. What stays honestly unbound (the interpreter/
    // harness binary itself, imports outside a declared root) is labeled.
    let exec_servers: Vec<(String, crate::manifest::Server)> = servers
        .iter()
        .filter_map(|(n, r)| r.as_ref().ok().map(|r| (n.clone(), r.server.clone())))
        .collect();
    let exec_statuses = crate::executable::executable_lock_statuses(&dir, &exec_servers, &lock);
    if !exec_statuses.is_empty() {
        say!("  local executable content (pinned by current bytes):");
        for (label, status) in &exec_statuses {
            let disp = crate::text::sanitize_line(label);
            // An executable is identified by its path (the label the review
            // shows); byte drift is caught by the verdict below, not the diff.
            let mk = diff.mark("executable", label, label);
            match crate::verify::executable_verdict(status) {
                crate::verify::Verdict::Ok => say!("{mk}· {disp}   [pinned]"),
                crate::verify::Verdict::Unpinned => {
                    say!("{mk}{} {disp}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        label.clone(),
                        "local executable unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                crate::verify::Verdict::Block(why) => {
                    say!("{mk}{} {disp}   [{}]", "✗".red(), why.red());
                    blockers.push((label.clone(), why));
                }
            }
        }
        say!(
            "  (unbound, by design: interpreter/harness binaries from $PATH, and imports outside a declared integrity root)"
        );
    }

    // Native extensions (D6): executable add-on code delivered into a
    // harness's own extension directory. It runs INSIDE the harness process,
    // outside the policy ceiling — the pin is the only governance there is,
    // so unpinned AND drifted both block, like the D3 executable surface.
    if !m.extensions.is_empty() {
        say!(
            "  native extensions (EXECUTABLE — run inside the harness process; agentstack pins the bytes but cannot govern them at runtime):"
        );
        let store = crate::store::Store::default_store();
        for (name, ext) in &m.extensions {
            use crate::resolve::{ExtensionLockStatus, ExtensionOrigin};
            let disp = crate::text::sanitize_line(name);
            let dest = format!("→ {}", crate::text::sanitize_line(&ext.target));
            // The extension's identity for the diff is its target (where it
            // installs); a retarget shows as `~ changed`.
            let mk = diff.mark("extension", name, &ext.target);
            // Read-only review: never fetch a git source here. An un-cached git
            // extension surfaces as offline, exactly like a skill.
            let report = crate::resolve::extension_lock_status(
                name,
                ext,
                &dir,
                &library,
                &lib_home,
                &store,
                &lock,
                crate::resolve::ResolveMode::NoFetch,
            );
            let origin_word = match report.origin {
                Some(ExtensionOrigin::Inline) => "inline",
                Some(ExtensionOrigin::Library) => "library",
                None => "?",
            };
            match report.status {
                ExtensionLockStatus::Matches => {
                    say!(
                        "{mk}{} {disp} {dest}   [{origin_word}, pinned]",
                        "▶".yellow()
                    );
                }
                ExtensionLockStatus::MissingLockEntry => {
                    say!(
                        "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                        "✗".red(),
                        "unpinned".red()
                    );
                    blockers.push((
                        name.clone(),
                        "extension unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                ExtensionLockStatus::ChecksumDrift { .. }
                | ExtensionLockStatus::RevDrift { .. } => {
                    say!(
                        "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                        "✗".red(),
                        "DRIFTED from lock".red()
                    );
                    blockers.push((
                        name.clone(),
                        "extension content drifted from lock".to_string(),
                    ));
                }
                ExtensionLockStatus::TargetDrift { locked, .. } => {
                    say!(
                        "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                        "✗".red(),
                        format!(
                            "RETARGETED since locked (was '{}')",
                            crate::text::sanitize_line(&locked)
                        )
                        .red()
                    );
                    blockers.push((
                        name.clone(),
                        "extension target changed since locked — run `agentstack lock`".to_string(),
                    ));
                }
                // Reproducibility can't be checked offline; not a blocker —
                // same posture as skills' un-cached git sources.
                ExtensionLockStatus::NotAvailableOffline { .. } => say!(
                    "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                    "▶".yellow(),
                    "offline — pin unverified".yellow()
                ),
                ExtensionLockStatus::ResolveFailed { error } => {
                    say!("{mk}{} {disp} {dest}: {}", "✗".red(), error.red());
                    blockers.push((name.clone(), error));
                }
            }
        }
    }

    // Governed workflows (D7 W1): orchestration code agentstack ITSELF will
    // execute, spawning agent runs under the declared role profiles. Stronger
    // than skills (context, not code), different in kind from extensions (a
    // harness runs those, ungoverned; agentstack runs this, gated and
    // sandboxed — which is precisely why the gate stands in front of it).
    // Unpinned, drifted, roles-drifted, and unresolvable all block, like the
    // extension surface; the diff identity is the sorted role set, so a roles
    // widening reads as `~ changed` even with unchanged bytes.
    if !m.workflows.is_empty() {
        say!(
            "  workflows (ORCHESTRATION CODE — spawns agent runs under the declared roles; agentstack executes this, gated and sandboxed):"
        );
        let store = crate::store::Store::default_store();
        for (name, wf) in &m.workflows {
            use crate::resolve::WorkflowLockStatus;
            let disp = crate::text::sanitize_line(name);
            let roles = wf.roles_sorted_unique();
            let roles_joined = roles.join(", ");
            let dest = format!(
                "→ roles: {}",
                if roles.is_empty() {
                    "(none — spawns nothing)".to_string()
                } else {
                    crate::text::sanitize_line(&roles_joined)
                }
            );
            let mk = diff.mark("workflow", name, &roles_joined);
            // Read-only review: never fetch a git source here. An un-cached
            // git workflow surfaces as offline, exactly like a skill.
            let status = crate::resolve::workflow_lock_status(
                name,
                wf,
                &dir,
                &store,
                &lock,
                crate::resolve::ResolveMode::NoFetch,
            );
            match status {
                WorkflowLockStatus::Matches => {
                    say!("{mk}{} {disp} {dest}   [pinned]", "▶".yellow());
                }
                WorkflowLockStatus::MissingLockEntry => {
                    say!("{mk}{} {disp} {dest}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        name.clone(),
                        "workflow unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                WorkflowLockStatus::ChecksumDrift { .. } | WorkflowLockStatus::RevDrift { .. } => {
                    say!(
                        "{mk}{} {disp} {dest}   [{}]",
                        "✗".red(),
                        "DRIFTED from lock".red()
                    );
                    blockers.push((
                        name.clone(),
                        "workflow content drifted from lock".to_string(),
                    ));
                }
                WorkflowLockStatus::RolesDrift { locked, .. } => {
                    say!(
                        "{mk}{} {disp} {dest}   [{}]",
                        "✗".red(),
                        format!(
                            "ROLES CHANGED since locked (was: {})",
                            crate::text::sanitize_line(&locked.join(", "))
                        )
                        .red()
                    );
                    blockers.push((
                        name.clone(),
                        "workflow roles changed since locked — run `agentstack lock`".to_string(),
                    ));
                }
                // Reproducibility can't be checked offline; not a blocker —
                // same posture as skills' and extensions' un-cached git sources.
                WorkflowLockStatus::NotAvailableOffline { .. } => say!(
                    "{mk}{} {disp} {dest}   [{}]",
                    "▶".yellow(),
                    "offline — pin unverified".yellow()
                ),
                WorkflowLockStatus::ResolveFailed { error } => {
                    say!(
                        "{mk}{} {disp} {dest}: {}",
                        "✗".red(),
                        crate::text::sanitize_line(&error).red()
                    );
                    blockers.push((name.clone(), error));
                }
            }
            // F13: when this script was authored from an approved blueprint,
            // show the SHAPE that was approved right here — this gate is the
            // one that authorizes execution, and until now it showed only
            // bytes while the graph the user actually reviewed lived in a chat
            // message. A reviewer should not have to remember a picture.
            if let Some(declared) = &wf.blueprint {
                for line in blueprint_review_lines(&dir, name, declared, &lock, &mut blockers) {
                    say!("      {line}");
                }
            }
        }
    }

    // Skills, reviewed like servers: name + origin + pin status. Their bodies
    // are exactly the bytes the trust digest does NOT cover, so the pin is
    // the only thing binding what the human reviews to what gets served.
    let skill_names = review_skill_names(m);
    if !skill_names.is_empty() {
        say!("  skills loadable over MCP:");
        let store = crate::store::Store::default_store();
        for name in &skill_names {
            let disp = crate::text::sanitize_line(name);
            let report = crate::resolve::skill_lock_status(
                name,
                m,
                &dir,
                &library,
                &lib_home,
                &store,
                &lock,
                crate::resolve::ResolveMode::NoFetch,
            );
            use crate::resolve::{SkillLockStatus, SkillOrigin};
            let origin_word = match report.origin {
                Some(SkillOrigin::Inline) => "inline",
                Some(SkillOrigin::Library) => "library",
                None => "?",
            };
            // A skill has no command/url; its diff identity is where its body
            // comes from (inline vs library), so a source flip shows `~ changed`.
            // The PIN — the lock checksum of the bytes being consented to — is
            // recorded alongside, not folded into the identity: it is what a
            // later re-gate needs to find the approved bytes in the content
            // store and render a real diff instead of "digest mismatch".
            let mk = diff.mark_pinned(
                "skill",
                name,
                origin_word,
                lock.get(name).map(|e| e.checksum.hex().to_string()),
            );
            match &report.status {
                SkillLockStatus::Matches => {
                    say!("{mk}· {disp}   [{origin_word}, pinned]");
                }
                SkillLockStatus::ChecksumDrift { .. } | SkillLockStatus::RevDrift { .. } => {
                    // Phase 2: say WHAT changed, not just that something did.
                    // The approved bytes are looked up by the pin the last
                    // consent recorded, so this compares against what the human
                    // actually said yes to — not against the current lock,
                    // which is what drifted in the first place.
                    let pin = prior_pin_for(&prior_items, "skill", name)
                        .or_else(|| lock.get(name).map(|e| e.checksum.hex().to_string()));
                    let live = live_skill_dir(name, m, &library, &dir, &lib_home, &store);
                    let pin_diff = match (&pin, &live) {
                        (Some(pin), Some(live)) => {
                            crate::regate::diff_against_pin(store.root(), pin, live)
                        }
                        _ => crate::regate::PinDiff::NoSnapshot,
                    };
                    let headline = crate::regate::headline(&pin_diff)
                        .unwrap_or_else(|| "changed since you approved it".to_string());
                    say!(
                        "{mk}{} {disp}   [{origin_word}, {}]",
                        "✗".red(),
                        headline.red()
                    );
                    for line in crate::regate::render_lines(&pin_diff, crate::regate::DIFF_LINE_CAP)
                    {
                        say!("  {line}");
                    }
                    blockers.push((name.clone(), "skill content drifted from lock".to_string()));
                    // STAGE the question — do not ask it here. The walk has not
                    // printed anything yet (Slice A composes into `body` and
                    // renders afterwards), so prompting at this point would ask
                    // the human to judge a change before showing them the diff
                    // that is, at this instant, still sitting unrendered in
                    // `body`. The answer loop runs after the render.
                    if !matches!(pin_diff, crate::regate::PinDiff::NoSnapshot) {
                        pending.push(PendingAnswer {
                            kind: "skill",
                            name: name.clone(),
                            // Which blocker this answer clears. Keyed by INDEX,
                            // not by name: `blockers` is `(name, why)` with no
                            // kind, so a skill and an instruction sharing a
                            // name would clear each other's.
                            blocker_ix: blockers.len() - 1,
                            approved_pin: pin.clone(),
                            live: live.clone(),
                            headline: headline.clone(),
                        });
                    }
                }
                SkillLockStatus::MissingLockEntry => match report.origin {
                    // An inline skill's bytes live in the repo under review —
                    // unpinned means trusting would leave them ungoverned.
                    Some(SkillOrigin::Inline) => {
                        say!("{mk}{} {disp}   [inline, {}]", "✗".red(), "unpinned".red());
                        blockers.push((
                            name.clone(),
                            "inline skill unpinned — run `agentstack lock`".to_string(),
                        ));
                    }
                    // A library skill's bytes are the user's own curated,
                    // scan-gated content — worth pinning, not worth blocking.
                    _ => say!(
                        "{mk}· {disp}   [{origin_word}, {}]",
                        "unpinned — run `agentstack lock`".yellow()
                    ),
                },
                // Reproducibility can't be checked offline; not a blocker.
                SkillLockStatus::NotAvailableOffline { .. } => say!(
                    "{mk}· {disp}   [{origin_word}, {}]",
                    "offline — pin unverified".yellow()
                ),
                SkillLockStatus::ResolveFailed { error } => {
                    say!(
                        "{mk}{} {disp}: broken ref ({})",
                        "✗".red(),
                        crate::text::sanitize_line(error)
                    );
                    blockers.push((name.clone(), format!("broken ref — {error}")));
                }
            }
        }
    }

    // Instruction fragments, same review: they compile into CLAUDE.md /
    // AGENTS.md — straight into agent context — and their bytes are repo
    // content the trust digest doesn't cover. The pin is what binds them.
    // (grant loads the project manifest only, so machine-layer fragments
    // can't appear here; the filter guards the invariant regardless.)
    let instructions: Vec<_> = m
        .instructions
        .iter()
        .filter(|(_, i)| !i.from_user_layer)
        .collect();
    if !instructions.is_empty() {
        say!("  instruction fragments (compile into CLAUDE.md / AGENTS.md):");
        for (name, instr) in instructions {
            let disp = crate::text::sanitize_line(name);
            use crate::resolve::InstructionLockStatus;
            // Instructions are keyed by name; there is no finer identity to
            // show, so they only ever read as added or removed. The pin is
            // what makes a re-gate able to show which lines of the fragment
            // moved — the identity alone could never carry that.
            let mk = diff.mark_pinned(
                "instruction",
                name,
                "",
                lock.get_instruction(name)
                    .map(|e| e.checksum.hex().to_string()),
            );
            match crate::resolve::instruction_lock_status(name, instr, &dir, &lock) {
                InstructionLockStatus::Matches => say!("{mk}· {disp}   [pinned]"),
                InstructionLockStatus::ChecksumDrift { .. } => {
                    say!("{mk}{} {disp}   [{}]", "✗".red(), "DRIFTED from lock".red());
                    blockers.push((
                        name.clone(),
                        "instruction content drifted from lock".to_string(),
                    ));
                }
                InstructionLockStatus::MissingLockEntry => {
                    say!("{mk}{} {disp}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        name.clone(),
                        "instruction unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                InstructionLockStatus::ResolveFailed { error } => {
                    say!(
                        "{mk}{} {disp}: broken ref ({})",
                        "✗".red(),
                        crate::text::sanitize_line(&error)
                    );
                    blockers.push((name.clone(), format!("broken ref — {error}")));
                }
            }
        }
    }

    // Hooks: an EXECUTABLE kind. Declaring or editing one re-gates trust
    // (the manifest bytes move, so the digest moves) — but until Phase 2 this
    // screen said nothing about them, so the human was re-asked without being
    // shown what they were re-approving. That is a consent surprise on the one
    // kind that runs commands in or around the harness at user permission, and
    // strategy v2 gives hooks the full ceremony with no compressed path. The
    // diff identity is the whole invocation (event, matcher, command line,
    // timeout, targets): changing ANY of them must read as `~ changed`, never
    // hide behind a stable name.
    if !m.hooks.is_empty() {
        say!(
            "  hooks (EXECUTABLE — agentstack compiles these into each harness's native config; the harness runs them at your permission, and agentstack does not govern them at runtime):"
        );
        for (name, hook) in &m.hooks {
            let disp = crate::text::sanitize_line(name);
            let args = if hook.args.is_empty() {
                String::new()
            } else {
                format!(" {}", hook.args.join(" "))
            };
            let invocation = format!("{}{args}", hook.command);
            let matcher = match &hook.matcher {
                Some(mt) if !mt.is_empty() => format!(" matching {mt}"),
                _ => String::new(),
            };
            let timeout = match hook.timeout {
                Some(t) => format!(", timeout {t}s"),
                None => String::new(),
            };
            // `targets` defaults to the wildcard `["*"]`, which is manifest
            // syntax, not something a consent screen may make the reader
            // decode — a bare `[*]` is the widest possible scope rendered as
            // the least alarming glyph. Say what it means. The diff identity
            // keeps the RAW targets: two manifests that differ only in
            // wildcard-vs-explicit must still read as `~ changed`.
            let raw_targets = hook.targets.join(", ");
            let targets_disp = if hook.targets.iter().any(|t| t == "*") {
                "every hook-capable CLI".to_string()
            } else {
                raw_targets.clone()
            };
            let identity = format!(
                "{}{matcher} runs {invocation}{timeout} → {raw_targets}",
                hook.event
            );
            let mk = diff.mark("hook", name, &identity);
            say!(
                "{mk}{} {disp}: on {} runs `{}`{}   [in {}]",
                "▶".yellow(),
                crate::text::sanitize_line(&format!("{}{matcher}", hook.event)),
                crate::text::sanitize_line(&invocation),
                crate::text::sanitize_line(&timeout),
                crate::text::sanitize_line(&targets_disp)
            );
        }
    }

    // Settings: inert per-CLI native config (permissions, feature flags) merged
    // into each harness's settings file. Not executable, but a settings value
    // can widen what a harness will do without asking — `ENFORCEMENT.md` says
    // editing one re-gates consent, so the review has to show it. The identity
    // is the canonical JSON of the whole per-adapter object, so any value
    // change reads as `~ changed`.
    if !m.settings.is_empty() {
        say!("  settings (merged into each CLI's own config file):");
        for (adapter, value) in &m.settings {
            let disp = crate::text::sanitize_line(adapter);
            // Canonical, key-sorted rendering: two objects that differ only in
            // key order are the same consent, and must not read as changed.
            let identity = canonical_json(value);
            let keys = match value.as_object() {
                Some(o) => {
                    let mut k: Vec<&str> = o.keys().map(|s| s.as_str()).collect();
                    k.sort_unstable();
                    k.join(", ")
                }
                None => identity.clone(),
            };
            let mk = diff.mark("settings", adapter, &identity);
            say!("{mk}· {disp}: sets {}", crate::text::sanitize_line(&keys));
        }
    }

    // Requested policy, shown at the trust boundary (ARCHITECTURE: "review
    // shows … policy changes"). Display-only: a bundle's policy can only
    // narrow — the machine layer caps everything at runtime regardless — so
    // there is nothing here to block on, but the human should see what the
    // repo asks for before blessing it.
    review_policy(&m.policy, &mut diff, &mut body);

    // P14: anything the last consented surface carried that is gone now. Printed
    // as part of the review (before the blocker bail) so the human sees the full
    // diff. A scoped block ends the borrow of `diff` before its `current` moves.
    {
        let removed = diff.removed();
        if !removed.is_empty() {
            say!("  no longer present (was trusted before):");
            for it in removed {
                let label = if it.name.is_empty() {
                    it.kind.clone()
                } else {
                    format!("{} {}", it.kind, crate::text::sanitize_line(&it.name))
                };
                let detail = if it.identity.is_empty() {
                    String::new()
                } else {
                    format!("  ({})", crate::text::sanitize_line(&it.identity))
                };
                say!("{} {label}{detail}", "-".red());
            }
        }
    }

    // ---- The card ----------------------------------------------------------
    // Two to five plain lines, answering the only questions the moment asks:
    // what runs, what it reaches, what it may read, and whether the bytes are
    // the ones being reviewed. Computed from the surface just walked, so it can
    // never describe a different set than the detail below it. Then the detail,
    // unabridged — the card summarizes the review, it does not replace it.
    for line in card_summary_lines(&diff.current, blockers.len()) {
        println!("{line}");
    }
    println!();
    for line in &body {
        println!("{line}");
    }

    // ---- The answer loop -----------------------------------------------
    // The only place a re-gate question is asked, and it asks NOTHING that
    // acts: every answer is recorded in memory and applied at the commit
    // point below. This is the staging contract
    // (`docs/design/consent-card.md`, "Answers stage; the single final yes
    // commits"), and this is the one function to audit for effects leaking
    // early — there are none between here and the commit.
    //
    // Placement is forced: after the render (so the human has seen the diff
    // they are judging) and before the blocker bail (so an answer can still
    // clear its blocker). `consented.is_none()` is load-bearing beyond the
    // usual non-interactive check — `--consented-digest` does NOT require
    // `--yes`, so a TTY caller passing only a digest is `interactive && !yes`
    // yet bound to a digest that accepting would invalidate.
    let mut answers: Vec<(usize, Answer)> = Vec::new();
    if let Some(probe) = probe {
        for (ix, p) in pending.iter().enumerate() {
            if let Some((_, a)) = probe.answers.iter().find(|(n, _)| *n == p.name) {
                answers.push((ix, *a));
            }
        }
    } else if interactive && !yes && consented.is_none() && !pending.is_empty() {
        println!(
            "\n{}",
            "This content changed since you approved it. For each item:".bold()
        );
        for (ix, p) in pending.iter().enumerate() {
            let disp = crate::text::sanitize_line(&p.name);
            let picked = crate::util::confirm::choose(
                &format!("\n  {} {disp} — {}", p.kind, p.headline),
                &[
                    ("a", "accept the change"),
                    ("k", "keep the approved version"),
                    ("b", "block this item"),
                ],
            )?;
            // `None` is not a fourth answer: it means nothing was decided, so
            // this item's blocker stays and the review refuses exactly as it
            // does today. Silence never resolves a consent question.
            match picked.as_deref() {
                Some("a") => answers.push((ix, Answer::Accept)),
                Some("k") => answers.push((ix, Answer::KeepPinned)),
                Some("b") => answers.push((ix, Answer::Block)),
                _ => {}
            }
        }
    }
    // Answers only ever REMOVE blockers; nothing here can add surface.
    {
        let cleared: Vec<usize> = answers
            .iter()
            .map(|(ix, _)| pending[*ix].blocker_ix)
            .collect();
        let mut keep = 0usize;
        blockers.retain(|_| {
            let this = keep;
            keep += 1;
            !cleared.contains(&this)
        });
    }

    if !blockers.is_empty() {
        // Names and reasons carry manifest/resolver text — hostile input, so
        // the summary sanitizes exactly like the per-line review above.
        let blockers: Vec<(String, String)> = blockers
            .iter()
            .map(|(name, why)| {
                (
                    crate::text::sanitize_line(name),
                    crate::text::sanitize_line(why),
                )
            })
            .collect();
        let width = blockers.iter().map(|(n, _)| n.len()).max().unwrap_or(0);
        let lines: Vec<String> = blockers
            .iter()
            .map(|(name, why)| format!("  {name:width$}  {why}"))
            .collect();
        let next = if blockers
            .iter()
            .all(|(_, why)| why.contains("agentstack lock"))
        {
            "Run `agentstack lock`, review the result, then `agentstack trust` again."
        } else {
            "Fix or remove the blocked declarations above. Then run `agentstack lock` for \
             anything marked unpinned and review again."
        };
        anyhow::bail!(
            "cannot trust {}: its loadable surface isn't fully pinned — {} {} locking or review:\n{}\n{}",
            base.display(),
            super::count(blockers.len(), "item"),
            if blockers.len() == 1 { "needs" } else { "need" },
            lines.join("\n"),
            next
        );
    }

    // The funnel's own review lines join the surface above, inside the same
    // screen, before the same gate — presentation is combined, the disclosure
    // is additive.
    if let Some(card) = card {
        for line in &card.lines {
            println!("{line}");
        }
    }

    // Consent gate: the review above is now fully printed. Trust is granted by
    // a human who read it — typing the command at a terminal IS that consent.
    // When stdin is not a terminal (a pipe, a here-string, an agent driving the
    // shell), there is no interactive consent, so refuse unless `--yes` was
    // passed to acknowledge the review explicitly. This runs BEFORE anything is
    // pinned or written, so a refusal leaves the trust store untouched — an
    // agent with shell access cannot self-trust a repo to defeat the
    // untrusted-means-inert gate.
    if !interactive && !yes {
        anyhow::bail!(
            "refusing to trust: stdin is not a terminal — review the declarations above and re-run interactively, or acknowledge non-interactively with --yes --consented-digest <surface_digest from `agentstack trust --preview`>"
        );
    }
    // §7.2: a non-interactive `--yes` must also present the digest of the
    // surface that was reviewed. Without it, "the user saw the review" would
    // be the caller's claim, not a checked fact.
    if !interactive && consented.is_none() {
        anyhow::bail!(
            "refusing to trust: --yes requires --consented-digest — run `agentstack trust --preview`, review the surface, and pass its `surface_digest` back"
        );
    }

    // The funnel asks its single question here — after the complete review,
    // before anything is granted or rendered. A refusal leaves the trust store
    // untouched, exactly like every other refusal on this path.
    if let Some(card) = card {
        if interactive && !yes {
            let said_yes = match card.answer {
                Some(answer) => answer,
                None => super::panel_edit::confirm(&card.question)?,
            };
            if !said_yes {
                anyhow::bail!("cancelled — nothing was granted or activated");
            }
        }
    }

    // `agentstack trust` has no closing confirmation — typing the command at a
    // terminal IS the consent, and that already happened, BEFORE the answers
    // above were given. Without this, N per-item answers would commit with no
    // further yes: exactly the many-moments shape the staging contract exists
    // to prevent. So a re-gate that collected answers asks once, here, at the
    // same point the funnel's card asks. A clean review, or a re-gate the human
    // answered nothing on, prompts nothing and is unchanged.
    if card.is_none() && !answers.is_empty() {
        let summary = answers
            .iter()
            .map(|(ix, a)| {
                format!(
                    "{} {}",
                    match a {
                        Answer::Accept => "accept",
                        Answer::KeepPinned => "keep approved version of",
                        Answer::Block => "block",
                    },
                    crate::text::sanitize_line(&pending[*ix].name)
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        let said_yes = match probe {
            Some(probe) => probe.confirm,
            None => crate::util::confirm::confirm(&format!("\nApply: {summary}?"))?,
        };
        if !said_yes {
            anyhow::bail!("cancelled — nothing was granted or changed");
        }
    }

    // ---- The commit point ----------------------------------------------
    // Everything the answers imply happens from here down, in one place, after
    // every gate. Order matters and is witnessed:
    //
    //   1. accepted items re-pin (deposit → patch lock → write → recompute the
    //      digest), because accept CHANGES the bytes the consent digest covers;
    //   2. the grant records that digest;
    //   3. standing decisions are written LAST — `set_decision` is a no-op
    //      without a trust entry (it must never be a second grant
    //      constructor), so on a first-ever trust an answer written before the
    //      grant would be silently dropped.
    let mut effective_digest = surface_digest.clone();
    let accepted: Vec<&PendingAnswer> = answers
        .iter()
        .filter(|(_, a)| *a == Answer::Accept)
        .map(|(ix, _)| &pending[*ix])
        .collect();
    if !accepted.is_empty() {
        // Patch the lock parsed FROM THE SNAPSHOT — never a manifest-wide
        // re-lock. `agentstack lock` re-pins every kind, which would fold
        // un-consented pin moves — including the very items answered
        // keep-pinned or block — into the digest this grant is about to bless.
        let mut patched = lock_from_snapshot(&snapshot, &dir)?;
        // The review's `store` binding is scoped to the skills block; this is
        // the same path-derived singleton, not a second store.
        let store = crate::store::Store::default_store();
        for p in &accepted {
            let Some(live) = &p.live else { continue };
            let checksum = crate::store::dir_digest(live)?.hex().to_string();
            // Through `Store::pin`, so the newly approved bytes land in the
            // content store and the NEXT re-gate can still show a diff.
            let pinned = store.pin(&crate::store::Resolved {
                path: live.clone(),
                rev: None,
                checksum: checksum.clone(),
                fetched: false,
                source_kind: "path",
            })?;
            if let Some(entry) = patched.skills.iter_mut().find(|s| s.name == p.name) {
                entry.checksum = pinned;
            }
            // The recorded surface must carry the pin the human just approved,
            // or the next re-gate would diff against the superseded one.
            if let Some(item) = diff
                .current
                .iter_mut()
                .find(|i| i.kind == p.kind && i.name == p.name)
            {
                item.pin = Some(checksum);
            }
        }
        patched.save(&dir)?;
        // Recompute, never re-read (§7.2). The manifest and local bytes are the
        // ones this review rendered from; only the lock moved, and these are
        // the exact bytes we just serialized — so a concurrent edit cannot
        // sneak into the digest this grant records. Same precedent `repin`
        // documents: computed from written content, never from a disk re-read.
        let lock_bytes = std::fs::read(crate::lock::Lock::path(&dir)).ok();
        effective_digest = trust::ConsentSnapshot {
            manifest: snapshot.manifest.clone(),
            local: snapshot.local.clone(),
            lock: lock_bytes,
        }
        .digest();
    }

    // Store the reviewed surface alongside the pin so the NEXT re-trust can
    // diff against it (P14). Display metadata only — it does not enter the
    // trust digest, so recording it never re-gates the project. When a
    // consented digest was presented (any mode), the grant is bound to it:
    // the trust crate refuses at the store-write point unless it still
    // matches the bytes on disk. Without one, the grant records the digest
    // of the SNAPSHOT this review rendered — never a fresh disk read — so a
    // mid-review byte swap leaves the project `Changed`, not blessed.
    let recorded_surface = diff.current;
    let digest = match consented {
        Some(consented) => trust::trust_with_consent(base, recorded_surface, consented)?,
        None => {
            trust::trust_reviewed(base, effective_digest.clone(), recorded_surface)?;
            effective_digest
        }
    };

    // Standing answers, last — see the ordering note above.
    for (ix, answer) in &answers {
        let p = &pending[*ix];
        let decision = match answer {
            // Accepting clears any prior standing answer for this item: the new
            // bytes ARE the approved ones now, so there is nothing to keep or
            // refuse.
            Answer::Accept => None,
            Answer::KeepPinned => p
                .approved_pin
                .clone()
                .map(|pin| trust::Decision::KeepPinned { pin }),
            Answer::Block => Some(trust::Decision::Blocked),
        };
        trust::set_decision(base, p.kind, &p.name, decision)?;
    }
    println!(
        "\n{} trusted at {digest}.\nEditing the manifest or lockfile invalidates this — re-run `agentstack trust` after reviewing changes.\nPinned skill/server content that drifts is blocked at use time until re-locked.\nWithdraw anytime with `agentstack trust --revoke`.",
        "✓".green()
    );
    Ok(())
}

/// A re-gate question the review walk STAGED but did not ask.
///
/// The separation is the whole staging contract in one type: the walk records
/// what could be asked, the render happens, and only then is anything asked —
/// and even then, nothing is acted on until the commit point. Answering happens
/// in one place, so there is one place to audit for "did an effect leak early".
struct PendingAnswer {
    /// `"skill"` / `"instruction"` — the surface kind, needed because
    /// `blockers` does not carry one.
    kind: &'static str,
    name: String,
    /// Index into `blockers`; the answer clears exactly this entry.
    blocker_ix: usize,
    /// The pin whose bytes the human previously approved — what `keep pinned`
    /// keeps, and what the shown diff was taken against.
    approved_pin: Option<String>,
    /// The live content directory, re-pinned on `accept`.
    live: Option<PathBuf>,
    headline: String,
}

/// What the human said about one staged question. Distinct from
/// [`trust::Decision`] because `Accept` exists here (it is a thing to do at the
/// commit point) but leaves no standing state to store afterwards.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Accept,
    KeepPinned,
    Block,
}

/// Test seam for the re-gate answer loop, injected exactly like `grant_gated`'s
/// `interactive` probe and `ConsentCard::answer`, and for the same stated
/// reason: a consent path whose answers cannot be driven in a test is a consent
/// path whose staging contract is unverified. Production always passes `None`
/// and prompts.
pub struct ReGateProbe {
    /// Answers by item name; items absent from this list are left undecided,
    /// which must keep their blocker exactly as it is today.
    pub answers: Vec<(String, Answer)>,
    /// What the closing confirmation returns.
    pub confirm: bool,
}

/// The pin a PREVIOUS consent recorded for an item, if any.
///
/// Deliberately preferred over the current lock entry when rendering a re-gate
/// diff: the lock is what drifted, so diffing against it would answer "what
/// changed since the machine last re-pinned" when the reviewer asked "what
/// changed since *I* said yes". Entries recorded before pins existed return
/// `None` and degrade to the honest no-snapshot message.
fn prior_pin_for(prior: &[SurfaceItem], kind: &str, name: &str) -> Option<String> {
    prior
        .iter()
        .find(|i| i.kind == kind && i.name == name)
        .and_then(|i| i.pin.clone())
}

/// The directory a skill's bytes live in right now, without network access or
/// content digesting — read-only, through the same seams activation uses.
/// `None` when the source cannot be located locally (an un-cached git skill),
/// which the caller renders as "no snapshot" rather than guessing.
fn live_skill_dir(
    name: &str,
    m: &crate::manifest::Manifest,
    library: &crate::library::Library,
    // The project's manifest dir — an inline skill's `path` is relative to it.
    dir: &Path,
    lib_home: &Path,
    store: &crate::store::Store,
) -> Option<PathBuf> {
    // An inline declaration always wins over a same-named library skill, which
    // is the same precedence activation applies.
    if let Some(skill) = m.skills.get(name) {
        return store
            .resolve_path_only(skill, dir, None)
            .ok()
            .flatten()
            .map(|r| r.path);
    }
    library.get(name)?.body_dir(lib_home)
}

/// The card: two to five plain lines summarizing the surface that was just
/// walked. Public and pure so a test asserts on exactly what the human sees —
/// the same reason [`policy_requested_lines`] is public.
///
/// It reads the reviewed surface (`ReviewDiff::current`) rather than the
/// manifest, which is deliberate: the card and the detail below it are then
/// provably the same set of items, and any kind added to the review later shows
/// up here without a second place to remember to update. `blocked` is the count
/// of items that failed their pin check, so the pin line is honest on the path
/// where the review ends in a refusal.
///
/// Every value interpolated here is already-sanitized display text or a machine
/// count; hostile names reach this function only through `SurfaceItem.name`,
/// which is sanitized at the point of use below.
pub fn card_summary_lines(items: &[SurfaceItem], blocked: usize) -> Vec<String> {
    // A server's identity is its command line or its URL; that is the only
    // place the two are distinguishable once the surface is flattened.
    let is_url = |s: &str| s.starts_with("http://") || s.starts_with("https://");
    let named = |kind: &str, items: &[SurfaceItem]| -> Vec<String> {
        items
            .iter()
            .filter(|i| i.kind == kind)
            .map(|i| crate::text::sanitize_line(&i.name))
            .collect()
    };

    let mut runs: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "server" && !is_url(&i.identity))
        .map(|i| crate::text::sanitize_line(&i.name))
        .collect();
    // Hooks and extensions are executable too — a card that counted only
    // servers as "runs" would undercount the executable surface, which is the
    // one number a reviewer must not be misled about.
    runs.extend(named("hook", items));
    runs.extend(named("extension", items));

    let contacts: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "server" && is_url(&i.identity))
        .map(|i| {
            let host = i
                .identity
                .split("://")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .unwrap_or(&i.identity);
            crate::text::sanitize_line(host)
        })
        .collect();

    let secrets: Vec<String> = items
        .iter()
        .filter(|i| i.kind == "secrets")
        .flat_map(|i| i.identity.split(", ").map(crate::text::sanitize_line))
        .filter(|s| !s.is_empty())
        .collect();

    let context = named("skill", items).len() + named("instruction", items).len();

    let mut lines = vec!["This project will:".to_string()];
    if !runs.is_empty() {
        lines.push(format!(
            "  run {} on your machine — {}",
            super::count(runs.len(), "command"),
            preview_names(&runs)
        ));
    }
    if !contacts.is_empty() {
        lines.push(format!(
            "  contact {} — {}",
            super::count(contacts.len(), "host"),
            preview_names(&contacts)
        ));
    }
    if !secrets.is_empty() {
        lines.push(format!(
            "  be able to read {} — {}",
            super::count(secrets.len(), "secret"),
            preview_names(&secrets)
        ));
    }
    if context > 0 {
        lines.push(format!(
            "  add {} to every agent's context",
            super::count(context, "file")
        ));
    }
    // A project that declares nothing still gets a card, and it should say so
    // plainly rather than presenting an empty list as if it were a surface.
    if lines.len() == 1 {
        lines.push("  nothing — this project declares no capabilities yet".to_string());
    }
    lines.push(if blocked == 0 {
        "  …using exactly the content shown below, pinned to these bytes.".to_string()
    } else {
        // `count` pluralizes by appending `s`, so the irregular verb branches
        // here, exactly as its doc comment directs.
        format!(
            "  …but {} {} not pinned to reviewed bytes — details below.",
            super::count(blocked, "item"),
            if blocked == 1 { "is" } else { "are" }
        )
    });
    lines
}

/// Name the first few items and count the rest, so a project with forty skills
/// still produces a card a human reads in one glance.
fn preview_names(names: &[String]) -> String {
    const SHOWN: usize = 3;
    if names.len() <= SHOWN {
        return names.join(", ");
    }
    format!(
        "{}, and {} more",
        names[..SHOWN].join(", "),
        names.len() - SHOWN
    )
}

/// Render a settings value with object keys sorted, recursively, so the review
/// diff keys on *meaning* and not on serialization order. Two manifests that
/// declare the same settings with their keys typed in a different order are the
/// same consent; without this they would read as `~ changed` and train the user
/// to wave through a diff that says nothing. Arrays keep their order — element
/// order in a settings list is meaningful (precedence), unlike key order.
fn canonical_json(value: &serde_json::Value) -> String {
    use serde_json::Value;
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_unstable();
            let inner: Vec<String> = keys
                .iter()
                .map(|k| format!("{}:{}", k, canonical_json(&map[*k])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(items) => {
            let inner: Vec<String> = items.iter().map(canonical_json).collect();
            format!("[{}]", inner.join(","))
        }
        other => other.to_string(),
    }
}

/// The review lines for a workflow's approved blueprint (F13): the shape the
/// user signed off as a graph, shown at the gate that actually authorizes the
/// script. Pushes a blocker when the blueprint is unpinned or drifted, because
/// "this is the graph you approved" must be a claim the lockfile can back.
///
/// Blueprint bytes are HOSTILE INPUT — they arrive from a model's output
/// stream through a repo file. Everything here is bounded and fails to a
/// stated "unreadable" rather than a panic or a partial render; a blueprint
/// that will not parse is shown as such, never silently skipped, or its
/// absence would read as "no graph was approved".
fn blueprint_review_lines(
    dir: &Path,
    name: &str,
    declared: &str,
    lock: &crate::lock::Lock,
    blockers: &mut Vec<(String, String)>,
) -> Vec<String> {
    // Anchored at the manifest dir, matching how `[workflows.*].path` and the
    // lock's blueprint pin resolve — see `lock::pin_blueprint`.
    let shown = crate::text::sanitize_line(declared);

    let actual = match agentstack_core::digest::contained_file_digest(dir, declared) {
        Ok(d) => d,
        Err(e) => {
            blockers.push((
                name.to_string(),
                format!("approved blueprint '{declared}' is unreadable: {e}"),
            ));
            return vec![format!(
                "{} approved blueprint {shown} — {}",
                "✗".red(),
                "UNREADABLE".red()
            )];
        }
    };
    match lock.workflows.iter().find(|w| w.name == name) {
        Some(l) if l.blueprint_checksum.as_ref() == Some(&actual) => {}
        Some(_) => {
            blockers.push((
                name.to_string(),
                "approved blueprint drifted from lock — re-review and run `agentstack lock`"
                    .to_string(),
            ));
            return vec![format!(
                "{} approved blueprint {shown} — {}",
                "✗".red(),
                "DRIFTED from lock".red()
            )];
        }
        None => {
            blockers.push((
                name.to_string(),
                "approved blueprint unpinned — run `agentstack lock`".to_string(),
            ));
            return vec![format!(
                "{} approved blueprint {shown} — {}",
                "✗".red(),
                "unpinned".red()
            )];
        }
    }

    let mut out = vec![format!(
        "{} approved blueprint {shown}   [pinned]",
        "◆".cyan()
    )];
    match std::fs::read_to_string(dir.join(declared))
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
    {
        Some(v) => {
            let pattern = v
                .get("pattern")
                .and_then(|p| p.as_str())
                .unwrap_or("custom");
            let goal = v.get("goal").and_then(|g| g.as_str()).unwrap_or("");
            out.push(format!(
                "    pattern: {}{}",
                crate::text::sanitize_line(pattern),
                if goal.is_empty() {
                    String::new()
                } else {
                    format!(" — {}", crate::text::sanitize_line(goal))
                }
            ));
            // Cap the node list: a blueprint is bounded at authoring time, but
            // a repo file is not, and a review surface that can be flooded is
            // a review surface that gets skipped.
            if let Some(nodes) = v.get("nodes").and_then(|n| n.as_array()) {
                for node in nodes.iter().take(16) {
                    let f = |k: &str| {
                        node.get(k)
                            .and_then(|x| x.as_str())
                            .map(crate::text::sanitize_line)
                            .unwrap_or_else(|| "?".into())
                    };
                    let fanout = node
                        .get("fanout")
                        .and_then(|x| x.as_str())
                        .map(|s| format!(" ×{}", crate::text::sanitize_line(s)))
                        .unwrap_or_default();
                    out.push(format!(
                        "    · {} — role {} · {}/{}{fanout}",
                        f("phase"),
                        f("role"),
                        f("model"),
                        f("effort")
                    ));
                }
                if nodes.len() > 16 {
                    out.push(format!(
                        "    · … {}",
                        super::count(nodes.len() - 16, "more node")
                    ));
                }
            }
        }
        None => out.push(format!(
            "    {}",
            "(blueprint is pinned but not readable as JSON — review the file itself)".yellow()
        )),
    }
    // Rule 8: say exactly what the pin does and does not buy. The graph and
    // the script are one consent; nothing here proves the script implements
    // the graph.
    out.push(format!(
        "    {}",
        "the graph and the script are pinned together; agentstack does not verify the script implements the graph"
            .dimmed()
    ));
    out
}

/// Print what the project's `[policy]` requests, per dimension. Bundles can
/// only narrow, so this is review signal, not a gate. Filesystem scopes are
/// labelled honestly: the write scope decides the sandbox workspace mount
/// (ro unless covered); read scopes are informational, and host mode
/// enforces neither.
fn review_policy(p: &crate::manifest::Policy, diff: &mut ReviewDiff, body: &mut Vec<String>) {
    let lines = policy_requested_lines(p);
    if !lines.is_empty() {
        // One aggregate item: any change to the requested set flips the header
        // line to `~ changed`.
        let mk = diff.mark("policy", "", &lines.join("\n"));
        body.push(format!(
            "{mk}policy requested by this project (can only narrow the machine layer):"
        ));
        for line in &lines {
            body.push(line.clone());
        }
    }
    // P15: ALWAYS name the machine policy ceiling file — even for a policy-free
    // repo — so a user consenting learns a machine layer exists and where it
    // lives. Constant machine fact, so no diff marker; honors AGENTSTACK_HOME.
    let ceiling = crate::util::paths::agentstack_home().join("agentstack.toml");
    body.push(format!(
        "  machine policy ceiling: {} — the repo can only narrow it, never loosen it",
        ceiling.display()
    ));
}

/// The requested-policy lines the trust review prints, as a pure builder —
/// public so the regression test asserts on exactly what the human sees.
pub fn policy_requested_lines(p: &crate::manifest::Policy) -> Vec<String> {
    let mut lines = Vec::new();
    let dims: [(&str, &indexmap::IndexMap<String, Vec<String>>); 3] = [
        ("tools", &p.tools),
        ("egress", &p.egress),
        ("secrets", &p.secrets),
    ];
    for (label, map) in dims {
        for (server, rules) in map {
            // Server names and rule strings are manifest content — hostile
            // input; sanitize like every other review line.
            lines.push(format!(
                "  · {label:<7} {}: {}",
                crate::text::sanitize_line(server),
                crate::text::sanitize_line(&rules.join(", "))
            ));
        }
    }
    if !p.filesystem.read.is_empty() {
        lines.push(format!(
            "  · filesystem read {} (informational — the sandbox mounts one whole workspace)",
            crate::text::sanitize_line(&p.filesystem.read.join(", "))
        ));
    }
    if !p.filesystem.write.is_empty() {
        lines.push(format!(
            "  · filesystem write {} (sandbox mode mounts the workspace read-only unless this covers it; advisory in host mode)",
            crate::text::sanitize_line(&p.filesystem.write.join(", "))
        ));
    }
    if !p.filesystem.deny.is_empty() {
        lines.push(format!(
            "  · filesystem deny {} (blocklist — UNIONS with the machine layer; enforced by the host guard)",
            p.filesystem.deny.join(", ")
        ));
    }
    lines
}

/// The skill names a trust review covers: the manifest's inline `[skills.*]`
/// plus every profile-referenced name (which may resolve to the central
/// library), deduped in first-seen order. The `"*"` wildcard expands to inline
/// skills only — the same rule as activation — so it adds nothing new here.
pub(crate) fn review_skill_names(m: &crate::manifest::Manifest) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    let push = |n: &str, names: &mut Vec<String>| {
        if n != "*" && !names.iter().any(|x| x == n) {
            names.push(n.to_string());
        }
    };
    for n in m.skills.keys() {
        push(n, &mut names);
    }
    for p in m.profiles.values() {
        for n in &p.skills {
            push(n, &mut names);
        }
    }
    names
}

fn revoke(base: &Path) -> Result<()> {
    if trust::revoke(base)? {
        println!(
            "{} trust revoked for {} — auto-mode is control-plane only there now.",
            "✓".green(),
            base.display()
        );
    } else {
        println!("{} was not trusted; nothing to revoke.", base.display());
    }
    Ok(())
}

fn list() -> Result<()> {
    let store = TrustStore::load();
    if store.trusted.is_empty() {
        println!("No trusted projects. Grant one with `agentstack trust <dir>`.");
        return Ok(());
    }
    for (path, entry) in &store.trusted {
        let state = trust::check(Path::new(path));
        let (mark, note) = match state {
            TrustState::Trusted => ("✓".green().to_string(), "current".to_string()),
            TrustState::Changed => (
                "⚠".yellow().to_string(),
                "manifest or lockfile changed since trusted — re-run `agentstack trust` there"
                    .to_string(),
            ),
            // An entry exists, so Untrusted can't come back here; kept for
            // completeness.
            TrustState::Untrusted => ("⚠".yellow().to_string(), "stale entry".to_string()),
        };
        println!("  {mark} {path} · {} · {note}", entry.digest);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    // CONSENT WITNESS (Phase 2, the card): the summary must count the whole
    // EXECUTABLE surface, not just servers. Hooks and extensions run commands
    // too, and a card that said "run 1 command" while three things execute is
    // precisely the consent surprise the phase gate counts.
    #[test]
    fn card_counts_hooks_and_extensions_as_things_that_run() {
        let items = vec![
            item("server", "fs", "node fs.js"),
            item("hook", "pre-commit", "PreToolUse runs ./check.sh → *"),
            item("extension", "pi-ext", "./ext.ts"),
        ];
        let lines = card_summary_lines(&items, 0);
        let runs = lines
            .iter()
            .find(|l| l.contains("run "))
            .expect("runs line");
        assert!(runs.contains("3 commands"), "{runs}");
        assert!(runs.contains("fs") && runs.contains("pre-commit") && runs.contains("pi-ext"));
    }

    // An HTTP server is a thing CONTACTED, not a thing run — the two must never
    // be conflated, in either direction.
    #[test]
    fn card_separates_contacted_hosts_from_run_commands() {
        let items = vec![
            item("server", "remote", "https://api.example.com/mcp"),
            item("server", "local", "node local.js"),
        ];
        let lines = card_summary_lines(&items, 0);
        let runs = lines
            .iter()
            .find(|l| l.contains("run "))
            .expect("runs line");
        let contacts = lines
            .iter()
            .find(|l| l.contains("contact "))
            .expect("contacts line");
        assert!(
            runs.contains("1 command") && runs.contains("local"),
            "{runs}"
        );
        // The host, not the whole URL — a path is noise at card altitude.
        assert!(
            contacts.contains("1 host") && contacts.contains("api.example.com"),
            "{contacts}"
        );
        assert!(!contacts.contains("/mcp"), "{contacts}");
    }

    // The card is a summary, and a summary that lies about pinning is worse
    // than no summary. When items failed their pin check the card says so
    // instead of claiming the reviewed bytes are the ones that will be used.
    #[test]
    fn card_pin_line_is_honest_when_items_are_unpinned() {
        let items = vec![item("skill", "greet", "library")];
        let clean = card_summary_lines(&items, 0);
        assert!(
            clean.last().unwrap().contains("pinned to these bytes"),
            "{:?}",
            clean.last()
        );
        let dirty = card_summary_lines(&items, 2);
        let last = dirty.last().unwrap();
        assert!(last.contains("2 items are not pinned"), "{last}");
        let one = card_summary_lines(&items, 1);
        assert!(one.last().unwrap().contains("1 item is not pinned"));
    }

    // Two to five lines: a forty-skill project must still be glanceable, so
    // names elide rather than wrapping the terminal.
    #[test]
    fn card_stays_glanceable_and_elides_long_lists() {
        let items: Vec<SurfaceItem> = (0..40)
            .map(|i| item("server", &format!("srv{i}"), "node x.js"))
            .collect();
        let lines = card_summary_lines(&items, 0);
        assert!(
            lines.len() <= 5,
            "card grew to {} lines: {lines:?}",
            lines.len()
        );
        let runs = lines.iter().find(|l| l.contains("run ")).unwrap();
        assert!(
            runs.contains("40 commands") && runs.contains("and 37 more"),
            "{runs}"
        );
    }

    // A project that declares nothing gets a card that says nothing — never an
    // empty list rendered as though it were a surface.
    #[test]
    fn card_names_the_empty_surface_plainly() {
        let lines = card_summary_lines(&[], 0);
        assert!(lines.iter().any(|l| l.contains("declares no capabilities")));
        assert!(lines.len() <= 5);
    }

    // Settings identity keys on MEANING, not serialization order: re-typing the
    // same object with keys in a different order is the same consent, and must
    // not read as `~ changed` — training a user to wave through diffs that say
    // nothing is how a real change gets waved through too.
    #[test]
    fn settings_identity_ignores_key_order_but_not_values() {
        let a: serde_json::Value = serde_json::json!({"b": 1, "a": {"y": 2, "x": [3, 4]}});
        let b: serde_json::Value = serde_json::json!({"a": {"x": [3, 4], "y": 2}, "b": 1});
        assert_eq!(canonical_json(&a), canonical_json(&b));
        let changed: serde_json::Value = serde_json::json!({"b": 2, "a": {"y": 2, "x": [3, 4]}});
        assert_ne!(canonical_json(&a), canonical_json(&changed));
        // Array ORDER is meaningful (precedence) and must still register.
        let reordered: serde_json::Value = serde_json::json!({"b": 1, "a": {"y": 2, "x": [4, 3]}});
        assert_ne!(canonical_json(&a), canonical_json(&reordered));
    }

    // SECURITY WITNESS (trust granting): the non-interactive consent gate. An
    // agent with shell access must NOT be able to self-trust a repo when stdin
    // is not a terminal — doing so would defeat the untrusted-means-inert gate.
    // Since §7.2, `--yes` alone is not enough either: the acknowledgement must
    // carry the previewed surface digest, or a headless caller could grant a
    // surface nobody reviewed. Tests run without a TTY, so `interactive: false`
    // is the real refusal path; `grant_gated` takes the probe as a parameter so
    // both branches are driven directly. NEVER delete or weaken this test.
    #[test]
    fn non_tty_grant_refuses_without_yes_and_consented_digest() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());

        // A minimal, blocker-free project: one inline HTTP server needs no lock
        // pin, so the review reaches the consent gate with nothing to block on.
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child(".agentstack/agentstack.toml")
            .write_str("version = 1\n[servers.x]\ntype = \"http\"\nurl = \"https://x/mcp\"\n")
            .unwrap();

        // (a) Non-TTY, no --yes: refuse, and the trust store keeps no grant.
        assert!(grant_gated(proj.path(), false, None, false, None).is_err());
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (b) Non-TTY with --yes but NO consented digest: still refuses —
        // the §7.2 binding, not just the acknowledgement, is required.
        let err = grant_gated(proj.path(), true, None, false, None).unwrap_err();
        assert!(format!("{err:#}").contains("--consented-digest"));
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (c) --yes with a WRONG digest: refuses (the trust-crate witness
        // covers the store staying clean; here we prove the CLI wiring).
        assert!(grant_gated(proj.path(), true, Some("sha256:beef"), false, None).is_err());
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (d) --yes with the previewed digest: grants.
        let previewed = trust::digest_for(proj.path()).unwrap();
        grant_gated(proj.path(), true, Some(&previewed), false, None).unwrap();
        assert_eq!(trust::check(proj.path()), TrustState::Trusted);

        std::env::remove_var("AGENTSTACK_HOME");
    }

    fn item(kind: &str, name: &str, identity: &str) -> SurfaceItem {
        SurfaceItem {
            kind: kind.to_string(),
            name: name.to_string(),
            identity: identity.to_string(),
            // The card summary reads kinds and identities, never pins — a pin
            // is for the re-gate diff, not for what the card counts.
            pin: None,
        }
    }

    // P14: the re-trust diff marks each item against the last consented
    // surface. This is the machine-checked form of the feature: same item →
    // plain, new item → added, same key but new identity → changed, and a prior
    // item never re-marked → removed. It also proves flat mode (no prior) marks
    // nothing, so first-trust and older-entry reviews look unchanged.
    #[test]
    fn mark_classifies_added_changed_unchanged_and_removed() {
        // The `git pull` scenario: last time we consented to a safe server and
        // a library skill; now a new `evil` server appears, the safe server's
        // command changed, the skill is unchanged, and an old server is gone.
        let prior = vec![
            item("server", "safe", "node safe.js"),
            item("server", "gone", "node gone.js"),
            item("skill", "greet", "library"),
        ];
        let mut diff = ReviewDiff::new(PriorSurface::Recorded(prior));
        assert!(diff.diffing());

        // Same key + same identity → unchanged (plain two-space indent).
        assert_eq!(diff.mark("skill", "greet", "library"), "  ");
        // Same key + different identity → changed.
        assert_eq!(diff.mark("server", "safe", "node safe.js --new"), "~ ");
        // New key → added — this is the surfaced `evil` server.
        assert_eq!(diff.mark("server", "evil", "sh -c pwn"), "+ ");

        // "gone" was in the prior surface but never re-marked → removed.
        let removed = diff.removed();
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].name, "gone");

        // The accumulated current surface is exactly what would be persisted,
        // in render order.
        assert_eq!(
            diff.current,
            vec![
                item("skill", "greet", "library"),
                item("server", "safe", "node safe.js --new"),
                item("server", "evil", "sh -c pwn"),
            ]
        );
    }

    #[test]
    fn flat_mode_marks_nothing_and_has_no_removals() {
        // First-ever trust (and an older entry with no snapshot) both render
        // flat: every marker is the plain indent, nothing reads as removed, yet
        // the surface is still accumulated for the next re-trust to diff.
        for prior in [PriorSurface::NeverTrusted, PriorSurface::Untracked] {
            let mut diff = ReviewDiff::new(prior);
            assert!(!diff.diffing());
            assert_eq!(diff.mark("server", "anything", "whatever"), "  ");
            assert!(diff.removed().is_empty());
            assert_eq!(diff.current, vec![item("server", "anything", "whatever")]);
        }
    }
}
