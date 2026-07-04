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
use owo_colors::OwoColorize;

use crate::cli::{SelfArgs, SelfCommand, SelfLinkArgs};

/// The binary name a shell looks up on PATH (`agentstack.exe` on Windows).
pub fn bin_name() -> String {
    format!("agentstack{}", std::env::consts::EXE_SUFFIX)
}

pub fn run(args: &SelfArgs) -> Result<()> {
    match &args.command {
        SelfCommand::Link(link) => run_link(link),
        SelfCommand::Which => run_which(),
    }
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
        "Harness configs registered with `agentstack connect` use this stable \
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

enum LinkOutcome {
    AlreadyLinked,
    Created,
    /// Replaced an existing entry; carries what it used to point at.
    Replaced(PathBuf),
}

/// Point `dest` at `exe`. A dest that already resolves to `exe` is a no-op;
/// symlinks are ours to re-point freely (that's the rebuild/upgrade path); a
/// foreign regular file needs `--force`.
fn link_into(exe: &Path, dest: &Path, force: bool) -> Result<LinkOutcome> {
    if let Ok(meta) = std::fs::symlink_metadata(dest) {
        if dest.canonicalize().ok().as_deref() == Some(exe) {
            return Ok(LinkOutcome::AlreadyLinked);
        }
        if !meta.file_type().is_symlink() && !force {
            anyhow::bail!(
                "{} exists and is not a symlink — not overwriting a real file without --force",
                dest.display()
            );
        }
        let old = std::fs::read_link(dest).unwrap_or_else(|_| dest.to_path_buf());
        std::fs::remove_file(dest).with_context(|| format!("removing {}", dest.display()))?;
        make_link(exe, dest)?;
        return Ok(LinkOutcome::Replaced(old));
    }
    make_link(exe, dest)?;
    Ok(LinkOutcome::Created)
}

#[cfg(unix)]
fn make_link(exe: &Path, dest: &Path) -> Result<()> {
    std::os::unix::fs::symlink(exe, dest)
        .with_context(|| format!("symlinking {} → {}", dest.display(), exe.display()))
}

/// Windows symlinks need elevation; a copy is the portable equivalent
/// (re-run `self link` after a rebuild).
#[cfg(not(unix))]
fn make_link(exe: &Path, dest: &Path) -> Result<()> {
    std::fs::copy(exe, dest)
        .map(|_| ())
        .with_context(|| format!("copying {} → {}", exe.display(), dest.display()))
}

/// True when `dir` is writable by this user — the `[ -w ]` check install.sh
/// uses to pick `/usr/local/bin` over `~/.local/bin`.
#[cfg(unix)]
fn dir_writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else {
        return false;
    };
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(not(unix))]
fn dir_writable(dir: &Path) -> bool {
    std::fs::metadata(dir)
        .map(|m| m.is_dir() && !m.permissions().readonly())
        .unwrap_or(false)
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

        // A symlink to some other binary is re-pointed without --force.
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
