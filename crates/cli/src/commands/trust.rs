//! `agentstack trust` — the human gate for the zero-files gateway.
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
        self.current.push(SurfaceItem {
            kind: kind.to_string(),
            name: name.to_string(),
            identity: identity.to_string(),
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
    let servers: Vec<serde_json::Value> =
        crate::resolve::effective_runtime_servers(m, &library, &lib_home, None)
            .into_iter()
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
                            .get_server(&name)
                            .is_some_and(|entry| entry.checksum.hex() == r.checksum),
                    };
                    if !pinned_ok {
                        return serde_json::json!({
                            "name": crate::text::sanitize_line(&name),
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
                        "name": crate::text::sanitize_line(&name),
                        "kind": kind,
                        "target": crate::text::sanitize_line(&target),
                    })
                }
                Err(e) => serde_json::json!({
                    "name": crate::text::sanitize_line(&name),
                    "kind": "unresolvable",
                    "target": crate::text::sanitize_line(&e.to_string()),
                }),
            })
            .collect();

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
    println!(
        "{}",
        serde_json::to_string_pretty(&crate::ui_contract::envelope(out))?
    );
    Ok(())
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
    grant_gated(base, yes, consented, std::io::stdin().is_terminal())
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
fn grant_gated(base: &Path, yes: bool, consented: Option<&str>, interactive: bool) -> Result<()> {
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

    println!(
        "Trusting {} for the zero-files gateway.\n",
        base.display().to_string().bold()
    );

    // P14: when this project was trusted before, mark the review against the
    // surface it last consented to — so a `git pull`'s new `evil` server reads
    // as `+ added` instead of hiding in a flat re-list. First-ever trust (and
    // an older entry that recorded no snapshot) stays the flat full review.
    let prior = trust::prior_surface(base);
    let untracked = matches!(prior, PriorSurface::Untracked);
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
    println!("This project declares — review what auto-mode may run/contact:");
    if servers.is_empty() {
        println!("  (no servers)");
    }
    // Trusting pins the lock bytes into the trust digest, so trusting over a
    // drifted or unpinned surface would bless pins that don't match content
    // (or bless no pin at all). Everything that must be lock-verified at use
    // time therefore has to be pinned and matching BEFORE trust is granted:
    // `agentstack lock` is a prerequisite of `agentstack trust`.
    let mut blockers: Vec<(String, String)> = Vec::new();
    for (name, resolved) in &servers {
        // This review is the consent screen for content that may be hostile —
        // display copies are sanitized; diff identities and lookups stay RAW
        // (two different hostile values must never collide after cleaning).
        let disp = crate::text::sanitize_line(name);
        let r = match resolved {
            Ok(r) => r,
            Err(e) => {
                let mk = diff.mark("server", name, "unresolvable");
                println!(
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
                println!(
                    "{mk}{} {disp}: runs `{}`{origin}",
                    "▶".yellow(),
                    crate::text::sanitize_line(&format!("{command} {args}"))
                );
            }
            ServerType::Http => {
                let url = r.server.url.as_deref().unwrap_or("?");
                let mk = diff.mark("server", name, url);
                println!(
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
        println!(
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
        println!("  local executable content (pinned by current bytes):");
        for (label, status) in &exec_statuses {
            let disp = crate::text::sanitize_line(label);
            // An executable is identified by its path (the label the review
            // shows); byte drift is caught by the verdict below, not the diff.
            let mk = diff.mark("executable", label, label);
            match crate::verify::executable_verdict(status) {
                crate::verify::Verdict::Ok => println!("{mk}· {disp}   [pinned]"),
                crate::verify::Verdict::Unpinned => {
                    println!("{mk}{} {disp}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        label.clone(),
                        "local executable unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                crate::verify::Verdict::Block(why) => {
                    println!("{mk}{} {disp}   [{}]", "✗".red(), why.red());
                    blockers.push((label.clone(), why));
                }
            }
        }
        println!(
            "  (unbound, by design: interpreter/harness binaries from $PATH, and imports outside a declared integrity root)"
        );
    }

    // Native extensions (D6): executable add-on code delivered into a
    // harness's own extension directory. It runs INSIDE the harness process,
    // outside the policy ceiling — the pin is the only governance there is,
    // so unpinned AND drifted both block, like the D3 executable surface.
    if !m.extensions.is_empty() {
        println!(
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
                    println!(
                        "{mk}{} {disp} {dest}   [{origin_word}, pinned]",
                        "▶".yellow()
                    );
                }
                ExtensionLockStatus::MissingLockEntry => {
                    println!(
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
                    println!(
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
                    println!(
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
                ExtensionLockStatus::NotAvailableOffline { .. } => println!(
                    "{mk}{} {disp} {dest}   [{origin_word}, {}]",
                    "▶".yellow(),
                    "offline — pin unverified".yellow()
                ),
                ExtensionLockStatus::ResolveFailed { error } => {
                    println!("{mk}{} {disp} {dest}: {}", "✗".red(), error.red());
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
        println!(
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
                    println!("{mk}{} {disp} {dest}   [pinned]", "▶".yellow());
                }
                WorkflowLockStatus::MissingLockEntry => {
                    println!("{mk}{} {disp} {dest}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        name.clone(),
                        "workflow unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                WorkflowLockStatus::ChecksumDrift { .. } | WorkflowLockStatus::RevDrift { .. } => {
                    println!(
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
                    println!(
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
                WorkflowLockStatus::NotAvailableOffline { .. } => println!(
                    "{mk}{} {disp} {dest}   [{}]",
                    "▶".yellow(),
                    "offline — pin unverified".yellow()
                ),
                WorkflowLockStatus::ResolveFailed { error } => {
                    println!(
                        "{mk}{} {disp} {dest}: {}",
                        "✗".red(),
                        crate::text::sanitize_line(&error).red()
                    );
                    blockers.push((name.clone(), error));
                }
            }
        }
    }

    // Skills, reviewed like servers: name + origin + pin status. Their bodies
    // are exactly the bytes the trust digest does NOT cover, so the pin is
    // the only thing binding what the human reviews to what gets served.
    let skill_names = review_skill_names(m);
    if !skill_names.is_empty() {
        println!("  skills loadable over MCP:");
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
            let mk = diff.mark("skill", name, origin_word);
            match &report.status {
                SkillLockStatus::Matches => {
                    println!("{mk}· {disp}   [{origin_word}, pinned]");
                }
                SkillLockStatus::ChecksumDrift { .. } | SkillLockStatus::RevDrift { .. } => {
                    println!(
                        "{mk}{} {disp}   [{origin_word}, {}]",
                        "✗".red(),
                        "DRIFTED from lock".red()
                    );
                    blockers.push((name.clone(), "skill content drifted from lock".to_string()));
                }
                SkillLockStatus::MissingLockEntry => match report.origin {
                    // An inline skill's bytes live in the repo under review —
                    // unpinned means trusting would leave them ungoverned.
                    Some(SkillOrigin::Inline) => {
                        println!("{mk}{} {disp}   [inline, {}]", "✗".red(), "unpinned".red());
                        blockers.push((
                            name.clone(),
                            "inline skill unpinned — run `agentstack lock`".to_string(),
                        ));
                    }
                    // A library skill's bytes are the user's own curated,
                    // scan-gated content — worth pinning, not worth blocking.
                    _ => println!(
                        "{mk}· {disp}   [{origin_word}, {}]",
                        "unpinned — run `agentstack lock`".yellow()
                    ),
                },
                // Reproducibility can't be checked offline; not a blocker.
                SkillLockStatus::NotAvailableOffline { .. } => println!(
                    "{mk}· {disp}   [{origin_word}, {}]",
                    "offline — pin unverified".yellow()
                ),
                SkillLockStatus::ResolveFailed { error } => {
                    println!(
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
        println!("  instruction fragments (compile into CLAUDE.md / AGENTS.md):");
        for (name, instr) in instructions {
            let disp = crate::text::sanitize_line(name);
            use crate::resolve::InstructionLockStatus;
            // Instructions are keyed by name; there is no finer identity to
            // show, so they only ever read as added or removed.
            let mk = diff.mark("instruction", name, "");
            match crate::resolve::instruction_lock_status(name, instr, &dir, &lock) {
                InstructionLockStatus::Matches => println!("{mk}· {disp}   [pinned]"),
                InstructionLockStatus::ChecksumDrift { .. } => {
                    println!("{mk}{} {disp}   [{}]", "✗".red(), "DRIFTED from lock".red());
                    blockers.push((
                        name.clone(),
                        "instruction content drifted from lock".to_string(),
                    ));
                }
                InstructionLockStatus::MissingLockEntry => {
                    println!("{mk}{} {disp}   [{}]", "✗".red(), "unpinned".red());
                    blockers.push((
                        name.clone(),
                        "instruction unpinned — run `agentstack lock`".to_string(),
                    ));
                }
                InstructionLockStatus::ResolveFailed { error } => {
                    println!(
                        "{mk}{} {disp}: broken ref ({})",
                        "✗".red(),
                        crate::text::sanitize_line(&error)
                    );
                    blockers.push((name.clone(), format!("broken ref — {error}")));
                }
            }
        }
    }

    // Requested policy, shown at the trust boundary (ARCHITECTURE: "review
    // shows … policy changes"). Display-only: a bundle's policy can only
    // narrow — the machine layer caps everything at runtime regardless — so
    // there is nothing here to block on, but the human should see what the
    // repo asks for before blessing it.
    review_policy(&m.policy, &mut diff);

    // P14: anything the last consented surface carried that is gone now. Printed
    // as part of the review (before the blocker bail) so the human sees the full
    // diff. A scoped block ends the borrow of `diff` before its `current` moves.
    {
        let removed = diff.removed();
        if !removed.is_empty() {
            println!("  no longer present (was trusted before):");
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
                println!("{} {label}{detail}", "-".red());
            }
        }
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
        anyhow::bail!(
            "cannot trust {}: its loadable surface isn't fully pinned — {} item(s) need locking or review:\n{}\nRun `agentstack lock`, review the result, then `agentstack trust` again.",
            base.display(),
            blockers.len(),
            lines.join("\n")
        );
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

    // Store the reviewed surface alongside the pin so the NEXT re-trust can
    // diff against it (P14). Display metadata only — it does not enter the
    // trust digest, so recording it never re-gates the project. When a
    // consented digest was presented (any mode), the grant is bound to it:
    // the trust crate refuses at the store-write point unless it still
    // matches the bytes on disk. Without one, the grant records the digest
    // of the SNAPSHOT this review rendered — never a fresh disk read — so a
    // mid-review byte swap leaves the project `Changed`, not blessed.
    let digest = match consented {
        Some(consented) => trust::trust_with_consent(base, diff.current, consented)?,
        None => {
            trust::trust_reviewed(base, surface_digest.clone(), diff.current)?;
            surface_digest
        }
    };
    println!(
        "\n{} trusted at {digest}.\nEditing the manifest or lockfile invalidates this — re-run `agentstack trust` after reviewing changes.\nPinned skill/server content that drifts is blocked at use time until re-locked.\nWithdraw anytime with `agentstack trust --revoke`.",
        "✓".green()
    );
    Ok(())
}

/// Print what the project's `[policy]` requests, per dimension. Bundles can
/// only narrow, so this is review signal, not a gate. Filesystem scopes are
/// labelled honestly: the write scope decides the sandbox workspace mount
/// (ro unless covered); read scopes are informational, and host mode
/// enforces neither.
fn review_policy(p: &crate::manifest::Policy, diff: &mut ReviewDiff) {
    let lines = policy_requested_lines(p);
    if !lines.is_empty() {
        // One aggregate item: any change to the requested set flips the header
        // line to `~ changed`.
        let mk = diff.mark("policy", "", &lines.join("\n"));
        println!("{mk}policy requested by this project (can only narrow the machine layer):");
        for line in &lines {
            println!("{line}");
        }
    }
    // P15: ALWAYS name the machine policy ceiling file — even for a policy-free
    // repo — so a user consenting learns a machine layer exists and where it
    // lives. Constant machine fact, so no diff marker; honors AGENTSTACK_HOME.
    let ceiling = crate::util::paths::agentstack_home().join("agentstack.toml");
    println!(
        "  machine policy ceiling: {} — the repo can only narrow it, never loosen it",
        ceiling.display()
    );
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
        assert!(grant_gated(proj.path(), false, None, false).is_err());
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (b) Non-TTY with --yes but NO consented digest: still refuses —
        // the §7.2 binding, not just the acknowledgement, is required.
        let err = grant_gated(proj.path(), true, None, false).unwrap_err();
        assert!(format!("{err:#}").contains("--consented-digest"));
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (c) --yes with a WRONG digest: refuses (the trust-crate witness
        // covers the store staying clean; here we prove the CLI wiring).
        assert!(grant_gated(proj.path(), true, Some("sha256:beef"), false).is_err());
        assert_eq!(trust::check(proj.path()), TrustState::Untrusted);

        // (d) --yes with the previewed digest: grants.
        let previewed = trust::digest_for(proj.path()).unwrap();
        grant_gated(proj.path(), true, Some(&previewed), false).unwrap();
        assert_eq!(trust::check(proj.path()), TrustState::Trusted);

        std::env::remove_var("AGENTSTACK_HOME");
    }

    fn item(kind: &str, name: &str, identity: &str) -> SurfaceItem {
        SurfaceItem {
            kind: kind.to_string(),
            name: name.to_string(),
            identity: identity.to_string(),
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
