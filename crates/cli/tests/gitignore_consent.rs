// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The managed `.gitignore` block is *previewed before it is written*.
//!
//! `apply` edits a file the user may have hand-curated, and the consent step
//! that guards it ("Apply this setup?", and the panel's confirm) is only
//! meaningful if the preview the user reads first actually names the edit.
//! Before this, the block was computed under `will_write` alone: a dry run
//! recorded nothing, so it previewed nothing, and the `.gitignore` change was
//! the one write nobody consented to.
//!
//! The load-bearing claim is not "the preview mentions .gitignore" but **the
//! preview names the same entries the write produces** — otherwise consent is
//! collected against a description that does not match the act.

use std::path::Path;
use std::process::Output;

use std::fs;

fn setup(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();

    let proj = tmp.join("proj");
    // `ensure_block` no-ops outside a git repo, so the repo marker is what
    // makes this scenario the one users actually hit.
    fs::create_dir_all(proj.join(".git")).unwrap();
    fs::create_dir_all(proj.join("skills/local")).unwrap();
    fs::write(proj.join("skills/local/SKILL.md"), "# local\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        // The managed .gitignore block covers RENDERED artifacts, and `apply`
        // honours the delivery planner — so this fixture asks for the rendered
        // lane explicitly. Without it there is no `.mcp.json` to hide, and the
        // block under test has no subject.
        "version = 1\n[delivery]\nrender_locally = true\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"http\"\nurl = \"https://x/mcp\"\n\
         [skills.local]\npath = \"./skills/local\"\n\
         [profiles.default]\nservers = [\"demo\"]\nskills = [\"local\"]\n",
    )
    .unwrap();
    (home, proj)
}

fn agentstack(home: &Path, proj: &Path, args: &[&str]) -> Output {
    std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .arg("--manifest-dir")
        .arg(proj)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .output()
        .expect("run agentstack binary")
}

/// Test-only grant, the two-step a panel drives: pin the surface, review it,
/// then bind the yes to the digest of exactly those bytes. The managed block is
/// this file's subject, so consent must not be what stops the writes that
/// produce it — and it has to be re-run after every manifest or overlay edit,
/// since those bytes ARE the consent digest (`render::apply::trust_refusal`).
fn grant(home: &Path, proj: &Path) {
    agentstack(home, proj, &["lock", "--write"]);
    let preview = agentstack(home, proj, &["trust", "--preview"]);
    let text = String::from_utf8_lossy(&preview.stdout).into_owned();
    let digest = serde_json::from_str::<serde_json::Value>(&text)
        .unwrap_or_else(|e| panic!("trust --preview is not JSON ({e}):\n{text}"))["surface_digest"]
        .as_str()
        .expect("preview carries a surface digest")
        .to_string();
    agentstack(home, proj, &["trust", "--yes", "--consented", &digest]);
}

/// Record the durable opt-out by hand, the way a committed manifest carries it.
fn opt_out(proj: &Path) {
    let path = proj.join("agentstack.toml");
    let text = fs::read_to_string(&path).unwrap();
    fs::write(&path, format!("{text}\n[meta]\ngitignore = false\n")).unwrap();
}

