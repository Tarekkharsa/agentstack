//! `agentstack lock` — resolve each profile's skill + server refs
//! (library-aware) and pin them in `agentstack.lock`, WITHOUT rendering
//! configs or materializing skills.
//!
//! The lock-only counterpart of `use <profile> --write`: clean-at-rest repos
//! reference library capabilities by name and keep no generated files, so
//! pinning must not require an activate-then-deactivate dance. Resolution
//! fetches git-backed sources as needed (like `use --write`), and lock entries
//! for names outside the selected profiles are preserved.

use agentstack_core::digest::Sha256Hex;
use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::LockArgs;
use crate::library::Library;
use crate::lock::{Lock, LockedInstructionVariant};
use crate::manifest::Manifest;
use crate::render::{resolve_active_servers, Selection};
use crate::resolve::{ResolveMode, ResolvedServer, ResolvedSkill};

use super::use_profile::{record_lock, resolve_active_skills};

/// The one lockfile verb: plain `lock` pins, `--update` re-resolves git skills
/// first (the old `update` command), `--upgrade` re-resolves an installed
/// vendor pack (the old `upgrade` command). The absorbed implementations are
/// unchanged — this only routes.
///
/// This is the CLI entry point (the only caller is `main`), and it is where the
/// house preview gate lives: every other mutating command previews by default
/// and writes only with `--write`, and `lock` was the single exception. In-crate
/// callers that mean to pin call [`run`] directly, so their deliberate writes
/// are unaffected.
pub fn dispatch(args: &LockArgs, manifest_dir: Option<&Path>) -> Result<()> {
    if let Some(name) = &args.update {
        // Every `lock` write requires `--write`, including this one. `--update`
        // re-fetches git skills and re-pins them through `install::run_update`,
        // which has no preview implementation — so rather than claim a dry run
        // this path cannot perform, refuse honestly and name the command that
        // works. An implied preview would be the false claim invariant 8 bars.
        if !args.write {
            // `--update` takes an optional name (bare `--update` means "all"),
            // so echo back exactly what the user typed.
            let named = match name {
                Some(n) => format!(" {n}"),
                None => String::new(),
            };
            anyhow::bail!(
                "refusing to update pins: `--update` fetches and re-pins, and it has no \
                 preview — re-run with `agentstack lock --update{named} --write`"
            );
        }
        return super::install::run_update(
            &crate::cli::UpdateArgs { name: name.clone() },
            manifest_dir,
        );
    }
    if args.upgrade.is_some() {
        let name = args.upgrade.clone().flatten();
        return super::upgrade::run(
            &crate::cli::UpgradeArgs {
                name,
                all: args.all,
                with_instructions: args.with_instructions,
                yes: args.yes,
                write: args.write,
            },
            manifest_dir,
        );
    }
    if args.write {
        run(args, manifest_dir)
    } else {
        preview(args, manifest_dir)
    }
}

/// Restores `agentstack.lock` to a snapshot when it goes out of scope. The pin
/// pipeline resolves and writes in one pass through helpers several other
/// commands also call, so the honest way to preview it is to run it for real
/// and put the file back byte-for-byte.
///
/// Rust note: this is the RAII/"scope guard" idiom — `Drop` runs on the normal
/// path AND when a `?` unwinds out of the pin pipeline, so an error mid-way can
/// no longer leave a half-written lock behind (today it can). `Drop` cannot
/// return an error, so a failed restore is reported on stderr and nothing else.
struct LockRollback<'a> {
    path: &'a Path,
    /// The bytes to restore; `None` means "the file did not exist".
    snapshot: Option<Vec<u8>>,
}

impl<'a> LockRollback<'a> {
    fn new(path: &'a Path, snapshot: Option<Vec<u8>>) -> Self {
        Self { path, snapshot }
    }
}

impl Drop for LockRollback<'_> {
    fn drop(&mut self) {
        let restored = match &self.snapshot {
            Some(bytes) => std::fs::write(self.path, bytes),
            None => match std::fs::remove_file(self.path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
                other => other,
            },
        };
        if let Err(e) = restored {
            eprintln!(
                "{} could not restore {} after the preview: {e}",
                "⚠".yellow(),
                self.path.display()
            );
        }
    }
}

/// The preview half of the gate: resolve and pin exactly as a write would, show
/// what WOULD move in the house diff style, then put the lockfile back.
///
/// The re-gate consequence is stated BEFORE the write, not after it — the
/// lockfile bytes feed the trust digest, so a bare `lock` used to make a live
/// grant stale with no warning the user could act on.
pub fn preview(args: &LockArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    refuse_invalid_manifest(&ctx)?;

    let trust_base = crate::manifest::project_root_of(&ctx.dir);
    crate::intake::print_notice(&ctx.dir, &trust_base, &ctx.loaded.manifest);
    let was_trusted = crate::trust::check(&trust_base) == crate::trust::TrustState::Trusted;

    let lock_path = Lock::path(&ctx.dir);
    let before = std::fs::read(&lock_path).ok();
    let existed = before.is_some();
    // Armed for the whole of `pin_all`, including its error paths, and never
    // committed — a preview always puts the file back.
    //
    // Honest about its limit: this is a restore, not a transaction. A SIGKILL
    // (or a power loss) between `pin_all`'s write and this `Drop` leaves the
    // freshly pinned lockfile on disk. That is fail-closed, not a hole: the
    // lockfile feeds the trust digest, so unexpected bytes make the project
    // untrusted/changed and every gated path refuses until the user re-reviews
    // with `agentstack trust .`. Nothing activates off a lock nobody consented
    // to.
    let _guard = LockRollback::new(&lock_path, before.clone());
    let pinned = pin_all(&ctx, args)?;
    let after = std::fs::read(&lock_path).ok();
    let changed = before != after;
    // The rendered lane is deliberately NOT run here: it writes instruction
    // files, and a preview writes nothing.
    let before_text = before.map(|b| String::from_utf8_lossy(&b).into_owned());
    let after_text = after.map(|b| String::from_utf8_lossy(&b).into_owned());

    println!(
        "{} lock preview: {} from {} in {}",
        "→".cyan(),
        pinned.summary,
        pinned.from,
        lock_path.display()
    );
    // Three states, three different truths. Saying "already matches this
    // project" when there is no lockfile at all claims a file exists — the
    // absent-file cases get their own words.
    match (changed, existed) {
        (true, existed) => {
            if !existed {
                println!("  agentstack.lock does not exist yet — these would be its first pins.");
            }
            let rendered = crate::util::diff::render(
                before_text.as_deref().unwrap_or(""),
                after_text.as_deref().unwrap_or(""),
            );
            for line in rendered.lines() {
                println!("  {line}");
            }
        }
        (false, true) => {
            println!("  no pin changes — agentstack.lock already matches this project.");
        }
        (false, false) => {
            println!(
                "  nothing to pin — this project declares no skills, servers, instructions, \
                 extensions, workflows, or packages, so no agentstack.lock would be written."
            );
        }
    }
    if let Some(notice) = relock_trust_preview_notice(was_trusted, changed) {
        println!("{} {notice}", "⚠".yellow());
    }
    // Say the quiet part: computing this preview ran the real resolver, so git
    // sources were fetched and their bytes deposited in the content store. Only
    // `agentstack.lock` is left unchanged — calling that a pure "dry run"
    // without this line would overclaim.
    println!("  (resolved sources to compute this — git-backed sources were fetched.)");
    println!(
        "\nDry run: nothing was pinned. Re-run with {} to pin these.",
        "--write".bold()
    );
    Ok(())
}

