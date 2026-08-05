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
        // `apply` honours the delivery planner, so this journey's one server
        // travels the live lane and no config file is written for it. With no
        // bridge registered, `apply --write` therefore delivers nothing and
        // exits nonzero on purpose — reporting success there would be the same
        // false success invariant 8 forbids everywhere else. The step is kept
        // (its output is exactly the vocabulary this file judges); only the
        // exit status is allowed to be the honest one, and only when the
        // transcript carries the disclosure that explains it.
        let routed_live_nothing_delivered = text.contains("routed to the live lane");
        assert!(
            ok || routed_live_nothing_delivered,
            "`agentstack {}` failed:\n{text}",
            args.join(" ")
        );
        transcript.push_str(&text);
    }

    // One carve-out, and it is invariant 8 ("claims match enforcement") beating
    // the vocabulary rule rather than an exception to it: when capabilities
    // route to the live lane and no CLI has the bridge registered, the scripted
    // import must say so and name the one command that fixes it. Staying silent
    // to protect the word "gateway" would leave the summary claiming delivery
    // that does not happen. Only that disclosure's own lines are exempt.
    let disclosure = |line: &str| {
        line.contains("NOT YET CONNECTED")
            || line.contains("register the bridge")
            || line.contains("gateway connect")
            // `doctor`'s half of the same disclosure, for the same reason.
            || line.contains("Zero-files gateway")
            || line.contains("nothing routed live is reaching it")
    };
    // The gateway section is hidden while it is all-clean, so this trust-state
    // phrase only reaches the screen because the honest bridge finding un-hides
    // the section it sits in. It is collateral of the same disclosure, not
    // vocabulary the journey reaches for — so exempt THE PHRASE, never the whole
    // line: any other advanced word sharing that line must still fail the test.
    let lower = transcript
        .lines()
        .filter(|l| !disclosure(l))
        .map(|l| {
            l.to_lowercase()
                .replace("this project is trusted for auto mode", "")
                .replace("not trusted for auto mode", "")
        })
        .collect::<Vec<_>>()
        .join("\n");
    for word in ADVANCED_VOCAB {
        assert!(
            !lower.contains(word),
            "the ordinary journey printed advanced vocabulary '{word}':\n{transcript}"
        );
    }

    // The journey really happened: the manifest exists, the render landed, and
    // doctor's only finding is the honest un-registered-bridge one — so the
    // vocabulary claim covers a working flow, not an early exit.
    //
    // This scripted journey never registers the bridge, so a clean doctor would
    // be the dishonest outcome: capabilities route to the live lane and reach
    // nothing. Invariant 8 makes that one error the correct close.
    assert!(proj.join(".agentstack/agentstack.toml").exists());
    // Exactly one finding, and it is the honest one: the un-registered bridge.
    //
    // This assertion used to expect `1 error, 1 warning`, the extra warning
    // being the file this journey imported FROM — `~/.claude.json` still holds
    // `search` and the harness still reads it. Commit `a435c4f` ("a config you
    // already had is not an abandoned render") removed that warning on
    // purpose: a GLOBAL harness config AgentStack never wrote is the user's own
    // machine environment and, on this very journey, the source of the import
    // rather than a leftover render — so `agentstack adopt` there named a
    // server the manifest already had. Zero warnings is now the correct close,
    // and it is the whole point of the fix that the most common first run in
    // the product reaches it.
    assert!(
        transcript.contains("1 error, 0 warnings"),
        "expected the bridge finding and nothing else:\n{transcript}"
    );
    // The other half of `a435c4f`, pinned so the noise cannot come back: the
    // config `init` read the servers out of is never reported as one
    // AgentStack did not write.
    assert!(
        !transcript.contains("AgentStack did not write it"),
        "the imported config must not be reported as an abandoned render:\n{transcript}"
    );
    assert!(
        transcript.contains("nothing routed live is reaching it"),
        "the one error must be the bridge finding:\n{transcript}"
    );
}

