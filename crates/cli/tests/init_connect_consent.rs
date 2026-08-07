// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Witness: the documented first run ends in a setup that delivers — and it
//! only ever registers the bridge because the user asked for it.
//!
//! The defect this pins closed: skills and MCP servers route to the LIVE lane
//! by default, and nothing routed live reaches any tool until the agentstack
//! bridge is registered in that tool's own global config. A scripted
//! `init --yes` therefore used to end with a manifest that delivered nothing,
//! and no way to finish the job in the same command.
//!
//! `--connect` is the fix AND the consent. The two halves are pinned together
//! here, because either one alone is a bug:
//!
//! - with `--connect`, the harness config carries the bridge when init returns;
//! - **without it, that file is byte-for-byte what it was** — `--yes` is
//!   consent to write the manifest and lifted token values, never consent to
//!   edit `~/.claude.json`, and the close says plainly that nothing is
//!   delivered yet and names both ways to fix it.
//!
//! Spawns the real binary with stdin closed: a prompt on this path would fail
//! the command rather than hang CI, which is the other half of "behaves
//! sensibly when no human is present".

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

/// One scripted `agentstack` run in a throwaway machine: fake `$HOME`, a stub
/// `claude` on `$PATH` so detection sees an installed CLI, and no terminal.
fn run(bin: &str, args: &[&str], home: &Path, cwd: &Path, stub_bin: &Path) -> Output {
    Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", format!("{}:/usr/bin:/bin", stub_bin.display()))
        .stdin(Stdio::null())
        .output()
        .expect("spawn agentstack")
}

fn transcript(out: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

/// The one native config this machine starts with: a Claude Code MCP server,
/// no secrets, an absolute launcher so no PATH quirk warning muddies the close.
const NATIVE_CONFIG: &str =
    r#"{"mcpServers":{"search":{"command":"/usr/bin/env","args":["npx","-y","search-mcp"]}}}"#;

/// A fresh fake machine: `$HOME` with that native config, a stub `claude`, and
/// an empty project directory. Returns (home, project, stub bin dir).
fn machine(tmp: &Path) -> (PathBuf, PathBuf, PathBuf) {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    fs::write(home.join(".claude.json"), NATIVE_CONFIG).unwrap();

    let stub_bin = tmp.join("bin");
    fs::create_dir_all(&stub_bin).unwrap();
    let claude = stub_bin.join("claude");
    fs::write(&claude, "#!/bin/sh\nexit 0\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&claude, fs::Permissions::from_mode(0o755)).unwrap();
    }

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".git")).unwrap();
    (home, proj, stub_bin)
}

/// Does this harness config carry the bridge entry? Read as JSON, not as a
/// substring, so the assertion cannot be satisfied by the word "agentstack"
/// appearing in a server command line.
fn has_bridge(config: &Path) -> bool {
    let text = fs::read_to_string(config).unwrap();
    let value: serde_json::Value = serde_json::from_str(&text).unwrap();
    value
        .get("mcpServers")
        .and_then(|m| m.get("agentstack"))
        .is_some()
}

/// The positive claim: after the documented first run with `--connect`, the
/// live lane really is connected, and the close no longer discloses a gap.
#[test]
fn init_connect_leaves_the_live_lane_actually_delivering() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj, stub_bin) = machine(tmp.path());

    let out = run(
        bin,
        &["init", "--yes", "--secrets", "skip", "--connect"],
        &home,
        &proj,
        &stub_bin,
    );
    let text = transcript(&out);
    assert!(out.status.success(), "init --connect failed:\n{text}");

    assert!(
        has_bridge(&home.join(".claude.json")),
        "--connect must register the bridge in the harness config:\n{text}"
    );
    // The close must not disclose a gap that no longer exists, and must not
    // send the user to a command with nothing to do.
    assert!(
        !text.contains("NOT YET CONNECTED"),
        "the bridge is registered — the close must not say otherwise:\n{text}"
    );
    assert!(
        !text.contains("not yet delivering"),
        "the bridge is registered — the close must not say otherwise:\n{text}"
    );
    // Honesty about the file that WAS edited: the old note claimed the CLI
    // configs were unchanged, which `--connect` makes false.
    assert!(
        !text.contains("the CLI configs above are unchanged"),
        "--connect edited those configs; the summary must not call them unchanged:\n{text}"
    );

    // The whole point: `status` now reports a delivering setup rather than
    // routing the user to a fourth command.
    let status = run(bin, &["status"], &home, &proj, &stub_bin);
    let status_text = transcript(&status);
    assert!(status.status.success(), "status failed:\n{status_text}");
    assert!(
        !status_text.contains("gateway connect"),
        "status must not still be asking for the bridge:\n{status_text}"
    );
    assert!(
        status_text.contains("served live"),
        "status should report the live lane as serving:\n{status_text}"
    );
}

/// The negative control, and the consent claim: the SAME run without
/// `--connect` must leave the machine-wide harness config exactly as it found
/// it. `--yes` is consent to write the manifest and any lifted token values —
/// never consent to edit a file in the user's home directory.
#[test]
fn init_yes_alone_never_touches_a_harness_config() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj, stub_bin) = machine(tmp.path());
    let config = home.join(".claude.json");
    let before = fs::read_to_string(&config).unwrap();

    let out = run(
        bin,
        &["init", "--yes", "--secrets", "skip"],
        &home,
        &proj,
        &stub_bin,
    );
    let text = transcript(&out);
    assert!(out.status.success(), "init --yes failed:\n{text}");

    // Byte-for-byte: not "no bridge entry", but "nothing was written at all".
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        before,
        "`--yes` alone must not write the harness config:\n{text}"
    );
    assert!(!has_bridge(&config));

    // And the run must not read as a success that delivers. The gap, its
    // consequence, and BOTH repairs — the standalone command for this machine
    // and the flag that prevents the gap next time — are on screen.
    assert!(
        text.contains("not yet delivering"),
        "the close must not claim a complete setup:\n{text}"
    );
    assert!(
        text.contains("NOT YET CONNECTED"),
        "the close must disclose the unconnected live lane:\n{text}"
    );
    assert!(
        text.contains("agentstack x gateway connect --all --write"),
        "the close must name the command that fixes this machine:\n{text}"
    );
    assert!(
        text.contains("agentstack init --connect"),
        "the close must name the one-step form:\n{text}"
    );
}

/// A preview is a preview on both halves. `--dry-run --connect` must write
/// neither the manifest nor the harness config, and must still say what the
/// flag would do — otherwise the safe way to inspect the new flag would be the
/// one way to be surprised by it.
#[test]
fn dry_run_connect_writes_nothing_anywhere() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, proj, stub_bin) = machine(tmp.path());
    let config = home.join(".claude.json");
    let before = fs::read_to_string(&config).unwrap();

    let out = run(
        bin,
        &["init", "--dry-run", "--secrets", "skip", "--connect"],
        &home,
        &proj,
        &stub_bin,
    );
    let text = transcript(&out);
    assert!(
        out.status.success(),
        "init --dry-run --connect failed:\n{text}"
    );

    assert_eq!(fs::read_to_string(&config).unwrap(), before);
    assert!(!proj.join(".agentstack/agentstack.toml").exists());
    assert!(
        text.contains("Would also register the agentstack bridge"),
        "a preview must say what --connect would do:\n{text}"
    );
}