/// Drop ANSI SGR sequences so entry lines can be parsed. The output colors `+`
/// green regardless of TTY or `NO_COLOR`, so stripping here is what makes the
/// "previewed == written" comparison possible at all.
fn plain(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // ESC '[' params… final. The '[' introducer must be consumed
            // first: it falls inside the @-~ final-byte range, so testing it
            // as a terminator ends the sequence immediately and leaks "32m".
            if chars.next() == Some('[') {
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The whole point: a dry run names the `.gitignore` edit, lists the entries
/// that would land, points at the opt-out — and changes nothing on disk.
#[test]
fn dry_run_previews_the_gitignore_edit_without_making_it() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = setup(tmp.path());
    grant(&home, &proj);

    let out = agentstack(&home, &proj, &["apply"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains(".gitignore"),
        "a dry run must name the .gitignore edit it is about to make:\n{stdout}"
    );
    assert!(
        stdout.contains("/.mcp.json"),
        "the preview must list the entries that would land, not just a count:\n{stdout}"
    );
    assert!(
        stdout.contains("--no-gitignore"),
        "the preview must name the opt-out, so declining is reachable:\n{stdout}"
    );
    assert!(
        !proj.join(".gitignore").exists(),
        "a dry run must not create .gitignore"
    );
}

/// Consent integrity: every entry the preview named is an entry the write
/// produces. A preview that under- or over-states the edit is worse than none.
#[test]
fn previewed_entries_are_exactly_the_written_entries() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = setup(tmp.path());
    grant(&home, &proj);

    let preview = agentstack(&home, &proj, &["apply"]);
    let preview_out = plain(&String::from_utf8_lossy(&preview.stdout));
    // The previewed block is the `+ <entry>` lines this command owns.
    let previewed: Vec<String> = preview_out
        .lines()
        .filter_map(|l| l.trim().strip_prefix("+ "))
        .filter(|l| l.starts_with('/'))
        .map(|l| l.trim().to_string())
        .collect();
    assert!(
        !previewed.is_empty(),
        "expected a previewed block:\n{preview_out}"
    );

    let wrote = agentstack(&home, &proj, &["apply", "--write"]);
    assert!(
        wrote.status.success(),
        "apply --write failed:\n{}",
        String::from_utf8_lossy(&wrote.stdout)
    );
    let block = fs::read_to_string(proj.join(".gitignore")).unwrap();

    for entry in &previewed {
        assert!(
            block.contains(entry.as_str()),
            "previewed {entry} never landed — the preview overstated the edit:\n{block}"
        );
    }
    // And nothing landed that the preview withheld.
    for line in block
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with('/') && !l.is_empty())
    {
        assert!(
            previewed.iter().any(|p| p == line),
            "{line} was written but never previewed — consent was collected \
             against an incomplete description:\n{preview_out}"
        );
    }
}

/// The durable opt-out: `[meta] gitignore = false` silences BOTH arms. This is
/// what a per-run flag could not do — the next activation would re-add the
/// block — so it is the property the whole setting exists for.
#[test]
fn manifest_opt_out_silences_preview_and_write() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = setup(tmp.path());
    grant(&home, &proj);
    opt_out(&proj);

    let preview = plain(&String::from_utf8_lossy(
        &agentstack(&home, &proj, &["apply"]).stdout,
    ));
    assert!(
        !preview.contains("would add a managed block"),
        "an opted-out project must not preview a block:\n{preview}"
    );

    agentstack(&home, &proj, &["apply", "--write"]);
    assert!(
        !proj.join(".gitignore").exists(),
        "an opted-out project must not get a .gitignore"
    );

    // And the activation path agrees — this is the one that used to undo the
    // decision on every toolset switch.
    agentstack(&home, &proj, &["use", "default", "--write"]);
    assert!(
        !proj.join(".gitignore").exists(),
        "activation must respect the opt-out too — this is the regression the \
         durable setting exists to prevent"
    );
}

/// Opting out with a block already committed does NOT strip it: routine
/// commands must leave a block a team may have committed alone. It is reported
/// instead, because silence would let the user believe files are visible to
/// `git status` while the stale block still hides them.
#[test]
fn leftover_block_is_reported_not_stripped() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = setup(tmp.path());
    grant(&home, &proj);

    agentstack(&home, &proj, &["apply", "--write"]);
    let before = fs::read_to_string(proj.join(".gitignore")).unwrap();
    assert!(before.contains("/.mcp.json"), "expected a block first");

    opt_out(&proj);
    grant(&home, &proj);
    let out = plain(&String::from_utf8_lossy(
        &agentstack(&home, &proj, &["apply", "--write"]).stdout,
    ));

    assert_eq!(
        fs::read_to_string(proj.join(".gitignore")).unwrap(),
        before,
        "a routine command must never strip the managed block"
    );
    assert!(
        out.contains("still present"),
        "the leftover block must be reported:\n{out}"
    );
}

