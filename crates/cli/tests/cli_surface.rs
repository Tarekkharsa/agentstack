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
        vec!["agentstack", "report", "calls", "--transcripts"],
        vec!["agentstack", "gateway", "connect", "--all"],
        vec!["agentstack", "gateway", "disconnect", "--all"],
        vec!["agentstack", "lib", "pack-init", "my-pack"],
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
    ] {
        assert!(
            Cli::try_parse_from(&argv).is_err(),
            "{argv:?} should no longer parse"
        );
    }
}
