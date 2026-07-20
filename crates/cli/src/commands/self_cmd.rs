//! `agentstack self` — manage this binary's own installation.
//!
//! Until a release is published (see RELEASING.md) people run source builds
//! (`target/release/agentstack`) behind hand-rolled shell functions, which
//! break in non-interactive/agent shells. `self link` gives the built binary a
//! stable name on PATH — one symlink, same directory choice as install.sh —
//! and `self which` explains which binary a bare `agentstack` actually runs,
//! flagging stale links after a rebuild or move.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::CommandFactory;
use owo_colors::OwoColorize;

use crate::cli::{Cli, SelfArgs, SelfCommand, SelfDocsArgs, SelfLinkArgs};

/// The binary name a shell looks up on PATH (`agentstack.exe` on Windows).
pub fn bin_name() -> String {
    format!("agentstack{}", std::env::consts::EXE_SUFFIX)
}

pub fn run(args: &SelfArgs) -> Result<()> {
    match &args.command {
        SelfCommand::Link(link) => run_link(link),
        SelfCommand::Which => run_which(),
        SelfCommand::Docs(docs) => run_docs(docs),
    }
}

// ── `self docs` — the self-compiling command reference ──────────────────────
//
// The "All commands" inventory in docs/reference.md used to be hand-written and
// rotted repeatedly. It's now generated from the clap tree: [`commands_block`]
// renders the block, `--write` splices it into a managed HTML-comment region,
// and a docs-freshness test (tests/docs_commands.rs) asserts the region matches
// this generator byte-for-byte, so it can never drift.

/// The HTML-comment markers bracketing the generated inventory in
/// docs/reference.md. Everything outside them is preserved on `--write`.
pub const COMMANDS_MARKER_BEGIN: &str = "<!-- agentstack:generated commands -->";
pub const COMMANDS_MARKER_END: &str = "<!-- agentstack:end -->";

/// Render the "All commands" inventory from the live clap tree — one line per
/// top-level command, in declaration order, with its one-line summary, visible
/// nested subcommands, and non-global long-form flags. Deterministic: shared by
/// `self docs` and the docs-freshness test so the doc region tracks the CLI.
pub fn commands_block() -> String {
    let cli = Cli::command();
    let mut lines: Vec<String> = Vec::new();
    for sc in cli.get_subcommands() {
        let name = sc.get_name();
        if name == "help" {
            continue;
        }
        // Hidden top-level commands are still real surface — list them, marked.
        let hidden = if sc.is_hide_set() { " _(hidden)_" } else { "" };
        // `get_about()` is the short help (first paragraph); collapse any wrap
        // newlines so each command renders as a single clean line.
        let summary = sc
            .get_about()
            .map(|a| a.to_string())
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let mut line = format!("- **`{name}`**{hidden} — {summary}");

        // Visible nested subcommands, declaration order. `is_hide_set()` is
        // clap's runtime read of `#[command(hide = true)]` — the same signal
        // that keeps a verb out of `--help`.
        let subs: Vec<&str> = sc
            .get_subcommands()
            .filter(|s| s.get_name() != "help" && !s.is_hide_set())
            .map(|s| s.get_name())
            .collect();
        if !subs.is_empty() {
            line.push_str(&format!(" — subcommands `{}`", subs.join("/")));
        }

        // Non-global, non-hidden long flags. `filter_map(get_long)` drops
        // positionals (no long) automatically; `help`/`version` are clap's
        // auto-args, not part of the surface.
        let flags: Vec<String> = sc
            .get_arguments()
            .filter(|a| !a.is_global_set() && !a.is_hide_set())
            .filter_map(|a| a.get_long())
            .filter(|l| *l != "help" && *l != "version")
            .map(|l| format!("--{l}"))
            .collect();
        if !flags.is_empty() {
            line.push_str(&format!(" — flags `{}`", flags.join("/")));
        }
        lines.push(line);
    }
    lines.join("\n")
}