/// `agentstack.local.toml` deep-merges over the manifest and is itself inside
/// the trust digest, so it is the designed home for "the repo says no but this
/// checkout wants it" — with no new code and no new trust reasoning.
#[test]
fn local_overlay_can_re_enable_for_one_checkout() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = setup(tmp.path());
    grant(&home, &proj);
    opt_out(&proj);
    fs::write(
        proj.join("agentstack.local.toml"),
        "[meta]\ngitignore = true\n",
    )
    .unwrap();
    grant(&home, &proj);

    agentstack(&home, &proj, &["apply", "--write"]);
    let block = fs::read_to_string(proj.join(".gitignore")).unwrap_or_default();
    assert!(
        block.contains("/.mcp.json"),
        "the local overlay must be able to re-enable the block for this checkout: {block}"
    );
}

/// The panel verb: previewed, digest-bound, and the block removal happens only
/// here — the one moment a human has said this project commits its artifacts.
#[test]
fn panel_verb_is_digest_bound_and_removes_the_block() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = setup(tmp.path());
    grant(&home, &proj);
    agentstack(&home, &proj, &["apply", "--write"]);
    assert!(proj.join(".gitignore").exists(), "expected a block first");

    let preview = agentstack(
        &home,
        &proj,
        &["set-gitignore", "--enabled", "false", "--preview"],
    );
    let body: serde_json::Value =
        serde_json::from_slice(&preview.stdout).expect("preview emits JSON");
    assert_eq!(body["removes_block"], serde_json::json!(true));
    let digest = body["consent_digest"].as_str().expect("consent_digest");

    // A wrong digest must refuse before writing anything.
    let refused = agentstack(
        &home,
        &proj,
        &[
            "set-gitignore",
            "--enabled",
            "false",
            "--yes",
            "--consented",
            "sha256:0000",
        ],
    );
    assert!(!refused.status.success(), "a stale digest must refuse");

    let applied = agentstack(
        &home,
        &proj,
        &[
            "set-gitignore",
            "--enabled",
            "false",
            "--yes",
            "--consented",
            digest,
        ],
    );
    assert!(
        applied.status.success(),
        "apply failed:\n{}",
        String::from_utf8_lossy(&applied.stdout)
    );
    assert!(
        fs::read_to_string(proj.join("agentstack.toml"))
            .unwrap()
            .contains("gitignore = false"),
        "the setting must be recorded in the manifest"
    );
    let after = fs::read_to_string(proj.join(".gitignore")).unwrap_or_default();
    assert!(
        !after.contains("/.mcp.json"),
        "the consented opt-out must remove the block: {after}"
    );
}

/// `use` is the toolset-activation path — the one a panel "Switch" runs — and
/// it edits the same file, so it owes the same preview. It materializes skills
/// too, so its block covers an entry `apply` alone never produces.
#[test]
fn activation_dry_run_previews_the_gitignore_edit() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = setup(tmp.path());
    grant(&home, &proj);

    let out = agentstack(&home, &proj, &["use", "default"]);
    let stdout = plain(&String::from_utf8_lossy(&out.stdout));

    assert!(
        stdout.contains("would add a managed block"),
        "activating a toolset must preview its .gitignore edit:\n{stdout}"
    );
    assert!(
        stdout.contains("/.claude/skills/"),
        "the preview must cover skills the activation would materialize:\n{stdout}"
    );
    assert!(
        !proj.join(".gitignore").exists(),
        "a dry-run activation must not create .gitignore"
    );
}

/// Declining is real: `--no-gitignore` suppresses the preview *and* the write,
/// so a user who opts out is never shown a change that will not happen.
#[test]
fn no_gitignore_suppresses_both_the_preview_and_the_write() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj) = setup(tmp.path());
    grant(&home, &proj);

    let preview = agentstack(&home, &proj, &["apply", "--no-gitignore"]);
    let preview_out = String::from_utf8_lossy(&preview.stdout);
    assert!(
        !preview_out.contains("would add a managed block"),
        "opting out must not preview a block that will never be written:\n{preview_out}"
    );

    let wrote = agentstack(&home, &proj, &["apply", "--write", "--no-gitignore"]);
    assert!(
        wrote.status.success(),
        "apply --write --no-gitignore failed:\n{}",
        String::from_utf8_lossy(&wrote.stdout)
    );
    assert!(
        !proj.join(".gitignore").exists(),
        "--no-gitignore must leave .gitignore untouched"
    );
}
