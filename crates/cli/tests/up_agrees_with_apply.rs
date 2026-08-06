// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `up` and `apply --write` are one operation, so they may not disagree about
//! whether it succeeded.
//!
//! `up`'s render step IS `apply --write` — it calls
//! [`agentstack::commands::apply::write_quiet`] and owns no writing path of its
//! own. The two therefore make the same claim about the same project, and a
//! script that reads one exit code must not get a different verdict from the
//! other.
//!
//! The state pinned here is the one where they came apart. Every capability the
//! project has routes to the live lane, and no bridge is registered anywhere:
//! nothing reaches any tool, and nothing will until a second command runs.
//! `apply --write` calls that a refused delivery and exits 1 — the reasoning is
//! in its source: exit 0 would be the same false success its validation gate
//! already refuses to give. `up` ran the identical render, printed the identical
//! refusal, and exited 0 — and `up` is the documented new-machine command, so a
//! CI job that runs it and reads success believes an environment was set up when
//! nothing was delivered at all.
//!
//! Two witnesses, and the second is what keeps the first from being satisfied by
//! a command that simply always fails:
//!
//! 1. On the refused-delivery state the two exit codes are EQUAL and nonzero,
//!    and both name a way forward. A nonzero exit with no next step would be a
//!    worse product than the bug.
//! 2. On a project that delivers something — the same manifest plus one setting
//!    for the rendered lane — both still exit 0. Live routing on its own is not
//!    a failure; delivering nothing is.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Everything this project has is an MCP server on an MCP-capable harness: the
/// planner routes it live, and with no bridge registered nothing is served.
/// There is no instruction, setting or hook, so the rendered lane has no content
/// of its own to carry either.
const LIVE_ONLY: &str = "version = 1\n\
                         [targets]\n\
                         default = [\"claude-code\"]\n\
                         [servers.docs]\n\
                         type = \"stdio\"\n\
                         command = \"echo\"\n";

/// The control: the same live-routed server, plus one declaration that really
/// does land on disk. The routing is identical — only the delivery differs.
const LIVE_PLUS_RENDERED: &str = "version = 1\n\
                                  [targets]\n\
                                  default = [\"claude-code\"]\n\
                                  [servers.docs]\n\
                                  type = \"stdio\"\n\
                                  command = \"echo\"\n\
                                  [settings.claude-code]\n\
                                  model = \"opus\"\n";

/// The one recovery command `apply` names for this state. Both surfaces must
/// carry it, or the exit code is all the user gets.
const CONNECT: &str = "agentstack x gateway connect --all --write";

struct Run {
    text: String,
    code: i32,
}

fn run(args: &[&str], home: &Path, proj: &Path) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("the binary must run");
    Run {
        text: format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        // A signal death has no code and would otherwise silently read as a
        // pass through `!= 0`.
        code: out.status.code().expect("the process must exit normally"),
    }
}

/// A trusted, locked checkout carrying `manifest`, in its own HOME so no other
/// machine state can decide the outcome. Trust is granted because an untrusted
/// project fails for a different reason entirely, which would make the exit
/// codes below agree for reasons that have nothing to do with delivery.
fn project(root: &Path, name: &str, manifest: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = root.join(format!("{name}-home"));
    let proj = root.join(name);
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::write(proj.join(".agentstack/agentstack.toml"), manifest).unwrap();

    let locked = run(&["lock", "--write"], &home, &proj);
    assert_eq!(locked.code, 0, "fixture: lock failed:\n{}", locked.text);
    let preview = run(&["trust", "--preview"], &home, &proj);
    let json: serde_json::Value =
        serde_json::from_str(&preview.text).expect("`trust --preview` must be JSON");
    let digest = json["surface_digest"]
        .as_str()
        .expect("the preview must carry a surface digest")
        .to_string();
    let granted = run(
        &["trust", "--yes", "--consented-digest", &digest],
        &home,
        &proj,
    );
    assert_eq!(granted.code, 0, "fixture: trust failed:\n{}", granted.text);
    (home, proj)
}

// ------------------------------------------------------- 1. they must agree

/// The defect, stated as a property: on the identical project, in the identical
/// state, the two commands report the identical verdict — and it is a failure,
/// because nothing was delivered.
///
/// Each command gets its own copy of the fixture so neither can be reading a
/// state the other left behind.
#[test]
fn up_and_apply_agree_that_delivering_nothing_is_a_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let (a_home, a_proj) = project(tmp.path(), "for-apply", LIVE_ONLY);
    let (u_home, u_proj) = project(tmp.path(), "for-up", LIVE_ONLY);

    let applied = run(&["apply", "--write"], &a_home, &a_proj);
    let upped = run(&["up"], &u_home, &u_proj);

    // The premise: this is the refused-delivery state, not some other failure.
    assert!(
        applied.text.contains("nothing was delivered"),
        "fixture: apply must be refusing the delivery, or this test is measuring \
         something else:\n{}",
        applied.text
    );
    assert_eq!(
        applied.code, 1,
        "apply's own contract: a refused delivery exits 1:\n{}",
        applied.text
    );

    assert_eq!(
        upped.code, applied.code,
        "`up` runs apply's write and must not disagree with it about whether the \
         project was set up. apply exited {}, up exited {} — a CI job that runs \
         `up` on this project reads success over an environment where nothing \
         reached any tool.\n--- up ---\n{}\n--- apply ---\n{}",
        applied.code, upped.code, upped.text, applied.text
    );

    // An exit code alone strands the user. Both must name the way out — the
    // same one, since a single constant defines it.
    for (who, r) in [("apply", &applied), ("up", &upped)] {
        assert!(
            r.text.contains(CONNECT),
            "`{who}` failed without naming the command that answers it — a nonzero \
             exit with no next step is worse than the bug:\n{}",
            r.text
        );
        assert!(
            r.text
                .contains("agentstack x delivery render-locally --write"),
            "`{who}` must also name the override for a user who wants the files \
             anyway:\n{}",
            r.text
        );
    }
}

// ------------------------------------------------------------- 2. the control

/// The same live routing, one delivered setting: both exit 0.
///
/// Without this, the test above is satisfied by making `up` fail whenever a
/// capability goes live — which would fail every healthy MCP project on the
/// planet. The failure being agreed on is "nothing was delivered", never
/// "routing happened".
#[test]
fn a_project_that_delivers_something_still_exits_zero_from_both() {
    let tmp = tempfile::tempdir().unwrap();
    let (a_home, a_proj) = project(tmp.path(), "for-apply", LIVE_PLUS_RENDERED);
    let (u_home, u_proj) = project(tmp.path(), "for-up", LIVE_PLUS_RENDERED);

    let applied = run(&["apply", "--write"], &a_home, &a_proj);
    assert_eq!(
        applied.code, 0,
        "a project whose rendered lane has content delivered something:\n{}",
        applied.text
    );

    let upped = run(&["up"], &u_home, &u_proj);
    assert_eq!(
        upped.code, 0,
        "`up` must not have become a command that fails whenever anything routes \
         live:\n{}",
        upped.text
    );
    assert!(
        !upped.text.contains("nothing was delivered"),
        "fixture: this project delivers a setting, so the refusal must not \
         appear:\n{}",
        upped.text
    );
}