fn run_docs(args: &SelfDocsArgs) -> Result<()> {
    let block = commands_block();
    if !args.write {
        println!("{block}");
        return Ok(());
    }
    let path = reference_md_path();
    let doc =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let updated = splice_commands(&doc, &block)?;
    if updated == doc {
        println!("{} {} already up to date", "✓".green(), path.display());
    } else {
        std::fs::write(&path, &updated).with_context(|| format!("writing {}", path.display()))?;
        println!(
            "{} regenerated the command inventory in {}",
            "✓".green(),
            path.display()
        );
    }
    Ok(())
}

/// Replace the text between the managed markers with `block`, leaving the
/// markers and everything outside them untouched. Fails with a clear message
/// if either marker is missing (so a mangled doc can't be silently half-written).
pub fn splice_commands(doc: &str, block: &str) -> Result<String> {
    let begin = doc.find(COMMANDS_MARKER_BEGIN).with_context(|| {
        format!("docs/reference.md is missing the `{COMMANDS_MARKER_BEGIN}` marker")
    })?;
    let end = doc.find(COMMANDS_MARKER_END).with_context(|| {
        format!("docs/reference.md is missing the `{COMMANDS_MARKER_END}` marker")
    })?;
    if end < begin {
        anyhow::bail!("docs/reference.md markers are out of order (end before begin)");
    }
    let content_start = begin + COMMANDS_MARKER_BEGIN.len();
    Ok(format!(
        "{}\n{}\n{}",
        &doc[..content_start],
        block,
        &doc[end..]
    ))
}

/// docs/reference.md, anchored at the repo root the same way
/// tests/docs_commands.rs does (`CARGO_MANIFEST_DIR` is `crates/cli`, so the
/// repo root is two levels up). `self docs` is a source-tree maintainer command.
fn reference_md_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/reference.md")
}

fn run_link(args: &SelfLinkArgs) -> Result<()> {
    let exe = running_exe()?;
    let dir = install_dir(args.prefix.as_deref())?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    let dest = dir.join(bin_name());

    match link_into(&exe, &dest, args.force)? {
        LinkOutcome::AlreadyLinked => println!(
            "{} {} already runs this binary",
            "✓".green(),
            dest.display()
        ),
        LinkOutcome::Created => println!(
            "{} linked {} → {}",
            "✓".green(),
            dest.display(),
            exe.display()
        ),
        LinkOutcome::Replaced(old) => println!(
            "{} re-linked {} → {} (was {})",
            "✓".green(),
            dest.display(),
            exe.display(),
            old.display()
        ),
    }
    if !on_path(&dir) {
        println!("Add to PATH:  export PATH=\"{}:$PATH\"", dir.display());
    }
    println!(
        "Harness configs registered with `agentstack gateway connect` use this stable \
         path on their next `connect --write`."
    );
    Ok(())
}

fn run_which() -> Result<()> {
    let exe = running_exe()?;
    println!("running:  {}", exe.display());

    let hits = find_all_on_path(&bin_name());
    let Some(first) = hits.first() else {
        println!("on PATH:  (not found)");
        println!(
            "  {} a bare `agentstack` won't run from other shells ↳ agentstack self link",
            "⚠".yellow()
        );
        return Ok(());
    };
    match first.canonicalize() {
        Ok(target) if target == exe => {
            println!("on PATH:  {} → {}", first.display(), target.display());
            println!("  {} PATH runs this binary", "✓".green());
        }
        Ok(target) => {
            println!("on PATH:  {} → {}", first.display(), target.display());
            println!(
                "  {} stale: a bare `agentstack` runs a different binary than this one \
                 ↳ agentstack self link",
                "⚠".yellow()
            );
        }
        Err(_) => {
            println!("on PATH:  {} (broken link)", first.display());
            println!(
                "  {} broken: the PATH entry no longer resolves ↳ agentstack self link",
                "⚠".yellow()
            );
        }
    }
    for shadowed in &hits[1..] {
        println!("shadowed: {}", shadowed.display());
    }
    Ok(())
}

