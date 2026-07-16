// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Docs-vs-CLI sync gate: every top-level subcommand (hidden ones included —
//! `trust`, `connect`, `mcp` are hidden but documented) must appear in
//! docs/reference.md's "All commands" inventory. The command surface grows
//! fast; a hand-maintained list silently rots without this.

use clap::CommandFactory;

#[test]
fn every_subcommand_is_documented_in_the_reference() {
    let reference = include_str!("../../../docs/reference.md");
    let section = reference
        .split("## All commands")
        .nth(1)
        .expect("docs/reference.md must keep an '## All commands' section")
        .split("\n## ")
        .next()
        .unwrap();

    let cmd = agentstack::cli::Cli::command();
    let mut missing: Vec<String> = Vec::new();
    for sc in cmd.get_subcommands() {
        let name = sc.get_name();
        if name == "help" {
            continue;
        }
        if !section.contains(name) {
            missing.push(name.to_string());
        }
    }
    assert!(
        missing.is_empty(),
        "subcommand(s) missing from the 'All commands' inventory in docs/reference.md: {missing:?}"
    );
}
