//! `agentstack run --sandbox` — launch the harness inside a container whose
//! egress is enforced by the AgentStack proxy (Phase 2, ROADMAP items 1-3).
//!
//! One seam assembles a run and one seam executes it: [`ExecutionPlan::build`]
//! turns a loaded bundle into an immutable, verified plan (trust checked,
//! effective policy compiled, mounts + command + mode resolved — the single
//! place the security model is assembled, so no command re-derives or skips a
//! piece), and either [`ExecutionPlan::display`] shows it (`--plan`, no Docker)
//! or [`execute_plan`] runs it — the latter behind the `sandbox` feature so
//! bollard + the egress proxy stay out of standard builds. Without that feature
//! `run --sandbox` fails with a clear rebuild hint rather than pretending.
//!
//! What it does with the feature on: mounts the project as the container's
//! workspace (read-only unless `[policy.filesystem]` write covers it — the
//! kernel enforces the bind mode, not the harness), stands up the egress
//! proxy (one identity for the sandbox) from the effective compiled policy,
//! points the container's `HTTPS_PROXY` at it,
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

/// The egress-proxy sidecar image (lockdown mode). Overridable with
/// `AGENTSTACK_EGRESS_IMAGE`; default matches the tag
/// `docker/egress-proxy.Dockerfile` builds.
#[cfg(feature = "sandbox")]
fn egress_image() -> String {
    std::env::var("AGENTSTACK_EGRESS_IMAGE")
        .unwrap_or_else(|_| "agentstack/egress-proxy:latest".to_string())
}