/// The running binary, fully resolved (same pattern as connect's
/// `bridge_command`) — the link must point at the real file, never at itself.
fn running_exe() -> Result<PathBuf> {
    std::env::current_exe()
        .context("cannot determine the running executable")?
        .canonicalize()
        .context("cannot resolve the running executable's real path")
}

/// Where to link: explicit `--prefix`, else `$AGENTSTACK_PREFIX`, else the
/// same choice as install.sh — `/usr/local/bin` when writable, else
/// `~/.local/bin`.
fn install_dir(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(p.to_path_buf());
    }
    if let Some(p) = std::env::var_os("AGENTSTACK_PREFIX") {
        if !p.is_empty() {
            return Ok(PathBuf::from(p));
        }
    }
    let usr_local = PathBuf::from("/usr/local/bin");
    if dir_writable(&usr_local) {
        return Ok(usr_local);
    }
    Ok(dirs::home_dir()
        .context("no home directory")?
        .join(".local/bin"))
}

#[derive(Debug)]
enum LinkOutcome {
    AlreadyLinked,
    Created,
    /// Replaced an existing entry; carries what it used to point at.
    Replaced(PathBuf),
}

/// Point `dest` at `exe`. A dest that already resolves to `exe` is a no-op.
/// Without `--force`, only a dest that looks like a previous self-link is
/// replaced (that's the rebuild/upgrade path): a symlink whose target is an
/// agentstack binary or no longer exists (see [`symlink_is_ours`]), or a
/// copied link carrying our marker (see [`copy_marker`]). Anything else — a
/// symlink somebody pointed at an unrelated tool, or a foreign regular file —
/// needs `--force`.
fn link_into(exe: &Path, dest: &Path, force: bool) -> Result<LinkOutcome> {
    if let Ok(meta) = std::fs::symlink_metadata(dest) {
        if dest.canonicalize().ok().as_deref() == Some(exe) {
            return Ok(LinkOutcome::AlreadyLinked);
        }
        if meta.file_type().is_symlink() {
            if !force && !symlink_is_ours(dest) {
                let target = std::fs::read_link(dest).unwrap_or_else(|_| dest.to_path_buf());
                anyhow::bail!(
                    "{} is a symlink to {} — not an agentstack binary; re-point it with --force",
                    dest.display(),
                    target.display()
                );
            }
        } else if !force && !copy_marker(dest).exists() {
            anyhow::bail!(
                "{} exists and is not a symlink — not overwriting a real file without --force",
                dest.display()
            );
        }
        let old = std::fs::read_link(dest).unwrap_or_else(|_| dest.to_path_buf());
        std::fs::remove_file(dest).with_context(|| format!("removing {}", dest.display()))?;
        // Drop any stale copy marker; `make_link` re-creates it on platforms
        // that copy instead of symlinking.
        let _ = std::fs::remove_file(copy_marker(dest));
        make_link(exe, dest)?;
        return Ok(LinkOutcome::Replaced(old));
    }
    make_link(exe, dest)?;
    Ok(LinkOutcome::Created)
}

/// True when the existing symlink at `dest` is safe to re-point without
/// `--force`: its target's file name is the agentstack binary name (a
/// previous build of us — where it lives doesn't matter) or the target no
/// longer exists (stale link, nothing left to protect). A symlink to an
/// unrelated binary is somebody else's wiring.
fn symlink_is_ours(dest: &Path) -> bool {
    let Ok(target) = std::fs::read_link(dest) else {
        return false;
    };
    if dest.canonicalize().is_err() {
        return true;
    }
    target.file_name() == Some(std::ffi::OsStr::new(&bin_name()))
}

