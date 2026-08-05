// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The executor's result-file bind is size-bounded BY THE KERNEL — Docker-gated.
//!
//! `tools_execute` gives its guest exactly one writable host path: a
//! pre-created, chmod-0666 file bound read-write at `/agentstack-result.json`
//! (`crates/cli/src/execution.rs`). Node's permission model
//! (`--allow-fs-write=/agentstack-result.json`) narrows WHICH path the guest
//! may write; on its own it says nothing about HOW MUCH, so before the
//! hardened profile carried `file_size_bytes` a guest could write until the
//! host disk filled.
//!
//! A mount option cannot fix that. `tmpfs size=16m` caps a container-private
//! filesystem; a bind's bytes land in the host inode, so nothing about the
//! mount bounds them. The bound therefore sits on the writer:
//! `SandboxSecurity::hardened_executor().file_size_bytes` becomes
//! `RLIMIT_FSIZE` (Docker `--ulimit fsize=`), which the kernel checks on every
//! write to every filesystem, the bind included. Capabilities are dropped and
//! `no-new-privileges` is set, so the guest cannot raise its own hard limit.
//!
//! Two runs against the SAME hardened spec and the same bind shape:
//! - a guest that writes far past the cap dies at the write (`SIGXFSZ`), and
//!   the host file stops at the cap;
//! - a legitimate result still round-trips byte for byte to the host.
//!
//! The host-side `MAX_RESULT_BYTES` read refusal is unchanged and complementary
//! — this test covers the write; `parse_result` covers the read.
//!
//! Compiles only with `--features sandbox`; SKIPS when no Docker daemon or
//! busybox image. Run it where Docker exists:
//!   cargo test -p agentstack --features sandbox --test sandbox_result_cap -- --nocapture
#![cfg(feature = "sandbox")]

use std::fs;
use std::path::Path;
use std::process::Command;

use agentstack_policy::CompiledRuleset;
use agentstack_runtime::docker::DockerSandbox;
use agentstack_runtime::{run, Exit, Mount, NetworkPolicy, SandboxSecurity, SandboxSpec};

const IMAGE: &str = "busybox:latest";
const GUEST_PATH: &str = "/agentstack-result.json";

/// The cap the hardened executor profile carries, as bytes.
fn cap_bytes() -> u64 {
    SandboxSecurity::hardened_executor()
        .file_size_bytes
        .expect("the hardened executor profile bounds guest file writes") as u64
}

/// Not Docker-gated: the two limits must stay in a deliberate relationship.
/// The kernel write cap has to sit comfortably ABOVE the host's read refusal,
/// so it can never clip a result the host would have accepted, while staying
/// well under the 16 MiB `/tmp` tmpfs so no single file can monopolise it.
#[test]
fn write_cap_is_a_deliberate_multiple_of_the_read_refusal() {
    let cap = cap_bytes();
    let read_refusal = agentstack_executor::MAX_RESULT_BYTES as u64;
    assert_eq!(
        cap,
        4 * read_refusal,
        "the guest write cap is four times MAX_RESULT_BYTES"
    );
    assert!(
        cap < 16 * 1024 * 1024,
        "one file must not be able to fill the 16 MiB /tmp tmpfs"
    );
}

fn docker_and_image() -> Option<DockerSandbox> {
    let sandbox = match DockerSandbox::connect() {
        Ok(sandbox) => sandbox,
        Err(error) => {
            eprintln!("SKIP: no Docker daemon ({error})");
            return None;
        }
    };
    // bollard does not auto-pull.
    let pulled = Command::new("docker")
        .args(["pull", IMAGE])
        .status()
        .map(|status| status.success())
        .unwrap_or(false);
    if !pulled {
        eprintln!("SKIP: cannot pull {IMAGE}");
        return None;
    }
    Some(sandbox)
}

