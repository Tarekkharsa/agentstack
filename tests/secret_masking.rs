//! Security (issue #4): `apply`/`diff` must never print a resolved secret value
//! in cleartext. The human-facing diff/apply preview redacts every `${REF}` that
//! resolved to its `${REF}` name — while `--write` still persists the REAL value
//! to the native config on disk. The two must not disagree: masked on screen,
//! true on disk.

use std::fs;

/// The secret value must never appear in a terminal preview; the resolver reads
/// it from the process env (the `env` link of the chain), so the binary resolves
/// it exactly as it would from the keychain — but only its `${REF}` name is shown.
const SECRET: &str = "gumlet_5f3bc35892abDEADBEEFcafef00dfeed";
const REF_NAME: &str = "GUMLET_TOKEN";

fn write_manifest(proj: &std::path::Path) {
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.gumlet]\ntype = \"http\"\nurl = \"https://gumlet/mcp\"\n\
         headers = { Authorization = \"Bearer ${GUMLET_TOKEN}\" }\n",
    )
    .unwrap();
}

/// Run the real binary with the secret present in the environment.
fn run(proj: &std::path::Path, home: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env(REF_NAME, SECRET)
        .output()
        .unwrap();
    // Strip ANSI color so assertions match on the raw text either way.
    let raw = String::from_utf8_lossy(&out.stdout).into_owned();
    strip_ansi(&raw)
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            // Skip until the terminating letter of the CSI sequence.
            for c in chars.by_ref() {
                if c.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[test]
fn apply_dry_run_masks_secret_but_write_persists_it() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_manifest(&proj);

    // 1. Dry-run preview: the resolved secret must NOT appear in stdout…
    let dry = run(&proj, &home, &["apply", "--dry-run", "--no-gitignore"]);
    assert!(
        !dry.contains(SECRET),
        "resolved secret leaked into the apply preview:\n{dry}"
    );
    // …and the masked `${REF}` form must appear in its place.
    assert!(
        dry.contains(&format!("Bearer ${{{REF_NAME}}}")),
        "expected the ${{REF}} mask in the preview:\n{dry}"
    );

    // Nothing written on a dry-run.
    let claude_cfg = home.join(".claude.json");
    assert!(!claude_cfg.exists(), "dry-run must not write");

    // 2. `--write`: the REAL secret must land on disk unchanged.
    let written_out = run(&proj, &home, &["apply", "--write", "--no-gitignore"]);
    assert!(
        !written_out.contains(SECRET),
        "the --write summary/diff must also mask the secret:\n{written_out}"
    );
    let on_disk = fs::read_to_string(&claude_cfg).unwrap();
    assert!(
        on_disk.contains(SECRET),
        "the true resolved secret must be written to the native config:\n{on_disk}"
    );
    assert!(
        !on_disk.contains(&format!("${{{REF_NAME}}}")),
        "the on-disk config must hold the value, not the ${{REF}} placeholder:\n{on_disk}"
    );
}

#[test]
fn diff_masks_secret_on_context_and_changed_lines() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    write_manifest(&proj);

    // Apply once so the secret is on disk, then `diff` against a manifest whose
    // url changed — the diff shows the unchanged secret header as a context line.
    run(&proj, &home, &["apply", "--write", "--no-gitignore"]);
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [servers.gumlet]\ntype = \"http\"\nurl = \"https://gumlet/mcp/v2\"\n\
         headers = { Authorization = \"Bearer ${GUMLET_TOKEN}\" }\n",
    )
    .unwrap();

    let diff = run(&proj, &home, &["diff"]);
    assert!(
        !diff.contains(SECRET),
        "diff leaked the secret on a context/changed line:\n{diff}"
    );
    assert!(
        diff.contains(&format!("Bearer ${{{REF_NAME}}}")),
        "diff should show the masked header:\n{diff}"
    );
    // The url change itself must still be visible (masking is surgical).
    assert!(diff.contains("v2"), "the real (non-secret) change must show:\n{diff}");
}