/// P9 heads-up: the lockfile is part of a project's consent surface (its bytes
/// feed the trust digest), so re-locking a *currently trusted* project whose
/// pins actually change invalidates that trust — new pins are new consent, and
/// trust must be re-granted. Returns the one-line notice when that's the case,
/// or `None` (untrusted project, or a byte-identical re-lock that changes
/// nothing). Kept pure so the decision is unit-testable without a live project.
pub(crate) fn relock_trust_notice(was_trusted: bool, pins_changed: bool) -> Option<&'static str> {
    if was_trusted && pins_changed {
        Some(
            "this project is trusted — new pins are new consent, so its trust is now stale; \
             re-review and re-grant with `agentstack trust .`",
        )
    } else {
        None
    }
}

/// The same P9 consequence as [`relock_trust_notice`], stated in the future
/// tense for the preview — the whole point of the gate is that the user reads
/// it while the grant is still live and can decide not to write.
pub(crate) fn relock_trust_preview_notice(
    was_trusted: bool,
    pins_change: bool,
) -> Option<&'static str> {
    if was_trusted && pins_change {
        Some(
            "this project is trusted — new pins are new consent, so writing this will make \
             its trust stale; re-review with `agentstack trust .` after",
        )
    } else {
        None
    }
}

/// What a pin pass moved, in the words the summary line uses.
struct Pinned {
    /// e.g. `2 skills + 1 server`, or `nothing new`.
    summary: String,
    /// e.g. `2 toolsets`, or the implicit-default phrasing.
    from: String,
}

/// N5: refuse a manifest that cannot validate, BEFORE pinning anything.
///
/// The lockfile is part of the consent surface, so `lock` is followed by
/// `trust .` — and without this gate an invalid manifest pinned cleanly,
/// printed a green ✓, and sent the user into a consent ceremony for a
/// bundle that could never be admitted; the refusal only surfaced later at
/// `workflow run`. A trust prompt that turns out to have been pointless is
/// exactly how a consent gate gets trained into a reflex click, so the
/// failure moves to the cheapest correct place. Library-aware, and the same
/// issue set / message / fix text `doctor` and `apply` already produce —
/// one rule set, three call sites, no third dialect for the user to learn.
fn refuse_invalid_manifest(ctx: &super::Context) -> Result<()> {
    let manifest = &ctx.loaded.manifest;
    let libctx = ctx.library_ctx();
    let vctx = libctx.validate_ctx(&ctx.dir);
    let target_ids: Vec<&str> = ctx.registry.ids().collect();
    let errors: Vec<_> = crate::manifest::validate_with_context(manifest, target_ids, &vctx)
        .into_iter()
        .filter(|i| i.kind.is_error())
        .collect();
    if !errors.is_empty() {
        let detail = errors
            .iter()
            .map(|i| match &i.fix {
                Some(fix) => format!("{}\n    ↳ {fix}", i.message),
                None => i.message.clone(),
            })
            .collect::<Vec<_>>()
            .join("\n  ");
        let pronoun = if errors.len() == 1 { "it" } else { "them" };
        anyhow::bail!(
            "refusing to lock: {} — pinning {pronoun} would put content in the \
             consent surface that can never be admitted:\n  {detail}",
            super::count(errors.len(), "validation error")
        );
    }
    Ok(())
}

/// Resolve and pin everything this project declares. Shared by the write path
/// ([`run`]) and the preview path ([`preview`]) so the two can never disagree
/// about what a write would do: the preview runs exactly this and then rolls
/// the lockfile back.
fn pin_all(ctx: &super::Context, args: &LockArgs) -> Result<Pinned> {
    let manifest = &ctx.loaded.manifest;

    // Instructions are manifest-global, not profile-scoped: pin them
    // regardless of the profile selection (and even with zero profiles). The
    // lock command is strict — an unreadable fragment errors, stale pins for
    // undeclared names are pruned.
    let instructions = record_instruction_pins(&ctx.dir, manifest, true)?;

    let library = Library::load_default()?;
    let lib_home = crate::util::paths::lib_home();
    let store = crate::store::Store::default_store();

    // The D3 executable surface is manifest-global too: it derives from the
    // EFFECTIVE runtime server set (inline `[servers.*]` fan-out included),
    // not from profiles — a profile-less manifest still declares runnable
    // local code, and the trust gate blocks unpinned executables, so `lock`
    // must be able to pin them or a profile-less project could never be
    // trusted at all.
    let executables = record_executable_pins(&ctx.dir, manifest, &library, &lib_home)?;

    // Native extensions (D6) are manifest-global like instructions: pin them
    // regardless of the profile selection. Strict — an undigestable or
    // unresolvable source is an error, stale pins for undeclared names are
    // pruned. Resolution is library-aware and fetches git sources.
    let extensions = record_extension_pins(&ctx.dir, manifest, &library, &lib_home, &store)?;

    // Governed workflows (D7 W1) are manifest-global like extensions: pin them
    // regardless of the profile selection. Strict — an undigestable or
    // sourceless entry is an error, stale pins for undeclared names are
    // pruned. Git sources are fetched through the shared store.
    let workflows = record_workflow_pins(&ctx.dir, manifest, &store)?;

    // Profile selection mirrors activation: named → that one; default → every
    // declared profile; none declared → the implicit default (the full inline
    // set), so a profile-less manifest is fully pinnable.
    let profiles: Option<Vec<String>> = match &args.profile {
        Some(p) => {
            manifest
                .profiles
                .get(p)
                .with_context(|| {
                    format!("no toolset '{p}' in this project — `agentstack toolset list` shows the ones declared here")
                })?;
            Some(vec![p.clone()])
        }
        None if manifest.profiles.is_empty() => None,
        None => Some(manifest.profiles.keys().cloned().collect()),
    };

    // W5: expand every package the selected toolsets reference into exact,
    // digest-pinned members. Done BEFORE `record_lock` for the same reason
    // resolution happens before any write — the expansion is strict, and a
    // package that cannot be expanded must leave the lock untouched rather
    // than half-written.
    let packages = record_package_pins(
        &ctx.dir,
        manifest,
        &library,
        &lib_home,
        &store,
        profiles.as_deref().unwrap_or(&[]),
    )?;

    let (skills, servers) = match &profiles {
        Some(profiles) => {
            resolve_profiles(manifest, &ctx.dir, &library, &lib_home, &store, profiles)?
        }
        None => resolve_implicit_default(manifest, &ctx.dir, &library, &lib_home, &store)?,
    };
    record_lock(&ctx.dir, &skills, &servers, manifest, &library)?;

    let from = match &profiles {
        Some(p) => super::count(p.len(), "toolset"),
        None => "the implicit default (no toolsets declared)".to_string(),
    };
    // Count only what actually pinned — six "+ 0 <jargon>(s)" segments turned
    // the beginner journey's first success line into a wall of internals.
    let mut pinned_parts: Vec<String> = Vec::new();
    for (n, what) in [
        (skills.len(), "skill"),
        (servers.len(), "server"),
        (instructions, "instruction"),
        (executables, "executable pin"),
        (extensions, "extension"),
        (workflows, "workflow"),
        (packages, "package"),
    ] {
        if n > 0 {
            pinned_parts.push(super::count(n, what));
        }
    }
    let pinned_summary = if pinned_parts.is_empty() {
        "nothing new".to_string()
    } else {
        pinned_parts.join(" + ")
    };
    Ok(Pinned {
        summary: pinned_summary,
        from,
    })
}

