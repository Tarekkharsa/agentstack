// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The consolidated CLI surface: merged verbs parse (`lock --update/--upgrade`,
//! the `report` umbrella, `gateway`, `lib pack-init`), retired top-level names
//! are really gone, and clap's own debug assertions hold for the whole tree.

use agentstack::cli::{Cli, Command, SessionCmd};
use clap::{CommandFactory, Parser};

#[test]
fn command_tree_is_well_formed() {
    Cli::command().debug_assert();
}

// DX witnesses for the progressive-disclosure help surface:
// `status` exists (git/docker muscle memory), the visible list stays the
// small beginner loop, and the after_help map lists every top-level command
// so nothing hidden is undiscoverable.
#[test]
fn status_parses_and_help_maps_every_command() {
    Cli::try_parse_from(["agentstack", "status"]).expect("status must parse");

    let cmd = Cli::command();
    let visible: Vec<&str> = cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        .map(|c| c.get_name())
        .collect();
    assert_eq!(
        visible,
        ["init", "status", "add", "search", "apply", "doctor", "use", "run", "trust"],
        "the visible list is the beginner loop, in task order"
    );

    let after_help = cmd.get_after_help().expect("after_help exists").to_string();
    for c in cmd.get_subcommands() {
        let name = c.get_name();
        if name == "help" || name == "setup" {
            continue; // setup is a deliberately unadvertised legacy alias.
        }
        assert!(
            after_help.contains(name),
            "'{name}' must appear in the grouped after_help map"
        );
    }
}

// `--help --all` must be a genuinely different, longer view: every command
// (hidden ones and nested subcommands included) WITH its summary — not a
// byte-for-byte copy of the abbreviated help (audit finding C5).
#[test]
fn full_inventory_differs_from_short_help_and_covers_hidden_commands() {
    let inventory = agentstack::cli::full_command_inventory();
    let short_after_help = Cli::command()
        .get_after_help()
        .expect("after_help exists")
        .to_string();
    assert_ne!(inventory, short_after_help);
    // Hidden top-level commands appear with their summaries…
    for hidden in ["optimize", "gateway", "restore", "settings"] {
        assert!(inventory.contains(hidden), "inventory lists '{hidden}'");
    }
    // …and so do nested subcommands the short help never shows.
    assert!(
        inventory.contains("pack-init"),
        "nested lib subcommands listed"
    );
    // The short help advertises how to reach it.
    assert!(short_after_help.contains("--help --all"));
    // T6: internal audit shorthand ("P27" and friends) never leaks into the
    // help a user reads.
    let leaks_p_number = |s: &str| {
        s.as_bytes()
            .windows(2)
            .any(|w| w[0] == b'P' && w[1].is_ascii_digit())
    };
    assert!(!leaks_p_number(&inventory), "P-number in --help --all");
    assert!(!leaks_p_number(&short_after_help), "P-number in --help");
}

#[test]
fn consolidated_verbs_parse() {
    for argv in [
        vec!["agentstack", "lock"],
        vec!["agentstack", "lock", "--update"],
        vec!["agentstack", "lock", "--update", "sql-review"],
        vec![
            "agentstack",
            "lock",
            "--upgrade",
            "acme",
            "--yes",
            "--write",
        ],
        vec!["agentstack", "lock", "--upgrade", "--all"],
        vec!["agentstack", "report", "run", "r-1234", "--json"],
        vec!["agentstack", "report", "runs", "--json"],
        vec!["agentstack", "report", "usage", "--live"],
        vec!["agentstack", "gateway", "connect", "--all"],
        vec!["agentstack", "gateway", "disconnect", "--all"],
        vec!["agentstack", "lib", "pack-init", "my-pack"],
        vec!["agentstack", "report", "calls", "--since", "7"],
        vec!["agentstack", "diff", "--json"],
        vec!["agentstack", "explain", "github", "--json"],
        // The machine-invoked entrypoint written into harness configs must
        // keep parsing exactly as `connect` renders it.
        vec!["agentstack", "mcp", "--auto-project"],
    ] {
        Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?} must parse: {e}"));
    }
}

