//! `agentstack run --sandbox` — launch the harness inside a container instead
//! of on the host (Phase 2, ROADMAP item 3).
//!
//! Two halves: [`build_sandbox_spec`] turns a bundle into a backend-agnostic
//! [`SandboxSpec`] (pure, unit-tested in every build), and `execute_sandbox`
//! runs it — behind the `sandbox` feature so bollard stays out of standard
//! builds. Without that feature `run --sandbox` fails with a clear rebuild
//! hint rather than pretending.
//!
//! Honest state for this increment: the container mounts the project workspace
//! and runs with NO network at all. The single controlled egress route (the
//! proxy) is Phase 2 item 2 (the `egress` crate) — until it lands, MCP servers
//! and the model API are unreachable from inside a sandbox.

use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::RunArgs;
use agentstack_policy::CompiledRuleset;
use agentstack_runtime::{Mount, NetworkPolicy, SandboxSpec};

/// Default image the sandbox runs the agent CLI in. A maintainer provides it
/// (the harness binary must exist inside); the gated integration test uses
/// `busybox` to prove the mechanics.
const DEFAULT_SANDBOX_IMAGE: &str = "agentstack/sandbox:latest";
/// Where the project is mounted inside the container.
const WORKSPACE: &str = "/workspace";

/// Build the sandbox spec for one run: mount the project as a read-write
/// workspace, run `command` there with no network, carry the run id in the
/// env (like host-mode `run`), and attach the effective compiled ruleset for
/// the future egress proxy to enforce.
pub fn build_sandbox_spec(
    workspace_host: &Path,
    command: Vec<String>,
    ruleset: CompiledRuleset,
    run_id: &str,
) -> SandboxSpec {
    SandboxSpec {
        image: DEFAULT_SANDBOX_IMAGE.to_string(),
        command,
        mounts: vec![Mount {
            host: workspace_host.display().to_string(),
            container: WORKSPACE.to_string(),
            read_only: false,
        }],
        workdir: WORKSPACE.to_string(),
        env: vec![(
            agentstack_recorder::RUN_ID_ENV.to_string(),
            run_id.to_string(),
        )],
        network: NetworkPolicy::None,
        ruleset,
    }
}

/// Entry point for `agentstack run --sandbox`.
pub fn run_sandboxed(dir: Option<&Path>, args: &RunArgs) -> Result<()> {
    let ctx = crate::commands::load(dir)?;
    let manifest = &ctx.loaded.manifest;
    let desc = ctx.registry.get(&args.harness).with_context(|| {
        format!(
            "unknown harness '{}' — see `agentstack adapters list`",
            args.harness
        )
    })?;
    let bin = desc
        .detect
        .bin
        .clone()
        .with_context(|| format!("{} has no known launch binary to run", desc.display))?;

    let mut command = vec![bin];
    command.extend(args.args.iter().cloned());

    let ruleset = crate::render::ruleset_for(manifest);
    let run_id = crate::runs::gen_id();
    let spec = build_sandbox_spec(&ctx.dir, command, ruleset, &run_id);

    println!(
        "{} sandboxing {} (run {})",
        "▶".green(),
        args.harness.bold(),
        run_id.dimmed()
    );
    println!(
        "  workspace: {} → {}",
        ctx.dir.display(),
        WORKSPACE.dimmed()
    );
    println!(
        "  {} no network route yet — MCP servers and the model API are unreachable from inside \
         until the egress proxy (Phase 2) lands.",
        "⚠".yellow()
    );

    execute_sandbox(&spec, &run_id)
}

#[cfg(feature = "sandbox")]
fn execute_sandbox(spec: &SandboxSpec, run_id: &str) -> Result<()> {
    use std::io::Write;

    let backend = agentstack_runtime::docker::DockerSandbox::connect()
        .map_err(|e| anyhow::anyhow!("cannot reach Docker ({e}) — is the daemon running?"))?;
    let log = agentstack_recorder::RunLog::create(run_id);

    let mut on_output = |chunk: agentstack_runtime::StreamChunk| match chunk.stream {
        agentstack_runtime::Stream::Stdout => {
            let _ = std::io::stdout().write_all(&chunk.bytes);
        }
        agentstack_runtime::Stream::Stderr => {
            let _ = std::io::stderr().write_all(&chunk.bytes);
        }
    };
    let mut on_event = |ev: agentstack_recorder::RunEvent| {
        if let Some(l) = &log {
            l.append(&ev);
        }
    };

    let exit = agentstack_runtime::run(&backend, spec, &mut on_output, &mut on_event)?;
    match exit.code {
        Some(0) => {
            println!("\n{} sandbox exited cleanly.", "✓".green());
            Ok(())
        }
        Some(c) => anyhow::bail!("sandbox exited with code {c}"),
        None => anyhow::bail!("sandbox was killed by a signal"),
    }
}

#[cfg(not(feature = "sandbox"))]
fn execute_sandbox(_spec: &SandboxSpec, _run_id: &str) -> Result<()> {
    anyhow::bail!(
        "sandbox support is not compiled into this build — rebuild with \
         `cargo build --features sandbox` (it also needs a running Docker daemon)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_mounts_workspace_and_isolates_network() {
        let spec = build_sandbox_spec(
            Path::new("/home/me/proj"),
            vec!["claude".into(), "--dangerously".into()],
            CompiledRuleset::default(),
            "r-abc",
        );
        assert_eq!(spec.command, vec!["claude", "--dangerously"]);
        assert_eq!(spec.workdir, WORKSPACE);
        assert_eq!(spec.mounts.len(), 1);
        let m = &spec.mounts[0];
        assert_eq!(m.host, "/home/me/proj");
        assert_eq!(m.container, WORKSPACE);
        assert!(!m.read_only, "workspace is read-write");
        assert_eq!(
            spec.network,
            NetworkPolicy::None,
            "no direct network in the sandbox"
        );
        // The run id rides in the env, like host-mode run.
        assert!(spec
            .env
            .iter()
            .any(|(k, v)| k == agentstack_recorder::RUN_ID_ENV && v == "r-abc"));
        assert_eq!(spec.workspace(), "/home/me/proj");
    }
}
