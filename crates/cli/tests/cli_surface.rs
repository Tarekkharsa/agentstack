// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The consolidated CLI surface: merged verbs parse (`lock --update/--upgrade`,
//! the `report` umbrella, `gateway`, `lib pack-init`), retired top-level names
//! are really gone, and clap's own debug assertions hold for the whole tree.

use agentstack::cli::Cli;
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
        // The machine-invoked entrypoint written into harness configs must
        // keep parsing exactly as `connect` renders it.
        vec!["agentstack", "mcp", "--auto-project"],
    ] {
        Cli::try_parse_from(&argv).unwrap_or_else(|e| panic!("{argv:?} must parse: {e}"));
    }
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