#[test]
fn runtime_defaults_scope_from_the_manifest_and_hidden_help_points_to_inventory() {
    let run = Cli::try_parse_from(["agentstack", "run", "codex"]).unwrap();
    let Some(Command::Run(args)) = run.command else {
        panic!("run parsed as the wrong command");
    };
    assert_eq!(
        args.scope, None,
        "runtime resolves the manifest-home default"
    );

    let session = Cli::try_parse_from(["agentstack", "session", "start", "dev"]).unwrap();
    let Some(Command::Session(args)) = session.command else {
        panic!("session parsed as the wrong command");
    };
    let SessionCmd::Start { scope, .. } = args.cmd else {
        panic!("session start parsed as the wrong subcommand");
    };
    assert_eq!(scope, None, "session resolves the manifest-home default");

    let runtime = agentstack::cli::runtime_command();
    let adopt = runtime
        .get_subcommands()
        .find(|c| c.get_name() == "adopt")
        .expect("adopt command");
    assert!(adopt
        .get_after_help()
        .expect("hidden footer")
        .to_string()
        .contains("agentstack --help --all"));
}

#[test]
fn retired_top_level_verbs_are_gone() {
    for argv in [
        vec!["agentstack", "bootstrap"],
        vec!["agentstack", "update"],
        vec!["agentstack", "upgrade", "x"],
        vec!["agentstack", "runs"],
        vec!["agentstack", "stats"],
        vec!["agentstack", "analyze"],
        vec!["agentstack", "connect"],
        vec!["agentstack", "disconnect"],
        vec!["agentstack", "pack", "init"],
        // Round-2 cuts: broken/duplicate/ungoverned surfaces.
        vec!["agentstack", "hook", "zsh"],
        vec!["agentstack", "codemode"],
        vec!["agentstack", "consolidate"],
        vec!["agentstack", "lib", "consolidate"],
        vec!["agentstack", "lib", "migrate"],
        // `audit` was folded into `doctor --deep`; the top-level verb is gone.
        vec!["agentstack", "audit"],
    ] {
        assert!(
            Cli::try_parse_from(&argv).is_err(),
            "{argv:?} should no longer parse"
        );
    }
}

// T1 (third-pass DX audit): a reader hanging up early must end the process
// silently — the Unix default — not as a `println!` panic with exit 101 and
// a backtrace note. The reader side of the pipe is dropped BEFORE the child
// spawns, so its very first write hits a closed pipe deterministically.
#[cfg(unix)]
#[test]
fn broken_pipe_exits_silently_not_as_a_panic() {
    use std::io::Read;
    use std::process::{Command, Stdio};

    let (reader, writer) = std::io::pipe().expect("pipe");
    drop(reader); // hang up before the child ever writes

    let mut child = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["--help", "--all"]) // long output, needs no manifest
        .stdout(writer)
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn agentstack");

    let status = child.wait().expect("wait");
    let mut stderr = String::new();
    child
        .stderr
        .take()
        .expect("stderr piped")
        .read_to_string(&mut stderr)
        .expect("read stderr");

    assert!(
        !stderr.contains("panicked"),
        "broken pipe must not panic; stderr: {stderr}"
    );
    assert_ne!(status.code(), Some(101), "exit 101 is a Rust panic");
}

// T2 (third-pass DX audit): `secret set` without a terminal must refuse with
// the flags that solve it, not rpassword's raw "Device not configured".
#[test]
fn secret_set_without_tty_names_value_flag() {
    use std::process::{Command, Stdio};

    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(["secret", "set", "DEMO_TOKEN"])
        .stdin(Stdio::null())
        .output()
        .expect("run agentstack");

    assert!(!out.status.success(), "refusal must be an error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("needs a terminal"),
        "names the cause; got: {stderr}"
    );
    assert!(
        stderr.contains("secret set DEMO_TOKEN --value"),
        "names the copy-pasteable fix; got: {stderr}"
    );
    assert!(
        !stderr.contains("os error"),
        "no raw OS error; got: {stderr}"
    );
}
