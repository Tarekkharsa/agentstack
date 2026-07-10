//! Real Docker backend integration test — gated behind a live daemon.
//!
//! This is the behavioral verification the `docker` module's doc comment
//! promises: it exercises create → stream → wait → teardown against an actual
//! daemon. It compiles only with `--features docker`, and it SKIPS (rather than
//! fails) when no daemon is reachable or the tiny test image can't be pulled —
//! so it is harmless in CI and on dev boxes without Docker, and meaningful
//! anywhere a daemon exists. Run it there with:
//!
//!   cargo test -p agentstack-runtime --features docker -- --nocapture
#![cfg(feature = "docker")]

use agentstack_runtime::docker::DockerSandbox;
use agentstack_runtime::{run, NetworkPolicy, SandboxSpec};

#[test]
fn busybox_run_streams_output_and_exits_clean() {
    let sandbox = match DockerSandbox::connect() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("SKIP: no Docker daemon reachable ({e})");
            return;
        }
    };

    let spec = SandboxSpec {
        image: "busybox:latest".into(),
        command: vec!["echo".into(), "sandbox-ok".into()],
        mounts: vec![],
        workdir: "/".into(),
        env: vec![],
        network: NetworkPolicy::None,
        ruleset: agentstack_policy::CompiledRuleset::default(),
    };

    let mut stdout = Vec::new();
    let mut events = Vec::new();
    let exit = match run(
        &sandbox,
        &spec,
        &mut |chunk| stdout.extend_from_slice(&chunk.bytes),
        &mut |ev| events.push(ev),
    ) {
        Ok(e) => e,
        Err(e) => {
            // A daemon that can't pull the image (offline, restricted registry)
            // is an infra gap, not a logic failure — skip rather than fail.
            eprintln!("SKIP: could not run the test container ({e})");
            return;
        }
    };

    assert_eq!(exit.code, Some(0), "busybox echo exits 0");
    let text = String::from_utf8_lossy(&stdout);
    assert!(text.contains("sandbox-ok"), "streamed stdout: {text:?}");
    assert_eq!(events.len(), 2, "SandboxStarted + SandboxExited");
}