pub fn run(args: &LockArgs, manifest_dir: Option<&Path>) -> Result<()> {
    let ctx = super::load(manifest_dir)?;
    let manifest = &ctx.loaded.manifest;
    refuse_invalid_manifest(&ctx)?;

    // P9: snapshot the consent state *before* pinning. If this project is
    // currently trusted, changing its pins re-gates it — we surface that the
    // moment the change is written, so the user is never silently left with
    // stale trust. `check` recomputes the digest against the trust store; the
    // lockfile bytes are what we compare afterward to know pins actually moved.
    let trust_base = crate::manifest::project_root_of(&ctx.dir);
    // `lock` pins what the manifest declares. Anything dropped into the intake
    // dirs but not declared is NOT pinned by this run, so say so here rather
    // than let the user infer that a green lock covered it.
    crate::intake::print_notice(&ctx.dir, &trust_base, manifest);
    let was_trusted = crate::trust::check(&trust_base) == crate::trust::TrustState::Trusted;
    // The same "before we write" reading, in the form the render gates take.
    // Captured HERE, beside the P9 snapshot and before `pin_all`, because the
    // lock bytes are part of the consent digest: the rendered lane below
    // compiles package prose in the same run, and without this the command
    // would refuse the very delivery it was typed to make
    // (`render::PriorTrust`).
    let prior = crate::render::PriorTrust::at_command_start(&ctx.dir);
    let lock_before = std::fs::read(Lock::path(&ctx.dir)).ok();

    let Pinned {
        summary: pinned_summary,
        from,
    } = pin_all(&ctx, args)?;

    // P9: did the pins actually move? Compare the lockfile bytes to the
    // pre-pin snapshot (the record_* helpers only rewrite the lock when
    // something changed, so this is an exact "pins changed" signal). When they
    // did and the project was trusted, warn that trust is now stale.
    let lock_after = std::fs::read(Lock::path(&ctx.dir)).ok();
    if let Some(notice) = relock_trust_notice(was_trusted, lock_before != lock_after) {
        println!("{} {notice}", "⚠".yellow());
    }

    // W5, the rendered lane. A package's instruction members are pinned above;
    // this is where the pinned bytes reach a file. Deliberately BEFORE the
    // `args.quiet` early return: quiet suppresses narration, never a write.
    let rendered_lane = render_package_instructions(&ctx, prior)?;

    if args.quiet {
        // Composed into the funnel's single card: the pin happened, and the
        // card says so in its own words. A second summary plus a competing
        // "Next:" is exactly the three-screens experience slice B removes.
        println!("  {} pinned {pinned_summary}", "✓".green());
        // A file written is never suppressed, however quiet the caller asked
        // to be — the rendered lane is a project artifact, not narration.
        for line in &rendered_lane {
            println!("  {line}");
        }
        return Ok(());
    }
    println!(
        "{} pinned {pinned_summary} from {from} in {}",
        "✓".green(),
        Lock::path(&ctx.dir).display()
    );
    for line in &rendered_lane {
        println!("  {line}");
    }
    // The claim narrows when a package carried house rules: instructions ARE
    // rendered on that path, and one blended "nothing was rendered" sentence
    // beside a `rendered lane:` line is exactly the dishonesty the delivery
    // contract's copy rules forbid.
    if rendered_lane.is_empty() {
        println!(
            "  no configs rendered, no skills materialized — that stays `agentstack use --write`."
        );
    } else {
        println!(
            "  no server configs rendered, no skills materialized — that stays `agentstack use --write`."
        );
    }
    let target_ids: Vec<String> = ctx.registry.ids().map(str::to_string).collect();
    let mode = super::overview::detect_mode(&ctx, &target_ids);
    let trust = crate::trust::check(&trust_base);
    match (mode, trust) {
        (
            super::overview::Mode::CleanAtRest | super::overview::Mode::ZeroFiles,
            crate::trust::TrustState::Untrusted | crate::trust::TrustState::Changed,
        ) => println!("\nNext: `agentstack trust .` to review and consent."),
        (super::overview::Mode::CleanAtRest, crate::trust::TrustState::Trusted) => {
            let profile = if manifest.profiles.len() == 1 {
                manifest
                    .profiles
                    .keys()
                    .next()
                    .map(String::as_str)
                    .unwrap_or("<toolset>")
            } else {
                "<toolset>"
            };
            println!("\nNext: `agentstack x session start {profile}` to load it for this session.");
        }
        (super::overview::Mode::ZeroFiles, crate::trust::TrustState::Trusted) => {
            println!("\nNext: `agentstack doctor` to verify the gateway wiring.");
        }
        (super::overview::Mode::Static, _) => {
            println!("\nNext: `agentstack use --write` to activate the pinned capabilities.");
        }
    }
    Ok(())
}

/// Digest every project-declared instruction fragment and pin it in the lock.
/// Machine-layer (`from_user_layer`) fragments never pin — they are the user's
/// own machine content, not repo bytes under review. Returns how many pinned.
///
/// `strict` (the `agentstack lock` command): an unreadable fragment is an
/// error (can't pin what can't be read), and pins for names no longer declared
/// are pruned. Non-strict (`apply --write` / `instructions --write` first-pin
/// recording): unreadable fragments are skipped — the compile machinery
/// already reports and blocks them per target — and nothing is pruned.
pub(crate) fn record_instruction_pins(
    dir: &Path,
    manifest: &Manifest,
    strict: bool,
) -> Result<usize> {
    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    // Same rule as `record_lock` for skills: a fragment under a standing
    // re-gate answer keeps the pin the answer named. Re-pinning it here would
    // absorb the declined live bytes into the lock with no consent moment.
    // It stays in `declared`, so strict pruning keeps its existing pin.
    let decided = super::use_profile::decided_names(dir, "instruction");
    // Sourceless fragments resolve their bodies from the linked sources; one
    // read serves the whole pass.
    let library = crate::library::Library::load_default_or_warn();
    let mut declared: Vec<String> = Vec::new();
    let mut pinned = 0usize;
    for (name, instr) in &manifest.instructions {
        if instr.from_user_layer {
            continue;
        }
        declared.push(name.clone());
        if decided.contains(name) {
            continue;
        }
        // Every body this fragment declares — the base and each per-(CLI, model)
        // variant. Pinning only the currently-selected one would leave the
        // others unpinned content the consent digest never covers, which is the
        // hole `docs/design/instruction-variants.md` §"Every variant body is
        // pinned" exists to close.
        let bodies = match crate::instructions::bodies(name, instr, dir, &library) {
            Ok(b) => b,
            Err(e) if strict => {
                return Err(e).context(format!("pinning instruction '{name}'"));
            }
            Err(_) => continue,
        };
        // The pin comes from `Store::pin_instruction`, which deposits the bytes
        // it hashes into the content store as part of producing the checksum —
        // so a later re-gate can show WHICH LINES of this fragment moved
        // instead of only that it changed. This is the sole production site
        // that builds a LockedInstruction, so routing it here makes the
        // deposit a property of pinning rather than of call-site discipline,
        // exactly as `Store::pin` does for skills.
        let store = crate::store::Store::default_store();
        let base_src = bodies.source_of(&bodies.base);
        match store.pin_instruction(&base_src) {
            Ok(checksum) => {
                // A variant that cannot be pinned is an error in strict mode and
                // skipped otherwise — the same posture the base body takes, so
                // one unreadable variant never silently drops the whole entry.
                let mut variants = Vec::new();
                for v in &bodies.variants {
                    let src = bodies.source_of(&v.path);
                    match store.pin_instruction(&src) {
                        Ok(checksum) => variants.push(LockedInstructionVariant {
                            cli: v.cli.clone(),
                            model: v.model.clone(),
                            path: v.path.clone(),
                            checksum,
                        }),
                        Err(e) if strict => {
                            return Err(e).with_context(|| {
                                format!(
                                    "pinning instruction '{name}' variant: reading {}",
                                    src.display()
                                )
                            });
                        }
                        Err(_) => {}
                    }
                }
                lock.upsert_instruction(agentstack_core::lock::LockedInstruction {
                    name: name.clone(),
                    path: bodies.base.clone(),
                    checksum,
                    variants,
                });
                pinned += 1;
            }
            Err(e) if strict => {
                return Err(e).with_context(|| {
                    format!(
                        "pinning instruction '{name}': reading {}",
                        base_src.display()
                    )
                });
            }
            Err(_) => {}
        }
    }
    if strict {
        lock.retain_instruction_names(&declared);
    }
    // Don't churn the lockfile (or the trust digest) for a byte-identical pin.
    if lock != before {
        lock.save(dir)?;
    }
    Ok(pinned)
}

