// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The consolidated CLI surface: merged verbs parse (`lock --update/--upgrade`,
//! the `report` umbrella, `gateway`, `lib pack-init`), retired top-level names
//! are really gone, and clap's own debug assertions hold for the whole tree.

use std::collections::BTreeSet;

use agentstack::cli::{Cli, Command, SessionCmd};
use clap::{CommandFactory, Parser};

/// The `--help --all` section that lists the fixed argv a graphical panel
/// invokes. Those names are a machine contract, not human commands, so they are
/// exempt from the "must be listed by `agentstack x`" rule below.
fn inventory_contract_section() -> String {
    let inventory = agentstack::cli::full_command_inventory();
    inventory
        .split_once("Integration contract (t3code)")
        .expect("the inventory carries a labelled integration-contract section")
        .1
        .to_string()
}

#[test]
fn command_tree_is_well_formed() {
    Cli::command().debug_assert();
}

// DX witnesses for the progressive-disclosure help surface:
// `status` exists (git/docker muscle memory), the visible list stays the small
// beginner loop, the default `--help` does NOT re-dump the whole surface it just
// curated, and `--help --all` lists every top-level command so nothing hidden is
// undiscoverable.
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
        [
            "init", "status", "add", "search", "apply", "doctor", "lock", "toolset", "use", "yes",
            "run", "trust", "undo", "adopt", "secret"
        ],
        "the visible list is DERIVED, not curated by taste: a command is here \
         because the product tells someone to run it — the first-run ladder in \
         `commands::overview`, doctor's `↳ fix` column, or a machine-readable \
         `next_action` / `fix` field can emit it — or because it is the obvious \
         verb for one of the four ideas the help itself promises. `lock` and \
         `secret` are the two promotions that rule forced: both are named \
         verbatim by doctor's fix column (`agentstack lock --write` is the most \
         emitted fix in the product, `agentstack secret set <name>` the second), \
         and both used to be hidden — guidance naming a command a reader cannot \
         find is the exact defect this list exists to prevent. `adopt` stays for \
         the same reason (doctor names it for a server found in a CLI config but \
         not in the manifest, which is a FIRST-RUN state). `toolset`, `status`, \
         `undo` and `init` cover Toolset · Status · Undo · Setup. The honest \
         count the rule produces is fifteen, not ten: hiding one of the forced \
         commands to reach a rounder number would trade a real guarantee for a \
         tidier screen. \
         `lib` and `why` were briefly here on an argument from importance, \
         which is not this rule. No first-run rung, no `↳ fix` line and no \
         machine field names either: doctor's single mention of `lib` (a linked \
         folder that vanished) already prints `agentstack x lib unlink`, and \
         nothing emits `why` at all — so they sit behind `x` on the same terms \
         as `guard` and `gateway`. \
         `up`, `share`, `receive`, `workflow` and `restore` moved behind \
         `agentstack x` — none is named by any fix or ladder rung, STRATEGY.md \
         v3 puts share/receive quiet until team features arrive, and `undo` is \
         the beginner spelling of `restore`. Nothing was removed: each still \
         runs at its own name and is listed by `agentstack x`."
    );

    // Review finding H5 still holds — the default help must not re-dump the ~45
    // names it just curated. It used to share this screen with an "Also named by
    // guidance" line naming eight hidden verbs, kept there so guidance could
    // never name a command a reader cannot find. That line is gone, and the
    // guarantee did not move an inch: every one of those eight is listed by
    // `agentstack x`, which rule (e) of `guidance_is_executable` counts as
    // discoverable in one step. The line was a second, hand-maintained copy of a
    // fact the toolbox already carried — the most rot-prone thing on the screen
    // this curation exists to shorten. The guarantee itself is asserted where it
    // can be derived rather than typed: rule (e) reads the discoverable set off
    // the real binary every run.
    let after_help = cmd.get_after_help().expect("after_help exists").to_string();
    assert!(
        after_help.contains("--help --all"),
        "the short help must still name the way to the full map"
    );
    assert!(
        after_help.contains("agentstack x"),
        "the short help must name the escape hatch to everything it hides"
    );
    // The eight guidance-named hidden verbs are no longer repeated here; they
    // are discoverable one step away, under `agentstack x`, which the toolbox
    // assertion below and rule (e) both check. Nothing that guidance never names
    // may pad this screen (H5).
    for grouped_only in ["proxy", "shim", "sign", "optimize", "kill"] {
        assert!(
            !after_help.contains(grouped_only),
            "'{grouped_only}' is back in the default --help, which re-dumps the \
             surface the visible list curates (H5)"
        );
    }

    // `agentstack x` is the home of everything not visible, and it must actually
    // list them — an escape hatch that names nothing is a dead end.
    let listing = agentstack::cli::namespace_listing();
    // The verbs guidance names by a `↳ fix` line, a ladder rung or ordinary
    // prose and that are hidden. They used to be repeated on the plain help
    // screen; the toolbox is now their only listing, so this is where the
    // "guidance never names an unfindable command" guarantee is anchored on
    // this side. `lib` and `why` rejoined them when the count went back to
    // fifteen: guidance still names both (`agentstack x lib unlink` in
    // doctor's fix column, `agentstack lib list` in `explain` and `why`
    // prose), so both must be findable here in one hop.
    for named_by_guidance in [
        "gateway",
        "guard",
        "install",
        "instructions",
        "lib",
        "self",
        "session",
        "up",
        "why",
    ] {
        assert!(
            listing.contains(named_by_guidance),
            "guidance can print `agentstack {named_by_guidance} …`, so `agentstack x` \
             must list it — it is the one screen a reader is pointed at that can \
             still show them where the command lives"
        );
    }
    for c in cmd.get_subcommands() {
        let name = c.get_name();
        if name == "help" || !c.is_hide_set() {
            continue;
        }
        // Panel argv is a machine contract, not a human command; it stays in
        // `--help --all` under its own heading (asserted below).
        if inventory_contract_section().contains(name) {
            continue;
        }
        assert!(
            listing.contains(name),
            "'{name}' is hidden and is not listed by `agentstack x` — it would be \
             reachable only by already knowing its name"
        );
    }

    // Every top-level command stays discoverable, but not all of it belongs in
    // the *human* task map: the panel verbs are fixed argv a graphical client
    // invokes, and listing them beside `init` and `doctor` made the surface
    // look bigger than the product is. They live under their own heading.
    // Discoverability is the invariant; the human map being exhaustive is not.
    let inventory = agentstack::cli::full_command_inventory();
    let (human, contract_section) = inventory
        .split_once("Integration contract (t3code)")
        .expect("the inventory carries a labelled integration-contract section");
    let (task_map, full_list) = human
        .split_once("And in full:")
        .expect("the inventory leads with the task-grouped map");
    for c in cmd.get_subcommands() {
        let name = c.get_name();
        if name == "help" {
            continue;
        }
        if task_map.contains(name) {
            assert!(
                !contract_section.contains(&format!("\n  {name} ")),
                "'{name}' is in the human map, so it must not also be filed as panel-only"
            );
            continue;
        }
        assert!(
            contract_section.contains(name),
            "'{name}' is in neither the grouped task map nor the \
             integration-contract section — it would be undiscoverable"
        );
        assert!(
            !full_list.contains(&format!("\n  {name} ")),
            "'{name}' must be listed once, under the contract heading only"
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
    for hidden in ["optimize", "gateway", "diff", "settings"] {
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

/// The closure arithmetic, recomputed from the real binary rather than typed:
/// **visible ∪ namespaced ∪ panel = every top-level command, and the three sets
/// do not overlap.** Moving a verb across the split (here `lib` and `why`, back
/// behind `x`) is exactly the change that can break it — a demoted command left
/// out of the toolbox listing appears nowhere, and a promoted one left in it
/// appears twice. Names are read off the `·`-separated group lines, never by
/// substring: `run` occurs inside the toolbox's own prose.
#[test]
fn visible_namespaced_and_panel_partition_the_whole_surface() {
    /// The task headings both grouped screens use. Matching on them — rather
    /// than on "contains a `·`" — is what makes `Undo   restore`, a group of
    /// one with no separator in it, count.
    const HEADINGS: &[&str] = &[
        "Set up", "Edit", "Share", "Render", "Undo", "Protect", "Run", "Inspect",
    ];

    fn grouped_names(screen: &str) -> BTreeSet<String> {
        screen
            .lines()
            .map(str::trim_start)
            .filter_map(|l| {
                let heading = HEADINGS.iter().find(|h| l.starts_with(*h))?;
                Some(l[heading.len()..].trim())
            })
            .flat_map(|rest| rest.split('·').map(|t| t.trim().to_string()))
            .filter(|t| !t.is_empty())
            .collect()
    }

    let cmd = Cli::command();
    let all: BTreeSet<String> = cmd
        .get_subcommands()
        .map(|c| c.get_name().to_string())
        .filter(|n| n != "help")
        .collect();

    let visible: BTreeSet<String> = cmd
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
        .map(|c| c.get_name().to_string())
        .collect();
    let namespaced = grouped_names(&agentstack::cli::namespace_listing());
    let contract = inventory_contract_section();
    let panel: BTreeSet<String> = all
        .iter()
        .filter(|n| contract.contains(&format!("\n  {n} ")))
        .cloned()
        .collect();

    // Nothing lands in two places.
    for (a, an, b, bn) in [
        (
            &visible,
            "visible",
            &namespaced,
            "the `agentstack x` toolbox",
        ),
        (&visible, "visible", &panel, "the panel contract"),
        (
            &namespaced,
            "the `agentstack x` toolbox",
            &panel,
            "the panel contract",
        ),
    ] {
        let both: Vec<&String> = a.intersection(b).collect();
        assert!(
            both.is_empty(),
            "{both:?} appear in BOTH {an} and {bn} — the three sets partition the surface"
        );
    }

    // And together they cover it.
    let covered: BTreeSet<String> = visible
        .union(&namespaced)
        .cloned()
        .collect::<BTreeSet<_>>()
        .union(&panel)
        .cloned()
        .collect();
    let missing: Vec<&String> = all.difference(&covered).collect();
    assert!(
        missing.is_empty(),
        "{missing:?} are in neither the visible list, the `agentstack x` toolbox, \
         nor the panel contract — they would be reachable only by already \
         knowing the name"
    );
    let invented: Vec<&String> = covered.difference(&all).collect();
    assert!(
        invented.is_empty(),
        "{invented:?} are listed but are not real top-level commands"
    );
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
        vec!["agentstack", "why", "github"],
        vec!["agentstack", "why", "sql-review", "--json"],
        // `lib` is hidden again; the whole subcommand tree must still parse at
        // the direct spelling, not just at `agentstack x lib`.
        vec!["agentstack", "lib", "list"],
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
    let diff = runtime
        .get_subcommands()
        .find(|c| c.get_name() == "diff")
        .expect("diff command");
    assert!(diff
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
        // The embedded UI was retired in favor of the t3code integration.
        vec!["agentstack", "dashboard"],
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

// `agentstack x <cmd>` is the SAME command, not a second one. The rewrite
// happens before clap parses (`strip_namespace`), so there is one parse tree
// and one dispatch arm per verb; this test pins that equivalence, and that the
// direct spelling of a now-hidden command still parses.
#[test]
fn the_namespace_is_a_prefix_not_a_second_command() {
    let argv = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    for verb in [
        vec!["secret", "list"],
        vec!["gateway", "connect", "--all"],
        vec!["guard", "install"],
        vec!["restore", "--last", "--write"],
        vec!["up"],
        vec!["share", "team-setup"],
        vec!["workflow", "list"],
        // The two the fifteen-verb count moved back behind `x`. Nothing was
        // removed: both still parse at their own names, and a nested
        // subcommand travels with its parent through the rewrite.
        vec!["lib", "list"],
        vec!["lib", "sources"],
        vec!["why", "github"],
        vec!["why", "sql-review", "--json"],
    ] {
        let mut direct = vec!["agentstack".to_string()];
        direct.extend(verb.iter().map(|s| s.to_string()));
        let mut namespaced = vec!["agentstack".to_string(), "x".to_string()];
        namespaced.extend(verb.iter().map(|s| s.to_string()));

        // Hidden does not mean gone: the direct spelling still parses.
        Cli::try_parse_from(&direct)
            .unwrap_or_else(|e| panic!("`agentstack {}` must still run: {e}", verb.join(" ")));

        let stripped = agentstack::cli::strip_namespace(&namespaced)
            .expect("a leading `x` is the namespace and is stripped");
        assert_eq!(
            stripped,
            direct,
            "`agentstack x {0}` must become `agentstack {0}` verbatim",
            verb.join(" ")
        );
    }

    // `x` is only a namespace in first position — an argument that happens to
    // be `x` belongs to the command that took it.
    assert!(
        agentstack::cli::strip_namespace(&argv(&["agentstack", "search", "x"])).is_none(),
        "`x` after a verb is that verb's argument, not the namespace"
    );
    assert!(
        agentstack::cli::strip_namespace(&argv(&["agentstack"])).is_none(),
        "a bare invocation is not namespaced"
    );
}

/// TODO #10 is explicit that nothing is removed when a verb is demoted, and
/// `lib` and `why` are the last two to move. Parse-tree equivalence is asserted
/// above; this drives the real binary, because "still runs at its own name with
/// its own `--help`" and "the same dispatch and exit code" are properties of a
/// process, not of a `clap::Command`. Both spellings are compared on stdout,
/// stderr AND exit code — a rewrite that lost an argument would still exit 0.
#[test]
fn demoted_verbs_run_identically_at_both_spellings() {
    use std::path::Path;
    use std::process::{Command, Stdio};

    let home = tempfile::tempdir().expect("temp home");
    let proj = tempfile::tempdir().expect("temp project");

    // An isolated HOME and a minimal PATH, for the same reason
    // `guidance_is_executable` does it: an inherited developer environment
    // would change which findings appear and make the comparison meaningless.
    let run = |args: &[&str], home: &Path, proj: &Path| {
        let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
            .args(args)
            .current_dir(proj)
            .env_clear()
            .env("HOME", home)
            .env("AGENTSTACK_HOME", home.join(".agentstack"))
            .env("PATH", "/usr/bin:/bin")
            .env("NO_COLOR", "1")
            .env("TERM", "dumb")
            .stdin(Stdio::null())
            .output()
            .expect("spawn agentstack");
        (
            out.status.code(),
            String::from_utf8_lossy(&out.stdout).into_owned(),
            String::from_utf8_lossy(&out.stderr).into_owned(),
        )
    };

    for direct in [
        vec!["lib", "--help"],
        vec!["lib", "list"],
        vec!["lib", "sources"],
        vec!["why", "--help"],
        vec!["why", "no-such-capability"],
    ] {
        // First-run side effects (a library directory created on demand) belong
        // to neither spelling, so let them happen before anything is compared.
        run(&direct, home.path(), proj.path());

        let (code, stdout, stderr) = run(&direct, home.path(), proj.path());
        assert!(
            code.is_some(),
            "`agentstack {}` must still run at its own name, not die on a signal",
            direct.join(" ")
        );
        if direct[1] == "--help" {
            assert_eq!(
                code,
                Some(0),
                "a hidden command keeps its own --help: `agentstack {}`",
                direct.join(" ")
            );
            assert!(
                stdout.contains(&format!("Usage: agentstack {}", direct[0])),
                "`agentstack {}` must print its own usage; got: {stdout}",
                direct.join(" ")
            );
        }

        let namespaced: Vec<&str> = std::iter::once("x").chain(direct.iter().copied()).collect();
        let (x_code, x_stdout, x_stderr) = run(&namespaced, home.path(), proj.path());
        assert_eq!(
            (x_code, x_stdout, x_stderr),
            (code, stdout, stderr),
            "`agentstack x {0}` and `agentstack {0}` must be one command — same \
             dispatch, same output, same exit code",
            direct.join(" ")
        );
    }
}

/// One consent flag, one spelling.
///
/// Three commands bound an apply to a reviewed digest and each invented its own
/// name for the same idea: `trust --consented-digest`, `init --consented-plan`,
/// `toolset create --consented`. A caller who learned the ceremony on one verb
/// guessed wrong on the next two, and the shape is the same everywhere:
/// preview, read, pass the digest back. It is now `--consented` on all three.
///
/// The old spellings survive as clap aliases for one release, because a rename
/// that breaks a working command line is a removal. They are HIDDEN: help must
/// teach exactly one name, or the unification is only half done.
#[test]
fn the_consent_digest_has_one_spelling_and_the_old_ones_are_hidden_aliases() {
    let root = Cli::command();

    let find = |path: &[&str]| {
        let mut cmd = &root;
        for segment in path {
            cmd = cmd
                .get_subcommands()
                .find(|c| c.get_name() == *segment)
                .unwrap_or_else(|| panic!("`{}` is a subcommand", path.join(" ")));
        }
        cmd
    };

    for (path, retired) in [
        (vec!["trust"], Some("consented-digest")),
        (vec!["init"], Some("consented-plan")),
        (vec!["toolset", "create"], None),
    ] {
        let cmd = find(&path);
        let arg = cmd
            .get_arguments()
            .find(|a| a.get_long() == Some("consented"))
            .unwrap_or_else(|| panic!("`agentstack {}` must take --consented", path.join(" ")));
        if let Some(old) = retired {
            assert!(
                arg.get_all_aliases().unwrap_or_default().contains(&old),
                "`--{old}` must survive as an alias on `agentstack {}` for one release",
                path.join(" ")
            );
            assert!(
                !cmd.get_arguments().any(|a| a.get_long() == Some(old)),
                "`--{old}` must not still be a long name of its own"
            );
            let help = find(&path).clone().render_long_help().to_string();
            assert!(
                help.contains("--consented"),
                "help teaches the surviving name:\n{help}"
            );
            assert!(
                !help.contains(&format!("--{old} ")) && !help.contains(&format!("--{old}\n")),
                "help must not still offer `--{old}` as a choice:\n{help}"
            );
        }
    }

    // Both spellings parse, so nobody's script broke on the rename.
    for argv in [
        vec!["agentstack", "trust", "--yes", "--consented", "abc"],
        vec!["agentstack", "trust", "--yes", "--consented-digest", "abc"],
        vec!["agentstack", "init", "--yes", "--consented", "abc"],
        vec!["agentstack", "init", "--yes", "--consented-plan", "abc"],
        vec![
            "agentstack",
            "toolset",
            "create",
            "t",
            "--yes",
            "--consented",
            "abc",
        ],
    ] {
        Cli::try_parse_from(&argv)
            .unwrap_or_else(|e| panic!("`{}` must parse: {e}", argv.join(" ")));
    }
}