/// Sidecar written next to a copied (non-unix) link — `agentstack.exe.self-link`
/// — marking the copy as agentstack's own, so a `self link` re-run after a
/// rebuild replaces it without `--force`. Chosen over always-allowing
/// replacement on non-unix so a real agentstack binary someone installed by
/// hand (no marker) stays protected.
fn copy_marker(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".self-link");
    dest.with_file_name(name)
}

#[cfg(unix)]
fn make_link(exe: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(exe, dest)
        .with_context(|| format!("symlinking {} → {}", dest.display(), exe.display()))
}

/// Windows symlinks need elevation; a copy is the portable equivalent
/// (re-run `self link` after a rebuild). The sidecar marker tags the copy as
/// ours so that re-run replaces it without `--force` (see [`copy_marker`]).
#[cfg(not(unix))]
fn make_link(exe: &Path, dest: &Path) -> Result<()> {
    std::fs::copy(exe, dest)
        .map(|_| ())
        .with_context(|| format!("copying {} → {}", exe.display(), dest.display()))?;
    // Best-effort: without the marker the next re-link just needs --force.
    let _ = std::fs::write(copy_marker(dest), format!("{}\n", exe.display()));
    Ok(())
}

/// True when `dir` is writable by this user — the `[ -w ]` check install.sh
/// uses to pick `/usr/local/bin` over `~/.local/bin`.
fn dir_writable(dir: &Path) -> bool {
    crate::sys::dir_writable(dir)
}

fn on_path(dir: &Path) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|p| p == dir)
}

