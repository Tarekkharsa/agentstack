//! Two build-time facts the crate cannot work out for itself.
//!
//! 1. Rebuild when the embedded adapter/catalog data changes. `include_dir!`
//!    does not track directory contents on its own, so without this a newly
//!    added or edited catalog file would not be re-embedded.
//! 2. Embed the commit this binary was built from, so `--version` names a
//!    build and not just a release number. The crate version is bumped by
//!    hand at release time, so every build between two bumps prints the same
//!    number: a tagged `v0.18.0-rc.2` binary and a `main` a hundred commits
//!    later were indistinguishable, and a bug report naming a version could
//!    mean either one.

use std::path::PathBuf;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=catalog");
    emit_build_rev();
}

/// Longest revision token we will paste into `--version`.
const REV_MAX: usize = 40;

/// Emit `AGENTSTACK_BUILD_REV_FIELD`: either empty, or `", <rev>"` — the
/// revision with the separator that precedes it, ready to be `concat!`ed into
/// the version string (see `cli::VERSION`). The separator lives here because
/// `concat!` cannot ask at compile time whether a literal is empty, and clap
/// wants a `&'static str`, so both shapes have to be decided on this side.
///
/// It leads with the comma rather than trailing one because cargo trims the
/// value of a `rustc-env` line: a trailing `", "` would arrive as `","` and
/// print `(sandbox: no,ecf5c4d)`. A leading comma is not whitespace, so it
/// survives, which is also why the revision goes last in the version string.
fn emit_build_rev() {
    // `AGENTSTACK_BUILD_REV` is the override: a reproducible build (a distro,
    // a CI job that wants the version string to depend only on its inputs)
    // sets it to the revision it wants, or to the empty string to leave the
    // revision out entirely. Set-but-empty is a deliberate "omit" and must not
    // fall through to asking git.
    println!("cargo:rerun-if-env-changed=AGENTSTACK_BUILD_REV");
    let rev = match std::env::var("AGENTSTACK_BUILD_REV") {
        Ok(explicit) => explicit.trim().to_string(),
        Err(_) => {
            watch_git_state();
            git_rev().unwrap_or_default()
        }
    };

    // The value ends up in a line users paste into bug reports, so keep it a
    // single tame token. Anything else is dropped rather than printed: a
    // missing revision degrades to the old version line, which is honest,
    // while a mangled one is not.
    let rev = if rev.is_empty() || rev.len() > REV_MAX || !rev.chars().all(is_rev_char) {
        String::new()
    } else {
        rev
    };

    if rev.is_empty() {
        println!("cargo:rustc-env=AGENTSTACK_BUILD_REV_FIELD=");
    } else {
        println!("cargo:rustc-env=AGENTSTACK_BUILD_REV_FIELD=, {rev}");
    }
}

fn is_rev_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

/// The short commit, with `-dirty` appended when tracked files differ from it.
///
/// `None` whenever git cannot answer — no git binary (a release tarball, a
/// vendored or `cargo publish` build), not a repository, or a repository with
/// no commits yet. None of those may fail the build; they simply produce a
/// version line without a revision.
///
/// `git describe --always --dirty` was the alternative. It is rejected because
/// this repository tags releases: it would print `v0.18.0-rc.2-125-ga1b2c3d`,
/// repeating the version number that is already the first word of the line.
/// The bare short hash says the same thing in seven characters.
fn git_rev() -> Option<String> {
    let head = git(&["rev-parse", "--short=7", "HEAD"])?;
    // Tracked changes only. Untracked files (scratch notes, build output an
    // ignore rule has not caught) do not go into the binary, and counting them
    // would mark almost every working tree dirty, which would make the marker
    // mean nothing.
    let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
        .is_some_and(|status| !status.is_empty());
    Some(if dirty { format!("{head}-dirty") } else { head })
}

/// Ask cargo to re-run this script when the answer could have changed.
///
/// Emitting any `rerun-if-changed` opts the script out of cargo's default
/// "re-run when anything changed", so without this the revision would be
/// frozen at whatever it was when the catalog last changed. Watching `src`
/// costs nothing extra — an edit there recompiles the crate anyway — and it
/// is what keeps the `-dirty` marker roughly current, since committing,
/// staging and editing this crate all reach one of these paths. It is still
/// best-effort: an uncommitted edit in another crate can leave the marker one
/// build stale. A revision that is occasionally one build behind is a large
/// improvement on a version number that is 125 commits behind, and pretending
/// otherwise would need a script that re-runs on every build.
fn watch_git_state() {
    println!("cargo:rerun-if-changed=src");

    let Some(git_dir) = git(&["rev-parse", "--absolute-git-dir"]).map(PathBuf::from) else {
        return;
    };
    let mut watched = vec![
        git_dir.join("HEAD"),
        git_dir.join("packed-refs"),
        git_dir.join("index"),
    ];
    // The branch ref itself: `HEAD` only records which branch is checked out,
    // so a commit on that branch moves the ref file and not `HEAD`.
    if let Some(head_ref) = git(&["symbolic-ref", "--quiet", "HEAD"]) {
        watched.push(git_dir.join(head_ref));
    }
    for path in watched {
        // A path that does not exist would make cargo re-run this script on
        // every build, so only name the ones that are really there.
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

/// Run git, and treat every failure — missing binary, non-zero exit, non-UTF-8
/// output — as "git has no answer".
fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    Some(text.trim().to_string())
}