/// Pin the D3 executable surface of the EFFECTIVE runtime server set (inline
/// fan-out + every profile-referenced name; the same set the trust preview,
/// doctor, and a locked run verify). Strict like the instruction pins: an
/// unverifiable local candidate (symlink, traversal, broken declared root) is
/// an error, and stale pins are PRUNED — a removed server or un-declared
/// integrity root must not leave a dead pin masking the surface (mirror of
/// `retain_instruction_names`; the profile-scoped `record_lock` first-pin path
/// never prunes, since it only sees a subset of servers). Unresolvable server
/// refs are skipped here — the profile resolution below (or the use path)
/// reports those; their existing pins are retained, never pruned on a broken
/// resolution. Returns how many pinned.
pub(crate) fn record_executable_pins(
    dir: &Path,
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
) -> Result<usize> {
    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    let mut pinned = 0usize;
    let mut keep: Vec<(String, agentstack_core::lock::ExecutableKind)> = Vec::new();
    let mut all_resolved = true;
    for (name, resolved) in
        crate::resolve::effective_runtime_servers(manifest, library, lib_home, None)
    {
        let Ok(r) = resolved else {
            all_resolved = false;
            continue;
        };
        for pin in crate::executable::derive_executable_pins(dir, &name, &r.server)? {
            keep.push((pin.path.clone(), pin.kind));
            lock.upsert_executable(pin);
            pinned += 1;
        }
    }
    // Prune only from a complete picture: if any server failed to resolve, its
    // executable surface is unknown, and pruning would drop live pins.
    if all_resolved {
        lock.retain_executables(&keep);
    }
    // Don't churn the lockfile (or the trust digest) for byte-identical pins.
    if lock != before {
        lock.save(dir)?;
    }
    Ok(pinned)
}

/// Pin every declared native extension (D6) by the STRICT integrity-root
/// digest — the executable-content family (symlink anywhere = hard error,
/// `.git` included), never the lenient skill digest. Resolution is inline-first
/// then central library, and git sources are fetched through the shared store
/// (`ResolveMode::Fetch`), exactly like `agentstack lock` resolves skills.
///
/// Always strict, like the lock command's other manifest-global pins: an
/// undigestable or unresolvable source errors, and pins for undeclared names
/// are pruned. Records the full source provenance (`source`/`path`/`git`/`rev`)
/// so the pin is self-describing and a git rev-drift is detectable. Returns how
/// many pinned.
pub(crate) fn record_extension_pins(
    dir: &Path,
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
) -> Result<usize> {
    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    let mut declared: Vec<String> = Vec::new();
    let mut pinned = 0usize;
    for (name, ext) in &manifest.extensions {
        declared.push(name.clone());
        let resolved = crate::resolve::resolve_extension_entry(
            name,
            ext,
            dir,
            library,
            lib_home,
            store,
            ResolveMode::Fetch,
        )
        .with_context(|| format!("pinning extension '{name}'"))?;
        lock.upsert_extension(agentstack_core::lock::LockedExtension {
            name: name.clone(),
            target: resolved.target.clone(),
            source: resolved.source_kind.to_string(),
            path: resolved.path.clone(),
            git: resolved.git.clone(),
            rev: resolved.rev.clone(),
            checksum: resolved.checksum,
        });
        pinned += 1;
    }
    lock.retain_extension_names(&declared);
    // Don't churn the lockfile (or the trust digest) for byte-identical pins.
    if lock != before {
        lock.save(dir)?;
    }
    Ok(pinned)
}

/// Pin every declared governed workflow (D7 W1) by the STRICT integrity-root
/// digest — the executable-content family, same as extensions, never the
/// lenient skill digest. Sources are inline-only (`path` or `git`; the
/// central-library workflow kind is W4), and git sources are fetched through
/// the shared store (`ResolveMode::Fetch`).
///
/// Always strict, like the lock command's other manifest-global pins: an
/// undigestable, sourceless, or unresolvable entry errors, and pins for
/// undeclared names are pruned. The pin records the sorted-unique `roles` set
/// alongside the provenance — the review binds this script to these
/// capability sets, so a later roles change is drift even with unchanged
/// bytes. Returns how many pinned.
/// Digest a declared blueprint path (F13), with the same containment rules the
/// rest of the integrity surface uses: `contained_file_digest` refuses a path
/// that escapes its anchor or passes through a symlink anywhere, so a repo
/// cannot point "the approved graph" at a file outside the bundle or swap it
/// via a link after review. The blueprint is a single JSON file, so this is
/// the file-level digest rather than the directory walk a script source gets.
///
/// Anchored at the MANIFEST dir, not the project root — `[workflows.*].path`
/// resolves that way (`resolve_workflow_entry` passes `manifest_dir` to
/// `integrity_root_digest`), and the blueprint sits beside the script it
/// belongs to. Anchoring the two differently would make `./workflows/x.js` and
/// `./workflows/x.blueprint.json` mean two different directories.
fn pin_blueprint(dir: &Path, name: &str, declared: &str) -> Result<Sha256Hex> {
    agentstack_core::digest::contained_file_digest(dir, declared).with_context(|| {
        format!("pinning the approved blueprint '{declared}' for workflow '{name}'")
    })
}