/// Mechanism nouns: the names of the parts, as opposed to the names of the
/// modes. `ADVANCED_VOCAB` above keeps *stronger features* out of the ordinary
/// journey; these keep *implementation vocabulary* off the surfaces a person
/// meets before they have seen a single result.
///
/// They are all real, all still documented, and none of them was renamed — the
/// Phase-3 change is only about which door they are behind. `--help --all`
/// still defines every one of them, on purpose, because someone eventually
/// needs to know.
const MECHANISM_NOUNS: &[&str] = &[
    "manifest",
    "adapter",
    "lockfile",
    "digest",
    "render",
    "materialize",
    "[targets]",
];

/// The default `agentstack --help` speaks the four ideas and no mechanism.
///
/// This is the single most first-contact surface there is: it is what someone
/// runs before they have decided whether to keep the tool. It used to end with
/// a glossary defining CLI, harness, adapter, and `[targets]` — five
/// mechanism nouns ahead of any result.
#[test]
fn the_default_help_speaks_the_four_ideas_and_no_mechanism() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let out = Command::new(bin)
        .arg("--help")
        .stdin(Stdio::null())
        .output()
        .expect("spawn agentstack");
    let text = String::from_utf8_lossy(&out.stdout).to_string();

    // Everything above `Options:` — the commands and the guidance, which is
    // the prose a first-time reader actually reads.
    //
    // The options block is excluded for a reason worth stating rather than
    // hiding: `--manifest-dir` is an existing flag NAME, and Phase 3's hard
    // constraint is that no existing name breaks. A flag cannot be reworded
    // without being renamed, so the honest scope of this witness is the prose,
    // not the identifiers. If `--manifest-dir` is ever softened, it will be by
    // adding an alias, not by breaking the name.
    let prose = text.split("Options:").next().unwrap_or(&text).to_string();
    let lower = prose.to_lowercase();

    for noun in MECHANISM_NOUNS {
        assert!(
            !lower.contains(noun),
            "the default --help printed the mechanism noun '{noun}' — it belongs \
             behind `--help --all`:\n{text}"
        );
    }

    // And it does teach the four ideas, so this is not merely a subtraction:
    // a help screen that says nothing would also pass the loop above.
    for idea in ["Setup", "Toolset", "Status", "Undo"] {
        assert!(
            text.contains(idea),
            "the default --help must name the idea '{idea}':\n{text}"
        );
    }
}

/// The mechanism vocabulary is *moved*, not deleted. If `--help --all` ever
/// stops defining these words, the product has quietly become less explicable
/// rather than more approachable — which is the failure mode this whole pass
/// could produce if it were done carelessly.
#[test]
fn the_full_help_still_defines_the_mechanism() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let out = Command::new(bin)
        .args(["--help", "--all"])
        .stdin(Stdio::null())
        .output()
        .expect("spawn agentstack");
    let text = String::from_utf8_lossy(&out.stdout).to_lowercase();

    for noun in ["manifest", "adapter", "lock", "[targets]"] {
        assert!(
            text.contains(noun),
            "`--help --all` must still explain '{noun}' — the words moved, they did not go away"
        );
    }
}

/// `status` leads with the four ideas too. Its first column used to be
/// "Manifest", which named the file rather than the question the line answers.
#[test]
fn status_leads_with_setup_not_the_file_name() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        "version = 1\n[servers.demo]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\n",
    )
    .unwrap();
    let stub_bin = tmp.path().join("bin");
    fs::create_dir_all(&stub_bin).unwrap();

    let (text, _ok) = run(bin, &["status"], &home, &proj, &stub_bin);

    assert!(
        text.contains("Setup"),
        "status must lead with the idea, not the file:\n{text}"
    );
    // The old label named the artifact. Nothing about the file changed — only
    // what the line is called.
    assert!(
        !text.contains("Manifest "),
        "status must not label a line with the mechanism noun:\n{text}"
    );
}
