//! `--version` must name a build, not only a release number.
//!
//! The version in `Cargo.toml` is bumped by hand when a release is cut, so
//! every build between two bumps prints the same number. That is how the
//! tagged `v0.18.0-rc.2` binary and a `main` 125 commits ahead of it — with
//! different consent gating and different delivery routing — came to print one
//! identical line, and why a bug report quoting a version could mean either.
//! `build.rs` therefore embeds the commit, and these tests hold that wiring in
//! place: the shape of the line, and the fact that a checkout with git really
//! does get a revision rather than silently falling back to the old line.
//!
//! The absent-revision case (a release tarball, a vendored tree, `cargo
//! publish`) is not a failure and is not tested by failing: it is the empty
//! form below, which every shape assertion here accepts.

use std::process::Command;

/// What `AGENTSTACK_BUILD_REV_FIELD` held when this crate was compiled: `""`,
/// or `", <rev>"` with its separator. Test targets are compiled by the same
/// build script run as the binary, so this is exactly what the binary got.
const REV_FIELD: &str = env!("AGENTSTACK_BUILD_REV_FIELD");

fn version_line() -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .arg("--version")
        .output()
        .expect("the binary under test runs");
    assert!(out.status.success(), "--version exits 0");
    String::from_utf8(out.stdout)
        .expect("--version is utf-8")
        .trim()
        .to_string()
}

/// The revision as printed, or `None` when this build has none.
fn printed_rev(line: &str) -> Option<String> {
    let inside = line
        .rsplit_once(')')
        .map(|(head, _)| head)
        .and_then(|head| head.split_once(" (").map(|(_, tail)| tail))
        .expect("the version line ends in a parenthesised detail");
    let sandbox = ["sandbox: yes", "sandbox: no"]
        .into_iter()
        .find(|s| inside.starts_with(s))
        .expect("the detail still opens with the compiled-in feature set");
    match inside[sandbox.len()..].strip_prefix(", ") {
        Some(rev) => Some(rev.to_string()),
        None => {
            assert_eq!(
                &inside[sandbox.len()..],
                "",
                "the only thing allowed after the feature set is `, <rev>`"
            );
            None
        }
    }
}

#[test]
fn version_prints_the_release_the_feature_set_and_the_build() {
    let line = version_line();
    assert_eq!(
        line,
        format!("agentstack {}", agentstack::cli::VERSION),
        "clap prints the binary name plus exactly the constant we compiled"
    );
    assert!(
        line.starts_with(concat!(
            "agentstack ",
            env!("CARGO_PKG_VERSION"),
            " (sandbox: "
        )),
        "the released shape — name, version, feature set — is a prefix that \
         readers and docs already rely on; a revision is appended, never \
         inserted: {line}"
    );
    // Round-trips the parse so a malformed detail fails here and not silently.
    let rev = printed_rev(&line);
    assert_eq!(
        rev.map(|r| format!(", {r}")).unwrap_or_default(),
        REV_FIELD,
        "the printed revision is the one the build script emitted"
    );
}

#[test]
fn a_printed_revision_is_one_tame_token() {
    let line = version_line();
    let Some(rev) = printed_rev(&line) else {
        return; // A build with no revision — the documented, honest fallback.
    };
    assert!(
        !rev.is_empty() && rev.len() <= 40,
        "a revision is short enough to read in a bug report: {rev:?}"
    );
    assert!(
        rev.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')),
        "a revision carries no spaces, quotes or newlines — the version line \
         is pasted into issues and read by eye: {rev:?}"
    );
}

/// A checkout that has git must not quietly print the old, ambiguous line.
///
/// Skipped where the premise does not hold — no git, no repository, or an
/// `AGENTSTACK_BUILD_REV` override deciding the answer instead — because in
/// each of those the empty form is correct rather than a regression.
#[test]
fn a_git_checkout_gets_a_commit() {
    if std::env::var_os("AGENTSTACK_BUILD_REV").is_some() {
        return;
    }
    let in_repo = Command::new("git")
        .args(["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"])
        .output()
        .is_ok_and(|out| out.status.success());
    if !in_repo {
        return;
    }

    let line = version_line();
    let rev = printed_rev(&line).unwrap_or_else(|| {
        panic!("built from a git checkout, so --version must name the commit: {line}")
    });
    let commit = rev.strip_suffix("-dirty").unwrap_or(&rev);
    assert!(
        commit.len() >= 7 && commit.chars().all(|c| c.is_ascii_hexdigit()),
        "the revision is an abbreviated commit, optionally marked -dirty: {rev:?}"
    );
}