/// The host side of the bind, prepared exactly as `prepare_files` does it:
/// pre-created, empty, and writable by the container's non-root uid.
fn prepare_result_file(path: &Path) {
    fs::write(path, "").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o666)).unwrap();
    }
}

/// The executor's spec shape: the hardened profile, and one read-write file
/// bind. Only the guest command differs between the two cases.
fn spec(result_host_path: &Path, command: &str) -> SandboxSpec {
    SandboxSpec {
        image: IMAGE.to_string(),
        command: vec!["sh".into(), "-c".into(), command.into()],
        mounts: vec![Mount {
            host: result_host_path.display().to_string(),
            container: GUEST_PATH.into(),
            read_only: false,
        }],
        workdir: "/".into(),
        env: vec![],
        network: NetworkPolicy::None,
        ruleset: CompiledRuleset::default(),
        security: SandboxSecurity::hardened_executor(),
    }
}

/// Runs the guest and returns its exit, or `None` when the daemon could not
/// run the container at all (an infra gap — skip, don't fail).
///
/// `Exit::code` is `None` for a container that exited NON-zero: the Docker
/// backend gets that status back from bollard as an error and keeps `None`.
/// So the callers below compare against `Some(0)` rather than treating `None`
/// as "unknown" — `None` here means "not a clean exit".
fn run_guest(sandbox: &DockerSandbox, spec: &SandboxSpec) -> Option<Exit> {
    let mut output = Vec::new();
    match run(
        sandbox,
        spec,
        &mut |chunk| output.extend_from_slice(&chunk.bytes),
        &mut |_event| {},
    ) {
        Ok(exit) => {
            eprintln!(
                "guest exit={:?} output={}",
                exit.code,
                String::from_utf8_lossy(&output).trim()
            );
            Some(exit)
        }
        Err(error) => {
            eprintln!("SKIP: could not run the test container ({error})");
            None
        }
    }
}

#[test]
fn a_guest_write_past_the_cap_fails_at_the_write_and_a_real_result_round_trips() {
    let Some(sandbox) = docker_and_image() else {
        return;
    };
    let cap = cap_bytes();
    let dir = assert_fs::TempDir::new().unwrap();

    // 1) HOSTILE: write many times the cap into the bind. The kernel must stop
    //    it at the cap — the guest cannot spill onto host disk beyond that.
    let hostile = dir.join("hostile-result.json");
    prepare_result_file(&hostile);
    let requested_mib = 64;
    let exit = run_guest(
        &sandbox,
        &spec(
            &hostile,
            &format!("dd if=/dev/zero of={GUEST_PATH} bs=1M count={requested_mib}"),
        ),
    );
    let Some(exit) = exit else { return };
    assert_ne!(
        exit.code,
        Some(0),
        "the over-cap write must fail inside the container, not succeed quietly"
    );
    let written = fs::metadata(&hostile).unwrap().len();
    assert!(
        written <= cap,
        "the host file grew to {written} bytes, past the {cap}-byte kernel cap"
    );
    assert!(
        written < requested_mib * 1024 * 1024,
        "the guest wrote everything it asked for — nothing bounded it"
    );

    // 2) LEGITIMATE: a normal-sized result still lands on the host intact.
    let good = dir.join("good-result.json");
    prepare_result_file(&good);
    let payload = r#"{"ok":true,"rows":[1,2,3]}"#;
    let exit = run_guest(
        &sandbox,
        &spec(&good, &format!("printf '%s' '{payload}' > {GUEST_PATH}")),
    );
    let Some(exit) = exit else { return };
    assert_eq!(
        exit.code,
        Some(0),
        "a legitimate result write must still succeed under the cap"
    );
    assert_eq!(
        fs::read_to_string(&good).unwrap(),
        payload,
        "the result must round-trip byte for byte to the host"
    );

    eprintln!(
        "\nRLIMIT_FSIZE bounded the bind at {cap} bytes: the {requested_mib} MiB \
         write stopped at {written}, the real result round-tripped."
    );
}