/// Build the sandbox spec for one run: mount the project as the workspace,
/// run `command` there routed through the egress proxy, carry the run id in
/// the env (like host-mode `run`), and attach the effective compiled ruleset
/// the proxy enforces. The `HTTPS_PROXY` env is added later, once the proxy's
/// port is known.
///
/// The workspace mounts read-only unless the effective `[policy.filesystem]`
/// write scope covers it — sandbox writes are deny-by-default (the semantics
/// live in `CompiledRuleset::workspace_write_decision`; this function just
/// asks). The backend turns `read_only` into a `:ro` bind, so the kernel
/// enforces it, not the harness.
pub fn build_sandbox_spec(
    workspace_host: &Path,
    command: Vec<String>,
    ruleset: CompiledRuleset,
    run_id: &str,
) -> SandboxSpec {
    let read_only = ruleset.workspace_write_decision().is_err();
    SandboxSpec {
        image: sandbox_image(),
        command,
        mounts: vec![Mount {
            host: workspace_host.display().to_string(),
            container: WORKSPACE.to_string(),
            read_only,
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

/// The single, immutable description of one sandbox run. Everything the run
/// needs — verified trust, effective compiled policy, the exact mounts, command,
/// egress mode, and run id — is assembled and checked in ONE place
/// ([`ExecutionPlan::build`]), so no command re-derives (or silently skips) a
/// piece of the security model. A command then either [`execute`](Self::execute)s
/// the plan or [`display`](Self::display)s it (`--plan`).
pub struct ExecutionPlan {
    /// Fresh run id — also the flight-recorder log path and the sandbox's
    /// `AGENTSTACK_RUN_ID` env.
    pub run_id: String,
    /// The egress identity the proxy attributes this run's traffic to.
    pub server: String,
    /// The backend-agnostic run description (image, command, mounts, workdir,
    /// base env, network mode, and the effective compiled ruleset).
    pub spec: SandboxSpec,
    /// Whether this is lockdown mode (no-direct-route sidecar) vs host-proxy.
    pub lockdown: bool,
    /// The bundle's trust state at plan time — the "verified content identity".
    pub trust: agentstack_trust::TrustState,
    /// Why the workspace mounts read-only, if it does (for the banner/display).
    pub fs_readonly_reason: Option<String>,
}

impl ExecutionPlan {
    /// Assemble and verify one sandbox run from a loaded context. This is the
    /// single seam where trust is checked, the effective (machine ∩ bundle)
    /// policy is compiled, the command + mounts are resolved, and the mode is
    /// chosen — no downstream command repeats any of it.
    pub fn build(ctx: &crate::commands::Context, args: &RunArgs) -> Result<ExecutionPlan> {
        // Verified content identity: the trust state of the project this run
        // would execute (keyed on the project root, like `agentstack trust .`).
        let project_root = agentstack_core::manifest::project_root_of(&ctx.dir);
        let trust = crate::trust::check(&project_root);

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

        // The effective compiled policy the proxy enforces (its version is
        // baked in). The read-only mount decision is read off it here so the
        // plan can explain it without recompiling downstream.
        let ruleset = crate::render::ruleset_for(&ctx.loaded.manifest);
        let fs_readonly_reason = ruleset.workspace_write_decision().err();
        let run_id = crate::runs::gen_id();
        let spec = build_sandbox_spec(&ctx.dir, command, ruleset, &run_id);

        Ok(ExecutionPlan {
            run_id,
            server: args.harness.clone(),
            spec,
            lockdown: args.lockdown,
            trust,
            fs_readonly_reason,
        })
    }

    /// The human-readable plan: what a run WOULD do. Printed by `--plan` (no
    /// Docker needed) and as the run banner. The first line names the trust
    /// state so an untrusted run is never silent. `include_command` appends the
    /// exact argv — on for `--plan` (the whole point of a dry run), off for the
    /// live run banner, whose stdout interleaves with the sandboxed program's
    /// output (echoing the command there would pollute that stream).
    pub fn display(&self, workspace_host: &Path, include_command: bool) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let trust_line = match self.trust {
            agentstack_trust::TrustState::Trusted => format!("{}", "trusted".green()),
            agentstack_trust::TrustState::Changed => format!(
                "{} — content changed since you trusted it; re-review with `agentstack trust .`",
                "CHANGED".yellow()
            ),
            agentstack_trust::TrustState::Untrusted => format!(
                "{} — not reviewed; `agentstack trust .` to review first",
                "UNTRUSTED".yellow()
            ),
        };
        let _ = writeln!(
            s,
            "{} sandboxing {} (run {}) — bundle {trust_line}",
            "▶".green(),
            self.server.bold(),
            self.run_id.dimmed()
        );
        match &self.fs_readonly_reason {
            Some(why) => {
                let _ = writeln!(
                    s,
                    "  workspace: {} → {} {} — {why}",
                    workspace_host.display(),
                    WORKSPACE.dimmed(),
                    "read-only".yellow()
                );
            }
            None => {
                let _ = writeln!(
                    s,
                    "  workspace: {} → {} {} ([policy.filesystem] write covers it)",
                    workspace_host.display(),
                    WORKSPACE.dimmed(),
                    "read-write".green()
                );
            }
        }
        if self.lockdown {
            let _ = writeln!(
                s,
                "  {} lockdown: no host route, no internet — the container's only \
                 peer is the egress sidecar. Review it with `agentstack report {}`.",
                "🔒".cyan(),
                self.run_id
            );
        } else {
            let _ = writeln!(
                s,
                "  {} egress is routed through the AgentStack proxy; \
                 review it after with `agentstack report {}`.",
                "🛡".cyan(),
                self.run_id
            );
        }
        if include_command {
            let _ = write!(s, "  command: {}", self.spec.command.join(" ").dimmed());
        } else {
            // Trim the trailing newline the mode line left, so the banner
            // doesn't end with a blank line.
            while s.ends_with('\n') {
                s.pop();
            }
        }
        s
    }
}

/// Entry point for `agentstack run --sandbox`.
pub fn run_sandboxed(dir: Option<&Path>, args: &RunArgs) -> Result<()> {
    let ctx = crate::commands::load(dir)?;
    let workspace_host = ctx.dir.clone();
    let plan = ExecutionPlan::build(&ctx, args)?;

    // `--plan`: show exactly what would run (trust, policy, mode, command) and
    // stop — no Docker, no feature needed. "Display the plan" instead of run it.
    if args.plan {
        println!("{}", plan.display(&workspace_host, true));
        return Ok(());
    }

    println!("{}", plan.display(&workspace_host, false));
    // Surface — but do not block — an unreviewed bundle: the sandbox contains
    // it, but its declared policy hasn't been reviewed. (Blocking would be a
    // one-line change here if the maintainer wants trust required.)
    if plan.trust != agentstack_trust::TrustState::Trusted {
        eprintln!(
            "  {} running an unreviewed bundle sandboxed — machine policy still \
             applies, but review it with `agentstack trust .`",
            "⚠".yellow()
        );
    }

    execute_plan(plan)
}

/// Connect to Docker and stream the sandbox container to completion, reporting
/// its exit. Shared by both modes; the caller has already set `spec.network`
/// and `HTTPS_PROXY`, and stood up whatever egress layer the mode needs.
#[cfg(feature = "sandbox")]
fn run_container_to_completion(
    spec: &SandboxSpec,
    log: &std::sync::Arc<Option<agentstack_recorder::RunLog>>,
    backend: &agentstack_runtime::docker::DockerSandbox,
) -> Result<()> {
    use std::io::Write;
    use std::sync::Arc;

    let mut on_output = |chunk: agentstack_runtime::StreamChunk| match chunk.stream {
        agentstack_runtime::Stream::Stdout => {
            let _ = std::io::stdout().write_all(&chunk.bytes);
        }
        agentstack_runtime::Stream::Stderr => {
            let _ = std::io::stderr().write_all(&chunk.bytes);
        }
    };
    let ev_log = Arc::clone(log);
    let mut on_event = |ev: agentstack_recorder::RunEvent| {
        if let Some(l) = ev_log.as_ref() {
            l.append(&ev);
        }
    };

    let exit = agentstack_runtime::run(backend, spec, &mut on_output, &mut on_event).with_context(
        || {
            format!(
                "running the sandbox container (image `{}`). If that image is \
                 missing, build a runner from docker/sandbox.Dockerfile and set \
                 AGENTSTACK_SANDBOX_IMAGE to its tag.",
                spec.image
            )
        },
    )?;
    match exit.code {
        Some(0) => {
            println!("\n{} sandbox exited cleanly.", "✓".green());
            Ok(())
        }
        Some(c) => anyhow::bail!("sandbox exited with code {c}"),
        None => anyhow::bail!("sandbox was killed by a signal"),
    }
}

/// Add the four `HTTPS_PROXY`/`HTTP_PROXY` spellings pointing at `url`, so any
/// client convention inside the container is covered.
#[cfg(feature = "sandbox")]
fn set_proxy_env(spec: &mut SandboxSpec, url: &str) {
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        spec.env.push((key.to_string(), url.to_string()));
    }
}

/// A fresh per-run credential the sandbox presents to its egress proxy — hex of
/// 32 random bytes from the same OS entropy source agentstack uses for its other
/// locally-minted secrets. Authenticates the container to the proxy so the
/// broadly-bound listener can't be used as an open relay by anything else.
#[cfg(feature = "sandbox")]
fn mint_proxy_token() -> String {
    use std::fmt::Write;
    agentstack_core::util::random_bytes()
        .iter()
        .fold(String::with_capacity(64), |mut s, b| {
            let _ = write!(s, "{b:02x}");
            s
        })
}

/// Execute an assembled [`ExecutionPlan`]. Creates the run's flight-recorder log
/// ONCE (fail closed — "nothing trusted runs unobserved") and mints ONE per-run
/// proxy token, then dispatches to the mode's executor. The executors no longer
/// re-create either — the plan is the single source of what runs.
#[cfg(feature = "sandbox")]
fn execute_plan(plan: ExecutionPlan) -> Result<()> {
    use std::sync::Arc;

    let log = Arc::new(agentstack_recorder::RunLog::create(&plan.run_id));
    if log.is_none() {
        anyhow::bail!(
            "could not create the run log for run {} under ~/.agentstack/runs \
             — refusing to run a sandbox unobserved",
            plan.run_id
        );
    }
    // A per-run token authenticates the sandbox to its proxy: the listener must
    // bind a broad address so the container can reach it, so the token — not the
    // bind — is what stops anything else that can route to it from using it.
    let token = mint_proxy_token();

    if plan.lockdown {
        execute_lockdown(plan.spec, &plan.run_id, &plan.server, log, token)
    } else {
        execute_proxy(plan.spec, &plan.server, log, token)
    }
}

/// Host-process-proxy mode (`--sandbox`): the container gets an ordinary bridge
/// network and its `HTTPS_PROXY` points at a proxy running on the host. This
/// gates the agent's *configured* egress; `--lockdown` is the stronger mode.
/// The run log and per-run token are supplied by [`execute_plan`].
#[cfg(feature = "sandbox")]
fn execute_proxy(
    mut spec: SandboxSpec,
    server: &str,
    log: std::sync::Arc<Option<agentstack_recorder::RunLog>>,
    proxy_token: String,
) -> Result<()> {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    // Stand up the egress proxy for this run from the compiled policy, bound on
    // 0.0.0.0 so the container reaches it via host.docker.internal. Attributed
    // to the harness as the sandbox's single egress identity.
    let sink_log = Arc::clone(&log);
    let sink: agentstack_egress::EventSink = Arc::new(move |ev| {
        if let Some(l) = sink_log.as_ref() {
            l.append(&ev);
        }
    });
    // Anti-SSRF address check is on by default; the demo dials the host gateway
    // (host.docker.internal), so it opts out via env — never set in real use.
    let proxy_config = agentstack_egress::proxy::ProxyConfig {
        allow_local_targets: matches!(
            std::env::var("AGENTSTACK_ALLOW_LOCAL_TARGETS")
                .ok()
                .as_deref(),
            Some("1") | Some("true") | Some("yes")
        ),
        auth_token: Some(proxy_token.clone()),
    };
    let bridge = agentstack_egress::BlockingBridge::start_on_with(
        IpAddr::V4(Ipv4Addr::UNSPECIFIED),
        std::slice::from_ref(&server.to_string()),
        spec.ruleset.clone(),
        sink,
        proxy_config,
    )
    .context("starting the egress proxy")?;
    let port = bridge
        .endpoints()
        .first()
        .context("egress proxy bound no endpoint")?
        .addr
        .port();

    // The credentials ride in the proxy URL's userinfo; curl et al. turn that
    // into `Proxy-Authorization: Basic …` on CONNECT.
    set_proxy_env(
        &mut spec,
        &format!("http://agentstack:{proxy_token}@host.docker.internal:{port}"),
    );

    let backend = agentstack_runtime::docker::DockerSandbox::connect()
        .map_err(|e| anyhow::anyhow!("cannot reach Docker ({e}) — is the daemon running?"))?;
    let result = run_container_to_completion(&spec, &log, &backend);
    drop(bridge); // stop the proxy now the run is done
    result
}

/// Lockdown mode (`--lockdown`): the container is attached ONLY to an internal
/// network with no host route and no internet; its sole reachable peer is the
/// egress-proxy sidecar the runtime stands up on that network. Ignoring the
/// proxy env then reaches nothing — the confinement is topological, not
/// convention.
#[cfg(feature = "sandbox")]
fn execute_lockdown(
    mut spec: SandboxSpec,
    run_id: &str,
    server: &str,
    log: std::sync::Arc<Option<agentstack_recorder::RunLog>>,
    proxy_token: String,
) -> Result<()> {
    use std::sync::Arc;

    // Hand the sidecar its policy: the compiled ruleset serialized to a host
    // file, bind-mounted read-only into the proxy container. Staged in a
    // run-scoped temp dir kept until the run — and the sidecar — are done, then
    // removed. (No `tempfile` dep in the shipped build; std is enough here.)
    let ruleset_dir = std::env::temp_dir().join(format!("agentstack-lock-{run_id}"));
    std::fs::create_dir_all(&ruleset_dir).context("creating the ruleset staging dir")?;
    let ruleset_path = ruleset_dir.join("ruleset.json");
    std::fs::write(
        &ruleset_path,
        serde_json::to_vec(&spec.ruleset).context("serializing the compiled ruleset")?,
    )
    .context("writing the ruleset for the sidecar")?;

    // The sidecar reports each egress decision as a JSON line; parse it into a
    // RunEvent and append to the same flight recorder the sandbox lifecycle
    // writes. Runtime forwards the raw line so serde stays out of that crate.
    let sink_log = Arc::clone(&log);
    let sink: agentstack_runtime::LockdownSink = Arc::new(move |line: &str| {
        if let (Some(l), Ok(ev)) = (
            sink_log.as_ref(),
            serde_json::from_str::<agentstack_recorder::RunEvent>(line),
        ) {
            l.append(&ev);
        }
    });

    let backend = agentstack_runtime::docker::DockerSandbox::connect()
        .map_err(|e| anyhow::anyhow!("cannot reach Docker ({e}) — is the daemon running?"))?;

    // The per-run token (from execute_plan) authenticates the sandbox to the
    // sidecar: the sidecar reads it from its env; the sandbox presents it via
    // the proxy URL userinfo.
    let lock = agentstack_runtime::Lockdown::start(
        &backend,
        run_id,
        std::slice::from_ref(&server.to_string()),
        &ruleset_path.display().to_string(),
        &egress_image(),
        Some(proxy_token),
        sink,
    )
    .context("standing up the egress lockdown (is the sidecar image built?)")?;

    // Attach the sandbox to the internal network and point it at the sidecar
    // (proxy_endpoint carries the token in its userinfo).
    spec.network = NetworkPolicy::Lockdown {
        network: lock.internal_network().to_string(),
    };
    set_proxy_env(&mut spec, &lock.proxy_endpoint());

    let result = run_container_to_completion(&spec, &log, &backend);
    drop(lock); // tear down the sidecar + networks first (it holds the mount)
    let _ = std::fs::remove_dir_all(&ruleset_dir); // then drop the staged ruleset
    result
}

#[cfg(not(feature = "sandbox"))]
fn execute_plan(_plan: ExecutionPlan) -> Result<()> {
    anyhow::bail!(
        "sandbox support is not compiled into this build — rebuild with \
         `cargo build --features sandbox` (it also needs a running Docker daemon)."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ruleset whose effective `[policy.filesystem]` write scope is exactly
    /// `scopes` (as a machine-layer grant).
    fn ruleset_with_write(scopes: &[&str]) -> CompiledRuleset {
        let machine = agentstack_core::manifest::Policy {
            filesystem: agentstack_core::manifest::FsPolicy {
                read: vec![],
                write: scopes.iter().map(|s| s.to_string()).collect(),
            },
            ..Default::default()
        };
        agentstack_policy::compile(&machine, &Default::default(), &[])
    }

    /// The plan's display names the trust state, the mode, the workspace mount,
    /// and the exact command — the `--plan` dry-run and the run banner both use
    /// it, so an untrusted run is never silent.
    #[test]
    fn plan_display_names_trust_mode_and_command() {
        let spec = build_sandbox_spec(
            Path::new("/proj"),
            vec!["claude".into(), "--go".into()],
            CompiledRuleset::default(),
            "r-test",
        );
        let plan = ExecutionPlan {
            run_id: "r-test".into(),
            server: "claude-code".into(),
            spec,
            lockdown: true,
            trust: agentstack_trust::TrustState::Untrusted,
            fs_readonly_reason: Some("no write scope".into()),
        };
        // --plan view includes the command.
        let out = plan.display(Path::new("/proj"), true);
        assert!(out.contains("UNTRUSTED"), "trust state shown: {out}");
        assert!(out.contains("lockdown"), "mode shown: {out}");
        assert!(out.contains("read-only"), "mount decision shown: {out}");
        assert!(out.contains("claude --go"), "command shown: {out}");
        assert!(out.contains("r-test"), "run id shown: {out}");
        // The live-run banner omits the command (its stdout mixes with the
        // sandboxed program's output).
        let banner = plan.display(Path::new("/proj"), false);
        assert!(
            !banner.contains("command:"),
            "banner omits command: {banner}"
        );

        // Trusted, non-lockdown reads the other way.
        let spec2 = build_sandbox_spec(
            Path::new("/proj"),
            vec!["x".into()],
            ruleset_with_write(&["./**"]),
            "r-2",
        );
        let plan2 = ExecutionPlan {
            run_id: "r-2".into(),
            server: "codex".into(),
            spec: spec2,
            lockdown: false,
            trust: agentstack_trust::TrustState::Trusted,
            fs_readonly_reason: None,
        };
        let out2 = plan2.display(Path::new("/proj"), true);
        assert!(out2.contains("trusted"), "{out2}");
        assert!(out2.contains("read-write"), "{out2}");
        assert!(out2.contains("proxy"), "{out2}");
    }

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

    /// Sandbox workspace writes are deny-by-default: no `[policy.filesystem]`
    /// write scope → the mount is read-only.
    #[test]
    fn workspace_mounts_read_only_without_a_write_scope() {
        let spec = build_sandbox_spec(
            Path::new("/home/me/proj"),
            vec!["claude".into()],
            CompiledRuleset::default(),
            "r-abc",
        );
        assert!(
            spec.mounts[0].read_only,
            "no write scope must mean a read-only workspace"
        );
        // A partial scope doesn't cover the workspace root either.
        let spec = build_sandbox_spec(
            Path::new("/home/me/proj"),
            vec!["claude".into()],
            ruleset_with_write(&["src/**"]),
            "r-abc",
        );
        assert!(spec.mounts[0].read_only, "partial scopes round down to ro");
    }

    /// A write scope covering the workspace root flips the mount to rw.
    #[test]
    fn workspace_mounts_read_write_when_the_write_scope_covers_it() {
        let spec = build_sandbox_spec(
            Path::new("/home/me/proj"),
            vec!["claude".into()],
            ruleset_with_write(&["./**"]),
            "r-abc",
        );
        assert!(!spec.mounts[0].read_only, "./** grants the workspace");
    }
}