/// Every `name` entry on PATH, in PATH order — the first is what a bare
/// `agentstack` runs; later ones are shadowed (often stale) installs. Broken
/// symlinks are included so `self which` can flag them.
pub fn find_all_on_path(name: &str) -> Vec<PathBuf> {
    let Some(path) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() {
            continue;
        }
        let cand = dir.join(name);
        if std::fs::symlink_metadata(&cand).is_ok() && !out.contains(&cand) {
            out.push(cand);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    #[test]
    fn install_dir_honors_prefix_env_and_explicit_flag() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        std::env::set_var("AGENTSTACK_PREFIX", "/opt/tools/bin");
        assert_eq!(install_dir(None).unwrap(), PathBuf::from("/opt/tools/bin"));
        // An explicit --prefix wins over the env var.
        assert_eq!(
            install_dir(Some(Path::new("/x/bin"))).unwrap(),
            PathBuf::from("/x/bin")
        );
        std::env::remove_var("AGENTSTACK_PREFIX");
    }

    #[cfg(unix)]
    #[test]
    fn link_into_creates_relinks_and_guards_real_files() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let exe = tmp.child("build/agentstack");
        exe.write_str("#!binary").unwrap();
        let exe = exe.path().canonicalize().unwrap();
        let dest = tmp.path().join("bin/agentstack");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();

        // Fresh link.
        assert!(matches!(
            link_into(&exe, &dest, false).unwrap(),
            LinkOutcome::Created
        ));
        assert_eq!(dest.canonicalize().unwrap(), exe);

        // Idempotent.
        assert!(matches!(
            link_into(&exe, &dest, false).unwrap(),
            LinkOutcome::AlreadyLinked
        ));

        // A symlink to an older agentstack build (target named like our
        // binary) is re-pointed without --force — the rebuild/upgrade path.
        let other = tmp.child("old/agentstack");
        other.write_str("#!old").unwrap();
        std::fs::remove_file(&dest).unwrap();
        std::os::unix::fs::symlink(other.path(), &dest).unwrap();
        assert!(matches!(
            link_into(&exe, &dest, false).unwrap(),
            LinkOutcome::Replaced(_)
        ));
        assert_eq!(dest.canonicalize().unwrap(), exe);

        // A foreign regular file is refused without --force, replaced with it.
        std::fs::remove_file(&dest).unwrap();
        std::fs::write(&dest, "someone else's binary").unwrap();
        assert!(link_into(&exe, &dest, false).is_err());
        assert!(matches!(
            link_into(&exe, &dest, true).unwrap(),
            LinkOutcome::Replaced(_)
        ));
        assert_eq!(dest.canonicalize().unwrap(), exe);
    }

    /// A symlink named `agentstack` that somebody pointed at an unrelated
    /// binary is their wiring, not ours — re-pointing it takes --force. A
    /// broken symlink (target gone) protects nothing and re-points freely.
    #[cfg(unix)]
    #[test]
    fn link_into_guards_unrelated_symlinks_but_repoints_broken_ones() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let exe = tmp.child("build/agentstack");
        exe.write_str("#!binary").unwrap();
        let exe = exe.path().canonicalize().unwrap();
        let dest = tmp.path().join("bin/agentstack");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();

        // dest → an unrelated tool: refused without --force, left untouched.
        let other = tmp.child("tools/kubectl");
        other.write_str("#!other").unwrap();
        std::os::unix::fs::symlink(other.path(), &dest).unwrap();
        let err = link_into(&exe, &dest, false).unwrap_err();
        assert!(
            err.to_string().contains("--force"),
            "error must point at --force, got: {err}"
        );
        assert_eq!(
            std::fs::read_link(&dest).unwrap(),
            other.path(),
            "refused link must be left untouched"
        );

        // --force re-points it.
        assert!(matches!(
            link_into(&exe, &dest, true).unwrap(),
            LinkOutcome::Replaced(_)
        ));
        assert_eq!(dest.canonicalize().unwrap(), exe);

        // dest → a target that no longer exists (whatever its name): stale,
        // re-pointed without --force.
        std::fs::remove_file(&dest).unwrap();
        std::os::unix::fs::symlink(tmp.path().join("gone/some-old-tool"), &dest).unwrap();
        assert!(matches!(
            link_into(&exe, &dest, false).unwrap(),
            LinkOutcome::Replaced(_)
        ));
        assert_eq!(dest.canonicalize().unwrap(), exe);
    }

    /// A previous non-unix `self link` leaves a copy plus a marker sidecar
    /// (see `copy_marker`). Re-running after a rebuild must replace that copy
    /// without --force — and clean the marker up when the replacement is a
    /// real symlink.
    #[cfg(unix)]
    #[test]
    fn link_into_replaces_marked_copy_without_force() {
        let tmp = assert_fs::TempDir::new().unwrap();
        let exe = tmp.child("build/agentstack");
        exe.write_str("#!binary").unwrap();
        let exe = exe.path().canonicalize().unwrap();
        let dest = tmp.path().join("bin/agentstack");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();

        // What non-unix make_link leaves behind: the copied binary + marker.
        std::fs::write(&dest, "old copied build").unwrap();
        std::fs::write(copy_marker(&dest), "/old/build/agentstack\n").unwrap();

        assert!(matches!(
            link_into(&exe, &dest, false).unwrap(),
            LinkOutcome::Replaced(_)
        ));
        assert_eq!(dest.canonicalize().unwrap(), exe);
        assert!(
            !copy_marker(&dest).exists(),
            "stale marker cleaned up once dest is a real symlink"
        );
    }

    #[test]
    fn find_all_on_path_returns_hits_in_path_order() {
        let _g = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let tmp = assert_fs::TempDir::new().unwrap();
        tmp.child("a/tool").write_str("x").unwrap();
        tmp.child("b/tool").write_str("y").unwrap();
        tmp.child("c/other").write_str("z").unwrap();
        let old = std::env::var_os("PATH");
        let joined = std::env::join_paths([
            tmp.path().join("a"),
            tmp.path().join("c"),
            tmp.path().join("b"),
        ])
        .unwrap();
        std::env::set_var("PATH", &joined);

        let hits = find_all_on_path("tool");
        assert_eq!(
            hits,
            vec![tmp.path().join("a/tool"), tmp.path().join("b/tool")]
        );
        assert!(!on_path(Path::new("/nowhere")));
        assert!(on_path(&tmp.path().join("c")));

        match old {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }
}
