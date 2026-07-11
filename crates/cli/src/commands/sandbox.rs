//! `agentstack run --sandbox` — launch the harness inside a container whose
//! egress is enforced by the AgentStack proxy (Phase 2, ROADMAP items 1-3).
//!
//! Two halves: [`build_sandbox_spec`] turns a bundle into a backend-agnostic
//! [`SandboxSpec`] (pure, unit-tested in every build), and `execute_sandbox`
//! runs it — behind the `sandbox` feature so bollard + the egress proxy stay
//! out of standard builds. Without that feature `run --sandbox` fails with a
//! clear rebuild hint rather than pretending.
//!
//! What it does with the feature on: mounts the project as the container's
//! workspace, stands up the egress proxy (one identity for the sandbox) from
//! the effective compiled policy, points the container's `HTTPS_PROXY` at it,
//! and records the run's lifecycle + every egress decision to the run's
//! flight-recorder log (readable with `agentstack report <run>`). The proxy is
//! a CONNECT forward proxy, so it gates the container's HTTPS egress (the model
//! API, HTTP MCP servers); an allowed host still reaches out — the honest claim
//! is *unapproved egress is blocked*, not that exfiltration is impossible.

use std::path::Path;

use anyhow::{Context, Result};
use owo_colors::OwoColorize;

use crate::cli::RunArgs;
use agentstack_policy::CompiledRuleset;
use agentstack_runtime::{Mount, NetworkPolicy, SandboxSpec};

/// Where the project is mounted inside the container.
const WORKSPACE: &str = "/workspace";

/// The image the sandbox runs the agent CLI in. Overridable with
/// `AGENTSTACK_SANDBOX_IMAGE` (used by the integration test); otherwise a
/// maintainer-provided default that carries the harness binary.
fn sandbox_image() -> String {
    std::env::var("AGENTSTACK_SANDBOX_IMAGE")
        .unwrap_or_else(|_| "agentstack/sandbox:latest".to_string())
}

/// Build the sandbox spec for one run: mount the project as a read-write
/// workspace, run `command` there routed through the egress proxy, carry the
/// run id in the env (like host-mode `run`), and attach the effective compiled
/// ruleset the proxy enforces. The `HTTPS_PROXY` env is added later, once the
/// proxy's port is known.
pub fn build_sandbox_spec(
    workspace_host: &Path,
    command: Vec<String>,
    ruleset: CompiledRuleset,
    run_id: &str,
) -> SandboxSpec {
    SandboxSpec {
        image: sandbox_image(),
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
        network: NetworkPolicy::ProxyOnly {
            endpoint: "host.docker.internal".to_string(),
        },
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
        "  {} egress is routed through the AgentStack proxy; \
         review it after with `agentstack report {}`.",
        "🛡".cyan(),
        run_id
    );

    execute_sandbox(spec, &run_id, &args.harness)
}

#[cfg(feature = "sandbox")]
fn execute_sandbox(mut spec: SandboxSpec, run_id: &str, server: &str) -> Result<()> {
    use std::io::Write;
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    // One flight-recorder log for the run, shared by the egress proxy (async,
    // its own threads) and the sandbox lifecycle (this thread). Append is
    // best-effort and O_APPEND-atomic per line.
    let log = Arc::new(agentstack_recorder::RunLog::create(run_id));

    // Stand up the egress proxy for this run from the compiled policy, bound on
    // 0.0.0.0 so the container reaches it via host.docker.internal. Attributed
    // to the harness as the sandbox's single egress identity.
    let sink_log = Arc::clone(&log);
    let sink: agentstack_egress::EventSink = Arc::new(move |ev| {
        if let Some(l) = sink_log.as_ref() {
            l.append(&ev);
        }
    });
    let bridge = agentstack_egress::BlockingBridge::start_on(
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        std::slice::from_ref(&server.to_string()),
        spec.ruleset.clone(),
        sink,
    )
    .context("starting the egress proxy")?;
    let port = bridge
        .endpoints()
        .first()
        .context("egress proxy bound no endpoint")?
        .addr
        .port();

    // Point the container's HTTPS egress at the proxy (the model API + HTTP MCP
    // servers use CONNECT, which this proxy gates). Both cases so any client
    // convention is covered.
    let proxy_url = format!("http://host.docker.internal:{port}");
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        spec.env.push((key.to_string(), proxy_url.clone()));
    }

    let backend = agentstack_runtime::docker::DockerSandbox::connect()
        .map_err(|e| anyhow::anyhow!("cannot reach Docker ({e}) — is the daemon running?"))?;

    let mut on_output = |chunk: agentstack_runtime::StreamChunk| match chunk.stream {
        agentstack_runtime::Stream::Stdout => {
            let _ = std::io::stdout().write_all(&chunk.bytes);
        }
        agentstack_runtime::Stream::Stderr => {
            let _ = std::io::stderr().write_all(&chunk.bytes);
        }
    };
    let ev_log = Arc::clone(&log);
    let mut on_event = |ev: agentstack_recorder::RunEvent| {
        if let Some(l) = ev_log.as_ref() {
            l.append(&ev);
        }
    };

    let exit = agentstack_runtime::run(&backend, &spec, &mut on_output, &mut on_event)?;
    drop(bridge); // stop the proxy now the run is done

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
fn execute_sandbox(_spec: SandboxSpec, _run_id: &str, _server: &str) -> Result<()> {
    anyhow::bail!(
        "sandbox support is not compiled into this build — rebuild with \
         `cargo build --features sandbox` (it also needs a running Docker daemon)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spec_mounts_workspace_and_routes_egress_through_the_proxy() {
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
        assert!(
            matches!(spec.network, NetworkPolicy::ProxyOnly { .. }),
            "egress routes through the proxy, not open network"
        );
        // The run id rides in the env, like host-mode run.
        assert!(spec
            .env
            .iter()
            .any(|(k, v)| k == agentstack_recorder::RUN_ID_ENV && v == "r-abc"));
        assert_eq!(spec.workspace(), "/home/me/proj");
    }
}