pub(crate) fn record_workflow_pins(
    dir: &Path,
    manifest: &Manifest,
    store: &crate::store::Store,
) -> Result<usize> {
    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    let mut declared: Vec<String> = Vec::new();
    let mut pinned = 0usize;
    for (name, wf) in &manifest.workflows {
        declared.push(name.clone());
        let resolved =
            crate::resolve::resolve_workflow_entry(name, wf, dir, store, ResolveMode::Fetch)
                .with_context(|| format!("pinning workflow '{name}'"))?;
        // F13: the approved blueprint is pinned BESIDE the script, so one
        // consent covers both and editing either re-gates the project. Strict
        // like every other pin — a declared blueprint that cannot be read is
        // an error, never a silently-dropped pin, or "approved graph" would
        // become a claim the lockfile does not actually carry.
        let blueprint_checksum = match &wf.blueprint {
            Some(rel) => Some(pin_blueprint(dir, name, rel)?),
            None => None,
        };
        lock.upsert_workflow(agentstack_core::lock::LockedWorkflow {
            name: name.clone(),
            roles: resolved.roles.clone(),
            source: resolved.source_kind.to_string(),
            path: resolved.path.clone(),
            git: resolved.git.clone(),
            rev: resolved.rev.clone(),
            checksum: Sha256Hex::parse(&resolved.checksum)
                .with_context(|| format!("pinning workflow '{name}'"))?,
            blueprint: wf.blueprint.clone(),
            blueprint_checksum,
        });
        pinned += 1;
    }
    lock.retain_workflow_names(&declared);
    // Don't churn the lockfile (or the trust digest) for byte-identical pins.
    if lock != before {
        lock.save(dir)?;
    }
    Ok(pinned)
}

/// Expand each package a **selected** toolset references and pin its exact
/// member set (W5, `docs/design/package-layer.md`). Returns how many packages
/// pinned.
///
/// Two scoping rules, and they are different on purpose:
///
/// - **Expansion is scoped to the selected toolsets.** `lock --profile backend`
///   re-reads and re-pins only what `backend` names, exactly as it re-resolves
///   only that toolset's skills and servers.
/// - **Pruning is scoped to every DECLARED toolset.** A package another toolset
///   still selects keeps its pin untouched; a package no toolset names any
///   more loses it. Pruning against the selected subset instead would silently
///   drop another toolset's expansion — a member set nothing re-verifies is
///   exactly the stale pin `retain_instruction_names` exists to prevent, and
///   here it would also be a member set the runtime resolves from.
///
/// Strict like the lock command's other pins: an unknown package, a body that
/// is not on this machine, a `pack.toml` that drifted from its index pin, a
/// member path that escapes the package, a package carrying executable kinds,
/// and every stale or ill-typed override are errors — and all of them happen
/// before a single byte of the lock is rewritten.
pub(crate) fn record_package_pins(
    dir: &Path,
    manifest: &Manifest,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
    selected_toolsets: &[String],
) -> Result<usize> {
    let selections = crate::package::selected_packages(manifest, selected_toolsets);
    let keep: Vec<String> = crate::package::all_selected_packages(manifest)
        .into_keys()
        .collect();
    // Nothing selects a package and nothing is pinned: leave the lock entirely
    // alone rather than loading and re-saving it for a no-op.
    if selections.is_empty() && keep.is_empty() && Lock::load(dir)?.packages.is_empty() {
        return Ok(0);
    }
    let expanded =
        crate::package::expand_selected(manifest, dir, library, lib_home, store, &selections)?;

    let mut lock = Lock::load(dir)?;
    let before = lock.clone();
    let pinned = expanded.len();
    for entry in expanded {
        lock.upsert_package(entry);
    }
    lock.retain_package_names(&keep);
    // Don't churn the lockfile (or the trust digest) for byte-identical pins.
    if lock != before {
        lock.save(dir)?;
    }
    Ok(pinned)
}

/// Compile this project's **pinned** package instruction members into the
/// managed instruction region, and describe on the rendered lane what happened.
/// Returns the report lines (empty when the project has no package instruction
/// member at all, which is the overwhelmingly common case).
///
/// Two rules, both inherited rather than invented:
///
/// - **Conservative scoping**, exactly as `upgrade::rerender_managed_regions`
///   applies it (W3): only a target whose instruction file ALREADY carries the
///   managed region is written. Locking a package must never be the reason a
///   `CLAUDE.md` first appears in a repo, or the reason an existing one first
///   gains an agentstack region — that is a decision a human makes by running
///   `apply` or `instructions --write`. `manages_file` is that whole rule.
/// - **Region merging is `render::merge_md`'s job**, reached through
///   `plan_instructions`, so prose outside the markers survives untouched and
///   there is exactly one implementation of the region contract.
///
/// The lane vocabulary is binding: this is the RENDERED lane. A package's
/// instruction member is never described as going live "via gateway", because
/// it does not — it goes into a file, and this says which one.
///
/// `prior` MUST be the trust state captured before `pin_all` wrote the
/// lockfile. A package's prose is the project's content and the compile is
/// gated on trust like any other fragment's — but the lock bytes are part of
/// the consent digest, so pinning flips a trusted project to `Changed` and this
/// render, in the same run, would refuse the very delivery the human typed
/// `lock --write` to get. A command cannot be allowed to refuse itself
/// (`render::PriorTrust`). A project that was untrusted or already drifted when
/// the command STARTED is still refused, and nothing is re-pinned, so the next
/// command re-gates it.
fn render_package_instructions(
    ctx: &super::Context,
    prior: crate::render::PriorTrust,
) -> Result<Vec<String>> {
    let pinned = Lock::load(&ctx.dir).unwrap_or_default();
    let members =
        crate::package::members_of_kind(&pinned, crate::lock::PackageMemberKind::Instruction, None);
    if members.is_empty() {
        return Ok(Vec::new());
    }
    let manifest = &ctx.loaded.manifest;
    let packages = crate::package::effective_members(&pinned);
    let scope = crate::scope::Scope::default_for(&ctx.dir);
    let target_ids = crate::render::resolve_targets(manifest, &ctx.registry, &[], &ctx.dir)?;

    let sel = crate::instructions::Selecting::for_command(None);
    let mut written: Vec<String> = Vec::new();
    let mut unverifiable: Vec<String> = Vec::new();
    let mut refused: Vec<String> = Vec::new();
    for id in &target_ids {
        let Some(desc) = ctx.registry.get(id) else {
            continue;
        };
        let Some(plan) = crate::render::instructions::plan_instructions(
            manifest, desc, scope, &ctx.dir, packages, &sel, prior,
        ) else {
            continue;
        };
        if !crate::render::instructions::manages_file(&plan.path) {
            continue;
        }
        // The trust gate. Reported like an unverifiable member rather than
        // raised: `lock` pinned successfully, and failing the whole command
        // over a render it was right to withhold would hide that.
        if let Some(why) = &plan.refusal {
            if !refused.contains(why) {
                refused.push(why.clone());
            }
            continue;
        }
        // A member whose pinned bytes are missing or fail verification lands in
        // `missing`; writing then would silently delete its prose from the
        // region. Fail closed to "not written", and say so.
        for name in &plan.missing {
            if !unverifiable.contains(name) {
                unverifiable.push(name.clone());
            }
        }
        if !plan.missing.is_empty() || !plan.changed() {
            continue;
        }
        let display = plan
            .path
            .strip_prefix(&ctx.dir)
            .unwrap_or(&plan.path)
            .display()
            .to_string();
        plan.write()
            .with_context(|| format!("rendering {}", plan.path.display()))?;
        if !written.contains(&display) {
            written.push(display);
        }
    }

    let count = super::count(members.len(), "package house-rule fragment");
    let mut lines = Vec::new();
    if written.is_empty() && !refused.is_empty() {
        // The honest negative when the GATE is what held the write back —
        // never the "no managed region here" sentence below, which would send
        // the user to a command that will refuse for the same reason.
        lines.push(format!(
            "rendered lane: {count} pinned; nothing was written — the project has not been \
             trusted for this content"
        ));
    } else if written.is_empty() {
        // The honest negative, and the one command that would render it.
        lines.push(format!(
            "rendered lane: {count} pinned; no instruction file here carries agentstack's \
             managed region, so no file was written"
        ));
        lines.push(
            "  ↳ `agentstack instructions --write` renders the region into CLAUDE.md / AGENTS.md"
                .to_string(),
        );
    } else {
        lines.push(format!(
            "rendered lane: {count} pinned; managed region updated in {}",
            written.join(", ")
        ));
    }
    if !unverifiable.is_empty() {
        lines.push(format!(
            "  ↳ {} could not be served from the content store — re-run `agentstack lock --write`",
            unverifiable.join(", ")
        ));
    }
    for why in &refused {
        lines.push(format!("  ↳ {why}"));
    }
    Ok(lines)
}

