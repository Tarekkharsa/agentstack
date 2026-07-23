// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Stage 1.4 witness: the ordinary local journey — scripted
//! `init → apply --write → doctor` over an existing native config — completes
//! without prompting and without surfacing a single advanced-mode concept.
//! No Docker, policy, gateway, confinement/lockdown/sandbox, workflow, or
//! trust vocabulary appears until the user reaches for those features.
//!
//! Spawns the real binary (not library calls) because the claim is about what
//! the terminal actually prints.

use std::fs;
use std::process::{Command, Stdio};

/// Words that name advanced modes or internal boundaries. The ordinary journey
/// must not print any of them (case-insensitive).
const ADVANCED_VOCAB: &[&str] = &[
    "docker",
    "gateway",
    "policy",
    "confinement",
    "lockdown",
    "sandbox",
    "workflow",
    "trust",
];

fn run(
    bin: &str,
    args: &[&str],
    home: &std::path::Path,
    cwd: &std::path::Path,
    stub_bin: &std::path::Path,
) -> (String, bool) {
    let out = Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", format!("{}:/usr/bin:/bin", stub_bin.display()))
        // No terminal: stdin is closed, so any prompt would fail the command
        // rather than hang the test.
        .stdin(Stdio::null())
        .output()
        .expect("spawn agentstack");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    (text, out.status.success())
}

#[test]
fn scripted_init_apply_doctor_needs_no_advanced_vocabulary() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();

    // One existing native config to import: a Claude Code server, no secrets.
    // The absolute launcher (like the first-value demo fixture) keeps the
    // bare-`npx` PATH quirk warning out of a journey meant to end clean.
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"search":{"command":"/usr/bin/env","args":["npx","-y","search-mcp"]}}}"#,
    )
    .unwrap();

    // A stub `claude` on a controlled PATH so detection sees an installed CLI,
    // not just a leftover config file.
    let stub_bin = tmp.path().join("bin");
    fs::create_dir_all(&stub_bin).unwrap();
    fs::write(stub_bin.join("claude"), "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(stub_bin.join("claude"), fs::Permissions::from_mode(0o755)).unwrap();
    }

    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();

    let mut transcript = String::new();
    for args in [
        vec!["init", "--yes", "--secrets", "skip"],
        vec!["apply", "--scope", "global", "--write"],
        vec!["doctor"],
    ] {
        let (text, ok) = run(bin, &args, &home, &proj, &stub_bin);
        assert!(ok, "`agentstack {}` failed:\n{text}", args.join(" "));
        transcript.push_str(&text);
    }

    let lower = transcript.to_lowercase();
    for word in ADVANCED_VOCAB {
        assert!(
            !lower.contains(word),
            "the ordinary journey printed advanced vocabulary '{word}':\n{transcript}"
        );
    }

    // The journey really happened: the manifest exists, the render landed, and
    // doctor closed clean — so the vocabulary claim covers a working flow, not
    // an early exit.
    assert!(proj.join(".agentstack/agentstack.toml").exists());
    assert!(transcript.contains("0 error(s), 0 warning(s)"));
}