/// Resolve the named profiles' skill + server refs through the library-aware
/// resolvers (inline-first, then central library), deduplicated by name across
/// profiles. Fails before any lock write if a ref resolves nowhere.
fn resolve_profiles(
    manifest: &Manifest,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
    profiles: &[String],
) -> Result<(Vec<ResolvedSkill>, Vec<ResolvedServer>)> {
    let mut seen_skills = BTreeSet::new();
    let mut seen_servers = BTreeSet::new();
    let mut skills = Vec::new();
    let mut servers = Vec::new();
    for pname in profiles {
        for r in resolve_active_skills(
            manifest,
            Some(pname),
            dir,
            library,
            lib_home,
            store,
            ResolveMode::Fetch,
        )? {
            if seen_skills.insert(r.name.clone()) {
                skills.push(r);
            }
        }
        let selection = Selection::Profile(pname.clone());
        for r in resolve_active_servers(manifest, library, lib_home, &selection)? {
            if seen_servers.insert(r.name.clone()) {
                servers.push(r);
            }
        }
    }
    // Every DECLARED skill is pinned too, not only the ones a toolset names.
    //
    // The trust gate reviews `[skills]` in full — its card calls them "skills
    // loadable over MCP", because loadability follows the declaration, not
    // toolset membership — and it REFUSES while any inline one is unpinned.
    // Pinning only the toolsets' selection left a manifest that declares a
    // skill outside every toolset in a state no command could leave: `doctor`
    // named `lock --write`, `lock --write` reported success while pinning
    // nothing for it, and `trust` refused again on the same item. Adding the
    // declared set here makes the pin set a SUPERSET of what it was: nothing
    // that used to be pinned stops being pinned, and the gate's demand and the
    // pinning verb now read one list.
    //
    // This pass is manifest-wide even under `--profile`. The flag narrows the
    // toolset selection (servers and packages still narrow), but the trust gate
    // reviews `[skills]` in full, so narrowing here would re-create exactly the
    // stuck state above for anyone who pins with `--profile`. The flag's help
    // says so; see `LockArgs::profile`.
    //
    // It is ADDITIVE and therefore LENIENT, unlike the strict toolset walk
    // above. Running it strictly made `lock` fail harder than the bug it
    // fixed: one unresolvable declared skill aborted the command before
    // `record_lock`, so a toolset skill that resolved fine pinned NOTHING.
    // Per-name recovery (rather than "run it after the write") keeps the
    // lockfile a coherent whole: `record_lock` still writes one set, in one
    // pass, all-or-nothing for the selection the user asked for, and the extra
    // declared names either join that set or are reported as problems.
    //
    // Honest disclosure (E): resolution here is `ResolveMode::Fetch`, so `lock`
    // may fetch a git source for a skill that no toolset selects — the
    // declared `[skills]` table only, since that is precisely what this walk
    // enumerates; a name that exists only inside a toolset is already covered
    // by the per-toolset loop above. It is the same action `lock` already
    // performs for selected skills, and the preview path runs the same
    // pipeline and discloses it, but it does widen the set of URLs an
    // untrusted manifest can make the host contact before the trust gate.
    let mut problems: Vec<String> = Vec::new();
    for name in manifest.skills.keys() {
        if seen_skills.contains(name) {
            continue;
        }
        match resolve_declared_skill(manifest, dir, library, lib_home, store, name) {
            Ok(r) => {
                seen_skills.insert(r.name.clone());
                skills.push(r);
            }
            Err(e) => problems.push(format!("{name}: {e:#}")),
        }
    }
    for p in &problems {
        println!("{} declared skill not pinned — {p}", "⚠".yellow());
    }
    Ok((skills, servers))
}

/// Resolve one declared `[skills.*]` entry the same way the shared walk does
/// (library-aware, fetching, and requiring the body to be present on disk),
/// but as a single fallible unit so one broken declaration cannot abort the
/// pins its neighbours earned.
fn resolve_declared_skill(
    manifest: &Manifest,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
    name: &str,
) -> Result<ResolvedSkill> {
    let resolved = crate::resolve::resolve_skill_with_pin(
        manifest,
        dir,
        library,
        lib_home,
        store,
        name,
        ResolveMode::Fetch,
        None,
    )
    .with_context(|| format!("resolving declared skill '{name}'"))?;
    // This walk resolved in `ResolveMode::Fetch`, so a library or git body
    // has already been materialised by the time we look. A path that still
    // does not exist is genuinely absent, and `agentstack install` cannot
    // conjure it — pointing there sent the reader to a command that refuses.
    // Say the two things a human can actually do instead. (`use --write`
    // keeps its own pointer: its resolve mode is a parameter and may be
    // offline, where installing IS the fix.)
    if !resolved.path.exists() {
        anyhow::bail!(
            "declared body at {} is not present on disk — restore it, or \
             remove `[skills.{name}]` from the manifest; no command can \
             install or pin a body that is not there",
            resolved.path.display()
        );
    }
    Ok(resolved)
}

/// The pin set for a profile-less manifest: every inline skill and server —
/// exactly what `use --write` would activate as the implicit default.
fn resolve_implicit_default(
    manifest: &Manifest,
    dir: &Path,
    library: &Library,
    lib_home: &Path,
    store: &crate::store::Store,
) -> Result<(Vec<ResolvedSkill>, Vec<ResolvedServer>)> {
    let skills = resolve_active_skills(
        manifest,
        None,
        dir,
        library,
        lib_home,
        store,
        ResolveMode::Fetch,
    )?;
    let servers = resolve_active_servers(manifest, library, lib_home, &Selection::All)?;
    Ok((skills, servers))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{LibraryServer, LibrarySkill};
    use crate::store::Store;
    use assert_fs::prelude::*;

    /// Write a path-source skill body + a server definition under `lib_home`
    /// and index both in the returned library.
    fn library_with(lib_home: &assert_fs::TempDir) -> Library {
        lib_home
            .child("skills/sql-review/SKILL.md")
            .write_str("# lib\n")
            .unwrap();
        lib_home
            .child("servers/kibana.toml")
            .write_str(
                "type = \"http\"\nurl = \"https://x/mcp\"\n\n[headers]\nAuthorization = \"Bearer ${TOKEN}\"\n",
            )
            .unwrap();
        let mut lib = Library::default();
        lib.upsert(LibrarySkill {
            name: "sql-review".into(),
            source: "path".into(),
            path: Some("sql-review".into()),
            git: None,
            rev: None,
            subpath: None,
            checksum: None,
            version: None,
            provenance: Some("consolidated".into()),
        });
        lib.upsert_server(LibraryServer {
            name: "kibana".into(),
            checksum: None,
            version: None,
            provenance: Some("consolidated:codex".into()),
        });
        lib
    }

    #[test]
    fn resolves_and_pins_all_profiles_without_materializing() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with(&lib_home);

        // Two profiles sharing a skill: dedup keeps one entry; servers come
        // from the second profile only.
        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.review]
            skills = ["sql-review"]
            [profiles.ops]
            skills = ["sql-review"]
            servers = ["kibana"]
            "#,
        )
        .unwrap();
        let profiles: Vec<String> = manifest.profiles.keys().cloned().collect();

        let (skills, servers) = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &profiles,
        )
        .unwrap();
        assert_eq!(skills.len(), 1, "shared skill deduplicated across profiles");
        assert_eq!(servers.len(), 1);

        record_lock(proj.path(), &skills, &servers, &manifest, &library).unwrap();

        let lock = Lock::load(proj.path()).unwrap();
        let skill = lock.get("sql-review").expect("skill pinned");
        assert_eq!(skill.checksum.hex().len(), 64);
        let server = lock.get_server("kibana").expect("server pinned");
        assert_eq!(server.source, agentstack_core::lock::ServerSource::Library);
        // Lock-only: nothing was rendered or materialized in the project.
        assert!(!proj.child(".mcp.json").path().exists());
        assert!(!proj.child(".claude").path().exists());
        // And never a secret value — the definition digest only.
        let text = std::fs::read_to_string(Lock::path(proj.path())).unwrap();
        assert!(!text.contains("Bearer"));
    }

    #[test]
    fn lock_pins_local_executables_and_declared_roots() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = Library::default();

        proj.child("scripts/entry.py")
            .write_str("import x")
            .unwrap();
        proj.child("tools/lib.py").write_str("v1").unwrap();

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [servers.agent]
            type = "stdio"
            command = "python"
            args = ["./scripts/entry.py"]
            integrity_roots = ["tools"]
            [profiles.dev]
            servers = ["agent"]
            "#,
        )
        .unwrap();

        let (skills, servers) = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &["dev".to_string()],
        )
        .unwrap();
        record_lock(proj.path(), &skills, &servers, &manifest, &library).unwrap();

        let lock = Lock::load(proj.path()).unwrap();
        use agentstack_core::lock::ExecutableKind;
        let file = lock
            .get_executable("scripts/entry.py", ExecutableKind::File)
            .expect("entry script pinned");
        assert_eq!(file.checksum.hex().len(), 64);
        let root = lock
            .get_executable("tools", ExecutableKind::Root)
            .expect("declared root pinned");
        assert_eq!(root.checksum.hex().len(), 64);

        // The one-byte re-gate chain: an edit inside the declared root makes
        // a re-lock rewrite the pin (new checksum → new lock bytes → the
        // trust digest flips via the existing chain).
        proj.child("tools/lib.py").write_str("v2").unwrap();
        record_lock(proj.path(), &skills, &servers, &manifest, &library).unwrap();
        let relocked = Lock::load(proj.path()).unwrap();
        assert_ne!(
            relocked
                .get_executable("tools", ExecutableKind::Root)
                .unwrap()
                .checksum,
            root.checksum
        );
    }

    #[test]
    fn removing_a_server_or_root_prunes_its_executable_pins() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let library = Library::default();
        proj.child("tool.sh").write_str("echo").unwrap();
        proj.child("tools/lib.py").write_str("v1").unwrap();

        let with_root: Manifest = toml::from_str(
            "version = 1\n[servers.agent]\ntype = \"stdio\"\ncommand = \"./tool.sh\"\nintegrity_roots = [\"tools\"]\n",
        )
        .unwrap();
        record_executable_pins(proj.path(), &with_root, &library, lib_home.path()).unwrap();
        assert_eq!(Lock::load(proj.path()).unwrap().executables.len(), 2);

        // Un-declaring the root prunes its pin; the command pin survives.
        let without_root: Manifest = toml::from_str(
            "version = 1\n[servers.agent]\ntype = \"stdio\"\ncommand = \"./tool.sh\"\n",
        )
        .unwrap();
        record_executable_pins(proj.path(), &without_root, &library, lib_home.path()).unwrap();
        let lock = Lock::load(proj.path()).unwrap();
        assert_eq!(lock.executables.len(), 1);
        use agentstack_core::lock::ExecutableKind;
        assert!(lock
            .get_executable("tool.sh", ExecutableKind::File)
            .is_some());
        assert!(lock.get_executable("tools", ExecutableKind::Root).is_none());

        // Removing the server entirely prunes everything.
        let empty: Manifest = toml::from_str("version = 1\n").unwrap();
        record_executable_pins(proj.path(), &empty, &library, lib_home.path()).unwrap();
        assert!(Lock::load(proj.path()).unwrap().executables.is_empty());
    }

    // E1 witness (D6): a one-byte edit to an extension's source fails strict
    // locked verification before launch, and re-locking rewrites the pin —
    // new checksum → new lock bytes → the trust digest flips via the existing
    // chain, forcing re-review.
    #[test]
    fn one_byte_extension_edit_refuses_locked_and_relock_regates() {
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child("extensions/checkpoint/index.ts")
            .write_str("export default function (pi) {} // v1")
            .unwrap();

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [extensions.checkpoint]
            path = "./extensions/checkpoint"
            target = "pi"
            "#,
        )
        .unwrap();

        let library = Library::default();
        let lib_home = proj.child("lib").path().to_path_buf();
        let store = crate::store::Store::with_root(proj.child("store").path().to_path_buf());

        record_extension_pins(proj.path(), &manifest, &library, &lib_home, &store).unwrap();
        let lock = Lock::load(proj.path()).unwrap();
        let pinned = lock.get_extension("checkpoint").expect("pinned").clone();
        assert_eq!(pinned.checksum.len(), 64);
        assert_eq!(pinned.target, "pi");
        assert_eq!(pinned.source, "path");
        assert_eq!(pinned.path.as_deref(), Some("./extensions/checkpoint"));

        let ext = &manifest.extensions["checkpoint"];
        let status = |lock: &Lock| {
            crate::resolve::extension_lock_status(
                "checkpoint",
                ext,
                proj.path(),
                &library,
                &lib_home,
                &store,
                lock,
                crate::resolve::ResolveMode::NoFetch,
            )
            .status
        };
        assert_eq!(status(&lock), crate::resolve::ExtensionLockStatus::Matches);
        let clean = vec![("checkpoint".to_string(), status(&lock))];
        assert!(crate::verify::ensure_locked_inputs("pi", &[], &[], &[], &[], &clean).is_ok());

        // One byte changes → strict verification refuses, naming the extension.
        proj.child("extensions/checkpoint/index.ts")
            .write_str("export default function (pi) {} // v2")
            .unwrap();
        let drifted = vec![("checkpoint".to_string(), status(&lock))];
        assert!(matches!(
            drifted[0].1,
            crate::resolve::ExtensionLockStatus::ChecksumDrift { .. }
        ));
        let err = crate::verify::ensure_locked_inputs("pi", &[], &[], &[], &[], &drifted)
            .unwrap_err()
            .to_string();
        assert!(err.contains("extension 'checkpoint'"), "{err}");

        // Re-locking accepts the edit by rewriting the pin: the lock bytes
        // change, which is exactly what flips the trust digest.
        let before = std::fs::read(Lock::path(proj.path())).unwrap();
        record_extension_pins(proj.path(), &manifest, &library, &lib_home, &store).unwrap();
        let after = std::fs::read(Lock::path(proj.path())).unwrap();
        assert_ne!(before, after, "accepting drift must change the lock bytes");

        // Retargeting without re-locking blocks too — the pin bound the code
        // to one harness.
        let retargeted: Manifest = toml::from_str(
            r#"
            version = 1
            [extensions.checkpoint]
            path = "./extensions/checkpoint"
            target = "opencode"
            "#,
        )
        .unwrap();
        let lock = Lock::load(proj.path()).unwrap();
        let status = crate::resolve::extension_lock_status(
            "checkpoint",
            &retargeted.extensions["checkpoint"],
            proj.path(),
            &library,
            &lib_home,
            &store,
            &lock,
            crate::resolve::ResolveMode::NoFetch,
        )
        .status;
        assert!(matches!(
            status,
            crate::resolve::ExtensionLockStatus::TargetDrift { .. }
        ));

        // Removing the declaration prunes its pin (stale-pin rule).
        let empty: Manifest = toml::from_str("version = 1\n").unwrap();
        record_extension_pins(proj.path(), &empty, &library, &lib_home, &store).unwrap();
        assert!(Lock::load(proj.path())
            .unwrap()
            .get_extension("checkpoint")
            .is_none());
    }

    // P9 witness: the re-lock trust notice fires only when a currently-trusted
    // project's pins actually change — never for an untrusted project, and never
    // for a byte-identical re-lock (which rewrites nothing and leaves trust
    // valid).
    #[test]
    fn relock_notice_only_when_trusted_and_pins_change() {
        assert!(relock_trust_notice(true, true).is_some());
        assert!(relock_trust_notice(true, false).is_none());
        assert!(relock_trust_notice(false, true).is_none());
        assert!(relock_trust_notice(false, false).is_none());
    }

    /// The preview states the SAME consequence before the write, in the future
    /// tense, and under the same two conditions.
    #[test]
    fn preview_notice_warns_before_the_write() {
        let notice = relock_trust_preview_notice(true, true).expect("warned");
        assert!(notice.contains("writing this will make"), "{notice}");
        assert!(notice.contains("agentstack trust ."), "{notice}");
        assert!(relock_trust_preview_notice(true, false).is_none());
        assert!(relock_trust_preview_notice(false, true).is_none());
    }

    /// The preview's rollback puts the lockfile back exactly as it was —
    /// including the "there was no lockfile" case, which must not leave one.
    #[test]
    fn rollback_restores_the_lockfile_snapshot() {
        let proj = assert_fs::TempDir::new().unwrap();
        let path = proj.child("agentstack.lock").path().to_path_buf();

        std::fs::write(&path, b"original").unwrap();
        {
            let _g = LockRollback::new(&path, Some(b"original".to_vec()));
            std::fs::write(&path, b"rewritten by the pin pass").unwrap();
        }
        assert_eq!(std::fs::read(&path).unwrap(), b"original");

        std::fs::remove_file(&path).unwrap();
        {
            let _g = LockRollback::new(&path, None);
            std::fs::write(&path, b"first pin").unwrap();
        }
        assert!(!path.exists(), "a preview never leaves a first lockfile");
    }

    #[test]
    fn single_profile_selection_locks_only_its_refs() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with(&lib_home);

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.review]
            skills = ["sql-review"]
            [profiles.ops]
            servers = ["kibana"]
            "#,
        )
        .unwrap();

        let (skills, servers) = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &["review".to_string()],
        )
        .unwrap();
        assert_eq!(skills.len(), 1);
        assert!(servers.is_empty(), "other profile's servers not resolved");
    }

    #[test]
    fn broken_ref_fails_before_any_lock_write() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = Library::default(); // empty — nothing resolves

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.p]
            skills = ["nope"]
            "#,
        )
        .unwrap();

        let err = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &["p".to_string()],
        )
        .unwrap_err();
        assert!(err.to_string().contains("nope"));
        assert!(!Lock::path(proj.path()).exists(), "no partial lock written");
    }

    /// A skill declared outside every toolset pins too, and it pins even when
    /// `--profile` narrows the selection (the C decision: skills are
    /// manifest-wide because the trust gate reviews `[skills]` in full). The
    /// written order is by name, so the consent-material bytes are stable.
    #[test]
    fn declared_skill_outside_every_toolset_still_pins() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with(&lib_home);
        proj.child("skills/zeta/SKILL.md")
            .write_str("# z\n")
            .unwrap();

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.review]
            skills = ["sql-review"]
            [skills.zeta]
            path = "skills/zeta"
            "#,
        )
        .unwrap();

        let (skills, _servers) = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            // Exactly what `lock --profile review` selects.
            &["review".to_string()],
        )
        .unwrap();
        let mut names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        names.sort();
        assert_eq!(names, vec!["sql-review", "zeta"]);

        record_lock(proj.path(), &skills, &[], &manifest, &library).unwrap();
        let lock = Lock::load(proj.path()).unwrap();
        let written: Vec<String> = lock.skills.iter().map(|s| s.name.clone()).collect();
        assert_eq!(written, vec!["sql-review".to_string(), "zeta".to_string()]);
    }

    /// Regression: the additive declared pass must never abort the pins the
    /// toolset selection already earned.
    #[test]
    fn broken_declared_skill_does_not_abort_toolset_pins() {
        let proj = assert_fs::TempDir::new().unwrap();
        let lib_home = assert_fs::TempDir::new().unwrap();
        let store = Store::with_root(proj.child("store").path().to_path_buf());
        let library = library_with(&lib_home);

        let manifest: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.docs]
            skills = ["sql-review"]
            [skills.broken]
            path = "skills/broken"
            "#,
        )
        .unwrap();

        let (skills, _servers) = resolve_profiles(
            &manifest,
            proj.path(),
            &library,
            lib_home.path(),
            &store,
            &["docs".to_string()],
        )
        .unwrap();
        let names: Vec<&str> = skills.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["sql-review"], "broken one skipped, not fatal");

        record_lock(proj.path(), &skills, &[], &manifest, &library).unwrap();
        let lock = Lock::load(proj.path()).unwrap();
        assert!(
            lock.get("sql-review").is_some(),
            "toolset skill still pinned"
        );
    }
}
