//! `agentstack run --sandbox` — launch the harness inside a container whose
//! egress is enforced by the AgentStack proxy.
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
//! flight-recorder log (readable with `agentstack report run <id>`). The proxy is
//! a CONNECT forward proxy, so it gates the container's HTTPS egress (the model
//! API, HTTP MCP servers); an allowed host still reaches out — the honest claim
//! is *unapproved egress is blocked*, not that exfiltration is impossible.

use std::path::{Path, PathBuf};

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

/// The egress-proxy sidecar image (lockdown mode). Defaults to the GHCR
/// image `release.yml` publishes for this exact version — pinned, not
/// `:latest`, so a binary never silently picks up a newer enforcement
/// sidecar — making `--lockdown` zero-config after a release. Override with
/// `AGENTSTACK_EGRESS_IMAGE` (e.g. a locally built
/// `docker/egress-proxy.Dockerfile` tag).
#[cfg(feature = "sandbox")]
fn egress_image() -> String {
    std::env::var("AGENTSTACK_EGRESS_IMAGE").unwrap_or_else(|_| {
        // env!/concat! evaluate at compile time: the crate version is baked
        // into the binary, and release.yml refuses a tag that doesn't match.
        concat!(
            "ghcr.io/tarekkharsa/agentstack-egress-proxy:v",
            env!("CARGO_PKG_VERSION")
        )
        .to_string()
    })
}

/// Build the sandbox spec for one run: mount the project as the workspace,
/// run `command` there routed through the egress proxy, carry the run id in
/// the env (like host-mode `run`), and attach the effective compiled ruleset
/// the proxy enforces. The `HTTPS_PROXY` env is added later, once the proxy's
/// port is known.
///
/// `manifest_dir` is the dir holding `agentstack.toml`; what mounts at
/// `/workspace` is the PROJECT root it belongs to (`project_root_of`): the
/// parent for the nested `.agentstack/` layout, the dir itself for a legacy
/// root manifest. Mounting the manifest dir under the nested layout would
/// confine the agent to `.agentstack/` and hide the project's actual code —
/// and it must match the anchor `agentstack trust .` keys on. The lockdown
/// shadow existence checks in `shadow_native_config`/`wire_sandbox_gateway`
/// are rooted at the same project root and must stay in lockstep with this
/// mount.
///
/// The workspace mounts read-only unless the effective `[policy.filesystem]`
/// write scope covers it — sandbox writes are deny-by-default (the semantics
/// live in `CompiledRuleset::workspace_write_decision`; this function just
/// asks). The backend turns `read_only` into a `:ro` bind, so the kernel
/// enforces it, not the harness.
pub fn build_sandbox_spec(
    manifest_dir: &Path,
    command: Vec<String>,
    ruleset: CompiledRuleset,
    run_id: &str,
) -> SandboxSpec {
    let workspace_host = crate::manifest::project_root_of(manifest_dir);
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
        security: agentstack_runtime::SandboxSecurity::default(),
    }
}

/// How strongly a run's effective policy is actually *enforced* at runtime, as
/// opposed to merely declared. This is the one honest label we surface wherever
/// a run is described (the start banner, `--plan`, and `agentstack report run`), so
/// nobody mistakes advisory host-mode policy for enforced sandbox policy.
///
/// (This is a plain C-like enum — the Rust analogue of a TypeScript string-union
/// discriminant. `#[derive(Clone, Copy)]` lets it be passed by value like a
/// number instead of moved, which is what you want for a tiny tag type.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Posture {
    /// Host process, no container: the gateway still brokers MCP tool calls, but
    /// nothing confines the process's own egress or filesystem — policy there is
    /// advisory, not enforced.
    Host,
    /// Sandboxed with a host-side egress proxy: proxied HTTPS egress is checked
    /// against compiled policy, but the container sits on an ordinary bridge —
    /// a process that ignores `HTTPS_PROXY` can still dial out directly. Only
    /// `--lockdown` removes that route, so this label must never say ENFORCED.
    Sandbox,
    /// Sandboxed on an internal-only network whose sole peer is the egress
    /// sidecar: enforced *and* topologically confined — no host route, no direct
    /// internet.
    Lockdown,
}

impl Posture {
    /// The stable machine slug persisted beside a run's event log and emitted in
    /// `report --json`. Kept separate from the human [`Display`] label so the
    /// on-disk form never shifts when we reword the banner.
    ///
    /// [`Display`]: std::fmt::Display
    pub fn slug(self) -> &'static str {
        match self {
            Posture::Host => "host",
            Posture::Sandbox => "sandbox",
            Posture::Lockdown => "lockdown",
        }
    }

    /// Parse a slug back into a `Posture`. `None` for anything unrecognized (a
    /// posture file from a future version, or a truncated write) — the reader
    /// then just omits the label rather than guessing.
    pub fn from_slug(s: &str) -> Option<Posture> {
        match s.trim() {
            "host" => Some(Posture::Host),
            "sandbox" => Some(Posture::Sandbox),
            "lockdown" => Some(Posture::Lockdown),
            _ => None,
        }
    }
}

impl std::fmt::Display for Posture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // `posture / enforcement-strength` — the second half is the honest part.
        // ENFORCED is reserved for lockdown: plain --sandbox proxies egress but
        // leaves the direct route open (a proxy-ignoring process can bypass it).
        let label = match self {
            Posture::Host => "HOST / ADVISORY",
            Posture::Sandbox => "SANDBOX / PROXIED · DIRECT ROUTE OPEN",
            Posture::Lockdown => "LOCKDOWN / ENFORCED · NO DIRECT ROUTE",
        };
        f.write_str(label)
    }
}

/// Read the enforcement posture recorded for a completed run, if any. The CLI
/// writes a one-word `posture` file beside the run's `events.jsonl` when a
/// sandbox starts (see [`execute_plan`]); `agentstack report run` reads it back to
/// label the run. `None` when the run predates posture recording, was a host
/// run (host runs aren't recorded at all), or the file is unreadable.
///
/// `run_id` comes from the user, so we guard it to a single safe path segment
/// before joining — a stray `../` must never read outside the runs directory
/// (mirrors the recorder's own `safe_run_segment`, which is private to it).
pub fn read_recorded_posture(run_id: &str) -> Option<Posture> {
    let safe = !run_id.is_empty()
        && run_id.len() <= 128
        && run_id != "."
        && run_id != ".."
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if !safe {
        return None;
    }
    let path = crate::util::paths::agentstack_home()
        .join("runs")
        .join(run_id)
        .join("posture");
    let raw = std::fs::read_to_string(path).ok()?;
    Posture::from_slug(&raw)
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
    /// The project/manifest dir this run executes — needed to rebuild the
    /// gateway (`Gateway::from_frozen`) that brokers the sandbox's MCP traffic.
    pub manifest_dir: PathBuf,
    /// The harness adapter descriptor, cloned at plan time so the gateway
    /// wiring can render a single MCP entry into the harness's own config
    /// format (per-adapter field names + transport tag) without re-consulting
    /// the registry inside the feature-gated executor.
    pub harness_desc: crate::adapter::AdapterDescriptor,
    /// `--profile <name>`, if given: the gateway fences its proxied surface to
    /// that profile's servers, so a sandboxed run scoped to a profile only
    /// exposes that profile's servers (same scoping the host path applies).
    pub profile: Option<String>,
    /// The profile-fenced runtime servers resolved ONCE at plan build (D4):
    /// strictly fenced (a missing profile errors, never broadens) and
    /// library-pin-verified. The SAME frozen definitions feed both the gateway's
    /// dispatch (`Gateway::from_frozen`) and, under lockdown, the gateway-only
    /// host classification — so the two can never diverge.
    pub frozen_servers: Vec<crate::resolve::FrozenServer>,
}

impl ExecutionPlan {
    /// This run's enforcement posture. Both sandbox modes enforce policy; the
    /// no-host-route lockdown is the stronger of the two.
    pub fn posture(&self) -> Posture {
        if self.lockdown {
            Posture::Lockdown
        } else {
            Posture::Sandbox
        }
    }

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
        let mut ruleset = crate::render::ruleset_for(&ctx.loaded.manifest)?;
        // Resolve the profile-fenced runtime servers ONCE (strict fence, library
        // pins verified) and freeze them — the SAME definitions feed both the
        // gateway's dispatch and, under lockdown, D4 classification, so a
        // classification/dispatch mismatch is impossible.
        let library = crate::library::Library::load_default_or_warn();
        let frozen_servers = crate::resolve::frozen_runtime_servers(
            &ctx.loaded.manifest,
            &library,
            &crate::util::paths::lib_home(),
            &ctx.dir,
            args.profile.as_deref(),
        )?;
        if args.lockdown {
            // Under lockdown the container may reach declared HTTP MCP upstreams
            // only through the gateway relay, never by direct egress. Classify
            // the frozen set's HTTP hosts and fence them in the ruleset the
            // sidecar enforces; an unclassifiable or unavailable selected server
            // fails the run here (`?`) rather than leaving a direct route open.
            ruleset.gateway_only_hosts = crate::resolve::gateway_only_hosts(&frozen_servers)?;
        }
        // Rendered here: the plan stores display text for the ro-mount notice.
        let fs_readonly_reason = ruleset
            .workspace_write_decision()
            .err()
            .map(|denial| denial.to_string());
        let run_id = crate::runs::gen_id();
        let spec = build_sandbox_spec(&ctx.dir, command, ruleset, &run_id);

        Ok(ExecutionPlan {
            run_id,
            server: args.harness.clone(),
            spec,
            lockdown: args.lockdown,
            trust,
            fs_readonly_reason,
            manifest_dir: ctx.dir.clone(),
            harness_desc: desc.clone(),
            profile: args.profile.clone(),
            frozen_servers,
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
        // Enforcement posture: the honest label for how strongly this run's
        // policy is enforced (both sandbox modes ENFORCE; lockdown also has no
        // host route). Shown in cyan like the mode lines below.
        let _ = writeln!(s, "  posture: {}", self.posture().to_string().cyan().bold());
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
                 peer is the egress sidecar. Review it with `agentstack report run {}`.",
                "🔒".cyan(),
                self.run_id
            );
        } else {
            let _ = writeln!(
                s,
                "  {} egress is routed through the AgentStack proxy; \
                 review it after with `agentstack report run {}`.",
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

/// P24: verify the sandbox prerequisite BEFORE any posture banner is painted, so
/// a run that cannot happen is never dressed up as one that can (principle 6 —
/// never paint a posture you can't deliver). Off the `sandbox` feature this is
/// the rebuild refusal; on it, it confirms the Docker daemon actually answers
/// (the same `connect()` ping the executors rely on), so a daemon-down run
/// refuses with the daemon hint here instead of after "▶ sandboxing …".
///
/// The feature-on preflight connects and immediately drops the handle; the
/// executor reconnects when it runs. That's a second cheap ping, not shared
/// state — the alternative (threading one backend through `execute_plan` →
/// `execute_proxy`/`execute_lockdown`, which connect only after wiring the
/// gateway) would move the Docker dependency deep into three signatures just to
/// save a round-trip. `--plan` never calls this — it describes, it never
/// launches, so it must keep working with the feature off or Docker down.
#[cfg(feature = "sandbox")]
fn preflight_prerequisites() -> Result<()> {
    agentstack_runtime::docker::DockerSandbox::connect()
        .map(|_backend| ())
        .map_err(|e| {
            anyhow::anyhow!(
                "cannot reach Docker ({e}) — nothing was launched\n\n  \
             start Docker Desktop (or your Docker daemon), verify with: docker info\n  \
             then re-run the same command"
            )
        })
}

#[cfg(not(feature = "sandbox"))]
fn preflight_prerequisites() -> Result<()> {
    anyhow::bail!(
        "sandbox support is not compiled into this build — rebuild with \
         `cargo build --features sandbox` (it also needs a running Docker daemon)."
    )
}

/// Entry point for `agentstack run --sandbox`.
pub fn run_sandboxed(dir: Option<&Path>, args: &RunArgs) -> Result<()> {
    let ctx = crate::commands::load(dir)?;
    let plan = ExecutionPlan::build(&ctx, args)?;
    // The banner shows the host dir actually mounted (the project root the
    // spec resolved), read back off the spec so the two can never diverge.
    let workspace_host = PathBuf::from(plan.spec.workspace());

    // `--plan`: show exactly what would run (trust, policy, mode, command) and
    // stop — no Docker, no feature needed. "Display the plan" instead of run it.
    // Deliberately BEFORE the prerequisite preflight below: a dry-run describes,
    // it never launches, so it must succeed with the feature off / Docker down.
    if args.plan {
        println!("{}", plan.display(&workspace_host, true));
        return Ok(());
    }

    // P24: the prerequisite check happens HERE, before the banner — never show a
    // container start that can't happen. Its message states the prerequisite
    // plainly (rebuild the binary, or start the daemon).
    preflight_prerequisites()?;

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
    run_id: &str,
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
    let result = match exit.code {
        Some(0) => {
            println!("\n{} sandbox exited cleanly.", "✓".green());
            Ok(())
        }
        Some(c) => Err(anyhow::anyhow!("sandbox exited with code {c}")),
        None => Err(anyhow::anyhow!("sandbox was killed by a signal")),
    };
    println!("See what happened: `agentstack report run {run_id}`");
    result
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

/// The container's home directory — where the harness reads its GLOBAL
/// (user-scope) MCP config. The gateway entry is rendered there rather than at
/// project scope, because project-scope MCP entries sit behind an interactive
/// "approve this server" prompt no one can answer inside a container (spike
/// finding, `docs/spikes/2026-07-11-gateway-http-transport.md`). Defaults to
/// `/root` (the shipped `docker/sandbox.Dockerfile` runs as root); override
/// with `AGENTSTACK_SANDBOX_HOME` for an image whose harness user differs.
#[cfg(feature = "sandbox")]
fn container_home() -> String {
    std::env::var("AGENTSTACK_SANDBOX_HOME").unwrap_or_else(|_| "/root".to_string())
}

/// The rendered configs and their in-container destinations for routing a
/// harness through the gateway. Pure over the descriptor + URL + token (no I/O,
/// no live endpoint), so the render is unit-testable.
#[cfg(feature = "sandbox")]
struct GatewayConfig {
    /// The harness's global (user-scope) config, containing exactly the one
    /// gateway entry, and where it mounts in the container (e.g.
    /// `/root/.claude.json`).
    global_body: String,
    global_container: String,
    /// An EMPTY project-scope config, and where to mount it to SHADOW any
    /// direct entries a prior `agentstack apply` left in the workspace (with
    /// baked secrets) — else the harness reaches those upstreams around the
    /// gateway. `None` when the harness has no project-scope config.
    empty_body: String,
    project_container: Option<String>,
}

/// A harness global config path (as declared on the descriptor, e.g.
/// `~/.claude.json`, `~/.codex/config.toml`) mapped to a path RELATIVE to the
/// container home, intermediate dirs preserved. Returns `None` for a
/// `{config}`/`{data}` placeholder form (VS Code, Claude Desktop) — those have
/// no reliable container-home mapping, so a run must decline to route rather
/// than mount at a bogus path and claim enforcement.
#[cfg(feature = "sandbox")]
fn container_rel_path(config_path: &str) -> Option<String> {
    if config_path.contains('{') {
        return None;
    }
    Some(
        config_path
            .strip_prefix("~/")
            .or_else(|| config_path.strip_prefix('~'))
            .unwrap_or_else(|| config_path.trim_start_matches('/'))
            .to_string(),
    )
}

/// Render the gateway entry into the harness's own config format. Returns
/// `None` when the harness has no MCP config, can't represent an HTTP entry
/// (e.g. a stdio-only adapter), or has a config path with no container mapping.
#[cfg(feature = "sandbox")]
fn render_gateway_config(
    desc: &crate::adapter::AdapterDescriptor,
    home: &str,
    url: &str,
    token: &str,
) -> Result<Option<GatewayConfig>> {
    use crate::adapter::descriptor::Format;
    use crate::adapter::render_server;
    use crate::manifest::{Server, ServerType};
    use crate::render::{merge_json, merge_toml};
    use crate::secret::MapResolver;

    let (Some(config), Some(mcp)) = (desc.config.as_ref(), desc.mcp.as_ref()) else {
        return Ok(None);
    };

    // ONE synthetic HTTP entry, rendered through the harness's own adapter so
    // it lands in that CLI's native field names + transport tag. The token is a
    // literal header value (not a `${REF}`), so an empty resolver renders it
    // verbatim.
    let gw_server = Server {
        server_type: ServerType::Http,
        url: Some(url.to_string()),
        command: None,
        args: Vec::new(),
        cwd: None,
        targets: crate::manifest::model::all_targets(),
        owner: None,
        // Field type (`IndexMap<String, String>`) drives the collect — no need
        // to name the import.
        headers: [("X-Agentstack-Token".to_string(), token.to_string())]
            .into_iter()
            .collect(),
        env: Default::default(),
        // A synthetic in-memory entry: nothing on disk to integrity-pin.
        integrity_roots: Vec::new(),
        extra: Default::default(),
    };
    let rendered = render_server(desc, &gw_server, &MapResolver::default());
    if !rendered.representable {
        return Ok(None);
    }

    // The global config path RELATIVE to home, intermediate dirs preserved:
    // `~/.claude.json` → `.claude.json`, `~/.codex/config.toml` →
    // `.codex/config.toml` (Docker creates the parent dir in the writable
    // container home). Flattening to the basename would mount codex's config at
    // `/root/config.toml`, a path it never reads.
    let Some(rel) = container_rel_path(&config.path) else {
        // A `{config}`/`{data}` placeholder path (e.g. VS Code's
        // `{config}/Code/User/mcp.json`) has no reliable mapping into the
        // container home — routing it would mount at a bogus path while
        // claiming enforcement. Decline to route rather than mislead.
        return Ok(None);
    };

    let entries = vec![("agentstack-gateway".to_string(), rendered.value)];
    let render = |es: &[(String, serde_json::Value)], fmt: Format| -> Result<String> {
        Ok(match fmt {
            Format::Json => merge_json::merge("", &mcp.location, es)?,
            Format::Toml => merge_toml::merge_with_removals(
                "",
                &mcp.location,
                es,
                &[],
                mcp.headers_as_subtable,
            )?,
        })
    };
    let global_body = render(&entries, config.format)?;
    // The shadow rides at PROJECT scope, so render the empty map in the
    // project's own format when it differs from the global one (a user adapter
    // may — `config_for` honors `project.format.unwrap_or(config.format)`).
    let project_fmt = desc
        .project
        .as_ref()
        .and_then(|p| p.format)
        .unwrap_or(config.format);
    let empty_body = render(&[], project_fmt)?;
    let project_container = desc
        .project
        .as_ref()
        .map(|p| format!("{WORKSPACE}/{}", p.config));

    Ok(Some(GatewayConfig {
        global_body,
        global_container: format!("{home}/{rel}"),
        empty_body,
        project_container,
    }))
}

/// Make a bind-mounted config file readable by the container's user, whatever
/// uid that is. These files live in a 0700 staging dir, which already gates
/// other HOST users from reaching them by path; the file's OWN mode only decides
/// whether the CONTAINER process — which bind-mounts the file directly, not via
/// the dir tree — can read it. A 0600 file owned by the host user is UNREADABLE
/// by a non-root container user on Linux (Docker Desktop's file sharing masks
/// this, so it only bites on a native daemon): the agent's token read comes back
/// empty and it can't authenticate to the gateway. 0644 fixes that and exposes
/// nothing, because the 0700 parent still blocks host users. No-op off unix.
#[cfg(feature = "sandbox")]
fn mount_readable(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Render EMPTY harness configs (no gateway entry) for the shadow-only path — a
/// lockdown run with nothing to route. `None` when the harness config can't be
/// mapped into the container home, so the caller refuses the lockdown run.
#[cfg(feature = "sandbox")]
fn render_shadow_config(
    desc: &crate::adapter::AdapterDescriptor,
    home: &str,
) -> Result<Option<GatewayConfig>> {
    use crate::adapter::descriptor::Format;
    use crate::render::{merge_json, merge_toml};

    let (Some(config), Some(mcp)) = (desc.config.as_ref(), desc.mcp.as_ref()) else {
        return Ok(None);
    };
    let Some(rel) = container_rel_path(&config.path) else {
        return Ok(None);
    };
    let render = |fmt: Format| -> Result<String> {
        Ok(match fmt {
            Format::Json => merge_json::merge("", &mcp.location, &[])?,
            Format::Toml => merge_toml::merge_with_removals(
                "",
                &mcp.location,
                &[],
                &[],
                mcp.headers_as_subtable,
            )?,
        })
    };
    let project_fmt = desc
        .project
        .as_ref()
        .and_then(|p| p.format)
        .unwrap_or(config.format);
    Ok(Some(GatewayConfig {
        global_body: render(config.format)?,
        global_container: format!("{home}/{rel}"),
        empty_body: render(project_fmt)?,
        project_container: desc
            .project
            .as_ref()
            .map(|p| format!("{WORKSPACE}/{}", p.config)),
    }))
}

/// Shadow the harness's native MCP config with EMPTY configs for a lockdown run
/// that has nothing to route (empty gateway: untrusted, or no proxied servers).
/// The container then finds zero MCP servers, so a stale native entry can't
/// reach an upstream around the absent gateway. Refuses (Err) when the harness
/// config can't be mapped into the container — there is no safe un-shadowed
/// fallback under lockdown.
#[cfg(feature = "sandbox")]
fn shadow_native_config(
    desc: &crate::adapter::AdapterDescriptor,
    manifest_dir: &Path,
    run_id: &str,
    spec: &mut SandboxSpec,
) -> Result<SandboxGateway> {
    let Some(cfg) = render_shadow_config(desc, &container_home())? else {
        anyhow::bail!(
            "lockdown: {} has no mappable MCP config to shadow, so a stale native \
             entry could reach an upstream directly — refusing to start",
            desc.display
        );
    };
    let tempdir = std::env::temp_dir().join(format!("agentstack-gw-{run_id}"));
    std::fs::create_dir_all(&tempdir).context("creating the shadow config staging dir")?;
    crate::util::restrict(&tempdir, true);
    // Own the tempdir in the RAII handle from HERE (no endpoint, no relay), so a
    // later `?` still cleans it up.
    let gw = SandboxGateway {
        _endpoint: None,
        tempdir,
        relay_dest: None,
    };

    // Empty global config shadows any user-scope native entry.
    let global_host = gw.tempdir.join("global-config");
    std::fs::write(&global_host, &cfg.global_body).context("writing the empty global shadow")?;
    mount_readable(&global_host);
    spec.mounts.push(Mount {
        host: global_host.display().to_string(),
        container: cfg.global_container,
        read_only: true,
    });

    // Empty project config shadows a workspace-scope native entry, but only when
    // one exists (mounting over a nonexistent read-only path fails the run).
    if let (Some(project_container), Some(proj)) = (cfg.project_container, desc.project.as_ref()) {
        // COUPLING NOTE: this existence check must be rooted at the same host
        // dir `build_sandbox_spec` mounts at `/workspace` — the PROJECT root
        // (`project_root_of`), not the manifest dir. If the two ever diverge, a
        // root-scope native MCP config would go un-shadowed under lockdown (a
        // direct-route shadow bypass). Keep BOTH shadow checks identical.
        if crate::manifest::project_root_of(manifest_dir)
            .join(&proj.config)
            .exists()
        {
            let empty_host = gw.tempdir.join("project-shadow");
            std::fs::write(&empty_host, &cfg.empty_body)
                .context("writing the project shadow config")?;
            mount_readable(&empty_host);
            spec.mounts.push(Mount {
                host: empty_host.display().to_string(),
                container: project_container,
                read_only: true,
            });
        }
    }

    println!(
        "  {} no servers to route; native MCP config scrubbed (zero MCP under lockdown)",
        "✓".green()
    );
    Ok(gw)
}

/// The live gateway wiring for one sandboxed run: the HTTP endpoint the
/// container talks to, plus the temp dir holding the rendered config mounted
/// into it. Held for the run's lifetime; its `Drop` removes the temp dir (RAII,
/// so the token-bearing config is cleaned up even on an early error return).
#[cfg(feature = "sandbox")]
struct SandboxGateway {
    /// The token-gated HTTP MCP endpoint. Its serve thread owns the
    /// `Arc<Gateway>`, so upstream connections (and lazily spawned stdio
    /// children) live until this process exits — the run's lifetime.
    /// `None` for a shadow-only run (empty gateway under lockdown): native MCP
    /// config is scrubbed but there is nothing to route, so no endpoint exists.
    _endpoint: Option<crate::gateway_http::GatewayHttp>,
    /// Run-scoped dir holding the rendered config files bind-mounted into the
    /// container; removed on drop.
    tempdir: PathBuf,
    /// In lockdown, the host gateway address (`host.docker.internal:<port>`)
    /// the sidecar relay must splice to; `None` in plain `--sandbox` (the
    /// container reaches the host gateway directly).
    relay_dest: Option<String>,
}

#[cfg(feature = "sandbox")]
impl Drop for SandboxGateway {
    fn drop(&mut self) {
        // The staged config carries the live per-run bearer token; remove it
        // once the run has released its mounts. Best-effort — a leaked temp dir
        // is visible, not silent, and never blocks the run's own result.
        let _ = std::fs::remove_dir_all(&self.tempdir);
    }
}

/// Route the sandbox's MCP traffic through the in-process gateway so
/// `[policy.tools]` is enforced and every tool call lands in this run's
/// `events.jsonl` — the gateway-unification milestone.
///
/// Trade-off (accepted at plan time): the gateway resolves `${REF}` secrets
/// and, for stdio servers, spawns children on the HOST. So it is HARD
/// trust-gated (`Gateway::from_frozen` — an untrusted bundle yields an empty
/// gateway and this returns `None`, leaving the run exactly as it was before
/// this milestone), and its endpoint is token-gated per request. Resolved
/// secrets never enter the container — it sees only the endpoint URL + token.
///
/// Returns `None` (leaving `spec` untouched) when there is nothing to route: an
/// untrusted bundle, one with no proxied servers, or a harness that can't host
/// an HTTP MCP entry — each surfaced on stderr. Both `--sandbox` (container
/// reaches the host gateway directly) and `--lockdown` (a sidecar relay bridges
/// the internal-only network to the host gateway) are routed.
#[cfg(feature = "sandbox")]
#[allow(clippy::too_many_arguments)]
fn wire_sandbox_gateway(
    manifest_dir: &Path,
    desc: &crate::adapter::AdapterDescriptor,
    run_id: &str,
    lockdown: bool,
    frozen: Vec<crate::resolve::FrozenServer>,
    spec: &mut SandboxSpec,
) -> Result<Option<SandboxGateway>> {
    use std::sync::Arc;

    // Build the run's gateway from the SAME compiled ruleset the plan carries
    // (never recompiled) and the SAME frozen server set the plan classified
    // (never re-resolved), then hard trust-gate it: untrusted → empty → no
    // routing, no secret resolution, no host children.
    let gateway = crate::gateway::Gateway::from_frozen(
        Some(manifest_dir),
        spec.ruleset.clone(),
        frozen,
        run_id,
    );
    if gateway.is_empty() {
        // An empty gateway means either an untrusted bundle (from_frozen already
        // printed "gateway: refusing to serve …") or a TRUSTED bundle with no
        // proxied servers.
        if lockdown {
            // D4 one-route: even with nothing to route, a lockdown container
            // must not read stale native MCP config (a prior `agentstack apply`
            // may have left entries with baked secrets) and reach an upstream
            // around the absent gateway. Scrub it by shadowing the harness's
            // native config with empty ones; if the config can't be mapped
            // there is no way to scrub it, so refuse rather than run un-shadowed.
            return Ok(Some(shadow_native_config(
                desc,
                manifest_dir,
                run_id,
                spec,
            )?));
        }
        let root = crate::manifest::project_root_of(manifest_dir);
        if crate::trust::check(&root) == agentstack_trust::TrustState::Trusted {
            eprintln!(
                "  {} no proxied servers to route through the gateway; \
                 running without tool-policy routing",
                "note:".dimmed()
            );
        }
        return Ok(None);
    }

    // Cheap pre-check: a harness that can't host an HTTP MCP entry (no config,
    // a stdio-only adapter with no `url` field, or a config path with no
    // container-home mapping) can't be routed — bail BEFORE binding an endpoint
    // we'd otherwise leak for the run.
    let can_host_http = desc
        .config
        .as_ref()
        .is_some_and(|c| container_rel_path(&c.path).is_some())
        && desc.mcp.as_ref().is_some_and(|m| m.fields.url.is_some());
    if !can_host_http {
        if lockdown {
            // D4 one-route: under lockdown the gateway relay must be the ONLY
            // MCP route, which means installing a gateway entry into the
            // harness's config and shadowing any native one. An adapter that
            // can't host an HTTP MCP entry can be neither routed nor reliably
            // shadowed, so there is no safe direct-route fallback — refuse to
            // start rather than run the container with an un-shadowed config.
            anyhow::bail!(
                "lockdown: {} cannot host an HTTP MCP entry, so the gateway can't be \
                 made the only MCP route and its native config can't be shadowed — \
                 refusing to start (no direct-route fallback exists under lockdown)",
                desc.display
            );
        }
        eprintln!(
            "  {} {} can't host an HTTP MCP entry; running without tool-policy routing",
            "note:".dimmed(),
            desc.display
        );
        return Ok(None);
    }

    // Preflight: the gateway endpoint is token-gated, and the per-run bearer
    // token rides in the `X-Agentstack-Token` HTTP header of the injected
    // config entry. `render_server` only emits that header when the adapter
    // declares a `mcp.fields.headers` mapping (see `agentstack_adapters::render`)
    // — without it the token is silently dropped and EVERY routed tool call is
    // rejected 401. Catch that here, before we bind an endpoint and launch a
    // container, instead of surfacing it as a confusing runtime auth failure.
    // Every shipped HTTP-capable adapter declares this field; the guard protects
    // a future one that forgets it.
    let can_carry_token = desc
        .mcp
        .as_ref()
        .is_some_and(|m| m.fields.headers.is_some());
    if !can_carry_token {
        if lockdown {
            // D4 one-route: the relay is the only MCP route, so a config that
            // can't carry the auth token isn't "degraded routing" — it's a
            // container that can reach nothing. Fail closed with a fix.
            anyhow::bail!(
                "lockdown: {} has no `mcp.fields.headers` mapping, so the gateway's \
                 per-run auth token can't be injected into its config — every routed \
                 tool call would be rejected (401). Add a `headers` field to the \
                 adapter's `mcp.fields`. Refusing to start (the gateway relay is the \
                 only MCP route under lockdown).",
                desc.display
            );
        }
        eprintln!(
            "  {} {} has no MCP headers field to carry the gateway auth token; \
             running without tool-policy routing",
            "note:".dimmed(),
            desc.display
        );
        return Ok(None);
    }

    // Start the endpoint so the rendered config can carry its real port +
    // token. Bind broadly (0.0.0.0) so the container reaches it — the token is
    // the gate, per the endpoint's own contract, like the egress proxy's bind.
    let endpoint = crate::gateway_http::start(Arc::new(gateway), "0.0.0.0:0")
        .context("starting the gateway HTTP endpoint")?;

    // How the container reaches the host gateway differs by mode:
    // - `--sandbox`: direct, via host.docker.internal (the container has a
    //   route to the host).
    // - `--lockdown`: NO host route — it reaches the sidecar's relay alias,
    //   which splices to the host gateway. `relay_dest` is what the sidecar
    //   dials on its egress leg.
    // NO_PROXY carves the gateway host out of the container's proxy env so its
    // plain-HTTP request goes DIRECT, not through the CONNECT-only egress proxy.
    let (gateway_host, relay_dest) = if lockdown {
        (
            format!(
                "{}:{}",
                agentstack_runtime::PROXY_ALIAS,
                agentstack_runtime::GATEWAY_RELAY_PORT
            ),
            Some(format!("host.docker.internal:{}", endpoint.port)),
        )
    } else {
        (format!("host.docker.internal:{}", endpoint.port), None)
    };
    let url = format!("http://{gateway_host}/mcp");
    let no_proxy_host = gateway_host
        .split(':')
        .next()
        .unwrap_or(&gateway_host)
        .to_string();

    let Some(cfg) = render_gateway_config(desc, &container_home(), &url, &endpoint.token)? else {
        if lockdown {
            // Same one-route contract as the pre-check above: if the gateway
            // config can't actually be rendered for this harness, we can't make
            // the relay the sole route or shadow native config — fail closed.
            anyhow::bail!(
                "lockdown: could not render a gateway config for {} — refusing to \
                 start rather than run with an un-shadowed native MCP config",
                desc.display
            );
        }
        eprintln!(
            "  {} {} can't host an HTTP MCP entry; running without tool-policy routing",
            "note:".dimmed(),
            desc.display
        );
        return Ok(None);
    };

    // Stage the rendered configs in a run-scoped 0700 dir, bind-mounted
    // read-only. The dir is owner-only (host-user protection for the live
    // endpoint token in the global config); the files inside are 0644 via
    // `mount_readable` so a non-root container user can still read them — see
    // that helper for why the file mode, not the dir, is what the container sees.
    let tempdir = std::env::temp_dir().join(format!("agentstack-gw-{run_id}"));
    std::fs::create_dir_all(&tempdir).context("creating the gateway config staging dir")?;
    crate::util::restrict(&tempdir, true);
    // Own the tempdir in the RAII handle from HERE, so any `?` below (a failed
    // config write) drops it and removes the token-bearing dir — no leak on the
    // error path.
    let gw = SandboxGateway {
        _endpoint: Some(endpoint),
        tempdir,
        relay_dest,
    };

    let global_host = gw.tempdir.join("global-config");
    std::fs::write(&global_host, &cfg.global_body).context("writing the gateway config")?;
    mount_readable(&global_host);
    spec.mounts.push(Mount {
        host: global_host.display().to_string(),
        container: cfg.global_container,
        read_only: true,
    });

    // Shadow the project-scope config — but ONLY when one actually exists in
    // the workspace. If there's no such file, there's nothing stale to hide,
    // and mounting a file over a nonexistent path inside a read-only workspace
    // fails the run (Docker can't create the mountpoint). When it does exist,
    // the mountpoint exists too, so the read-only overlay works.
    if let (Some(project_container), Some(proj)) = (cfg.project_container, desc.project.as_ref()) {
        // COUPLING NOTE: this existence check must be rooted at the same host
        // dir `build_sandbox_spec` mounts at `/workspace` — the PROJECT root
        // (`project_root_of`), not the manifest dir. If the two ever diverge, a
        // root-scope native MCP config would go un-shadowed under lockdown (a
        // direct-route shadow bypass). Keep BOTH shadow checks identical.
        if crate::manifest::project_root_of(manifest_dir)
            .join(&proj.config)
            .exists()
        {
            let empty_host = gw.tempdir.join("project-shadow");
            std::fs::write(&empty_host, &cfg.empty_body)
                .context("writing the project shadow config")?;
            mount_readable(&empty_host);
            spec.mounts.push(Mount {
                host: empty_host.display().to_string(),
                container: project_container,
                read_only: true,
            });
        }
    }

    // Carve the gateway host out of the container's proxy env (see above): its
    // plain-HTTP request must go DIRECT, not through the CONNECT-only egress
    // proxy (which would 400 it). Verified necessary in the spike. Real
    // upstream egress (other hosts) still goes through the proxy.
    for key in ["NO_PROXY", "no_proxy"] {
        spec.env.push((key.to_string(), no_proxy_host.clone()));
    }

    println!(
        "  {} MCP tool calls routed through the gateway (tool policy enforced, calls recorded)",
        "✓".green()
    );
    Ok(Some(gw))
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
    // Record the run's enforcement posture beside its event log so
    // `agentstack report run` can label it honestly later. Best-effort (same
    // contract as the recorder itself): a posture-write hiccup must never fail
    // the run it describes. Written to the exact dir `RunLog::create` prepared,
    // so it lands in the run-private 0700 directory.
    if let Some(l) = (*log).as_ref() {
        let _ = std::fs::write(l.path().with_file_name("posture"), plan.posture().slug());
    }
    // A per-run token authenticates the sandbox to its proxy: the listener must
    // bind a broad address so the container can reach it, so the token — not the
    // bind — is what stops anything else that can route to it from using it.
    let token = mint_proxy_token();

    // Route MCP traffic through the gateway (secrets resolved host-side, tool
    // policy enforced, calls recorded). Held alive across the run; its temp dir
    // is removed afterward. A distinct credential from the egress `token` above
    // — one tunnels bytes, the other executes tools with resolved secrets.
    let mut spec = plan.spec;
    let gateway = wire_sandbox_gateway(
        &plan.manifest_dir,
        &plan.harness_desc,
        &plan.run_id,
        plan.lockdown,
        plan.frozen_servers,
        &mut spec,
    )?;

    // In lockdown, the sidecar relay splices to this host gateway address.
    let relay_dest = gateway.as_ref().and_then(|g| g.relay_dest.clone());
    let result = if plan.lockdown {
        execute_lockdown(spec, &plan.run_id, &plan.server, log, token, relay_dest)
    } else {
        execute_proxy(spec, &plan.run_id, &plan.server, log, token)
    };

    // `gateway` (if any) drops here, after the run released its mounts — its
    // Drop removes the token-bearing staging dir.
    drop(gateway);
    result
}

/// Host-process-proxy mode (`--sandbox`): the container gets an ordinary bridge
/// network and its `HTTPS_PROXY` points at a proxy running on the host. This
/// gates the agent's *configured* egress; `--lockdown` is the stronger mode.
/// The run log and per-run token are supplied by [`execute_plan`].
#[cfg(feature = "sandbox")]
fn execute_proxy(
    mut spec: SandboxSpec,
    run_id: &str,
    server: &str,
    log: std::sync::Arc<Option<agentstack_recorder::RunLog>>,
    proxy_token: String,
) -> Result<()> {
    use std::net::{IpAddr, Ipv4Addr};
    use std::sync::Arc;

    // Stand up the egress proxy for this run from the compiled policy, bound on
    // 0.0.0.0 so the container reaches it via host.docker.internal. Attributed
    // to the harness as the sandbox's single egress identity.
    // The sink runs on the proxy's tokio workers (BlockingBridge has exactly
    // one), so the file append happens on a spool thread — a slow disk must
    // not stall every in-flight tunnel. Declared before `bridge` so it drops
    // (and flushes) after the proxies release their sender handles.
    let sink_log = Arc::clone(&log);
    let spool = agentstack_egress::WriterSpool::spawn(
        "egress-events",
        move |ev: agentstack_recorder::RunEvent| {
            if let Some(l) = sink_log.as_ref() {
                l.append(&ev);
            }
        },
    )
    .context("starting the egress event writer")?;
    let events = spool.sender();
    let sink: agentstack_egress::EventSink = Arc::new(move |ev| events.send(ev));
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
        // Host-proxy mode is `--sandbox`, the weaker posture with a direct route
        // still open; D4's literal-IP / non-TLS restrictions are lockdown-only.
        lockdown: false,
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

    let backend = agentstack_runtime::docker::DockerSandbox::connect().map_err(|e| {
        anyhow::anyhow!(
            "cannot reach Docker ({e}) — nothing was launched\n\n  \
             start Docker Desktop (or your Docker daemon), verify with: docker info\n  \
             then re-run the same command"
        )
    })?;
    let result = run_container_to_completion(&spec, run_id, &log, &backend);
    drop(bridge); // stop the proxy now the run is done
    result
}

/// Lockdown mode (`--lockdown`): the container is attached ONLY to an internal
/// network with no host route and no internet; its sole reachable peer is the
/// egress-proxy sidecar the runtime stands up on that network. Ignoring the
/// proxy env then reaches nothing — the confinement is topological, not
/// convention.
#[cfg(feature = "sandbox")]
#[allow(clippy::too_many_arguments)]
fn execute_lockdown(
    mut spec: SandboxSpec,
    run_id: &str,
    server: &str,
    log: std::sync::Arc<Option<agentstack_recorder::RunLog>>,
    proxy_token: String,
    gateway_relay_dest: Option<String>,
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
    // The sink runs on the log-follower's tokio task (a 2-worker runtime), so
    // parse + append happen on a spool thread, not an async worker. Declared
    // before `lock` so it drops (and flushes) after the follower's sender.
    let sink_log = Arc::clone(&log);
    let spool = agentstack_egress::WriterSpool::spawn("lockdown-events", move |line: String| {
        if let (Some(l), Ok(ev)) = (
            sink_log.as_ref(),
            serde_json::from_str::<agentstack_recorder::RunEvent>(&line),
        ) {
            l.append(&ev);
        }
    })
    .context("starting the lockdown event writer")?;
    let events = spool.sender();
    let sink: agentstack_runtime::LockdownSink = Arc::new(move |line: &str| {
        events.send(line.to_string());
    });

    let backend = agentstack_runtime::docker::DockerSandbox::connect().map_err(|e| {
        anyhow::anyhow!(
            "cannot reach Docker ({e}) — nothing was launched\n\n  \
             start Docker Desktop (or your Docker daemon), verify with: docker info\n  \
             then re-run the same command"
        )
    })?;

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
        gateway_relay_dest.as_deref(),
        sink,
    )
    .context("standing up the egress lockdown (is the sidecar image built?)")?;

    // Attach the sandbox to the internal network and point it at the sidecar
    // (proxy_endpoint carries the token in its userinfo).
    spec.network = NetworkPolicy::Lockdown {
        network: lock.internal_network().to_string(),
    };
    set_proxy_env(&mut spec, &lock.proxy_endpoint());

    let result = run_container_to_completion(&spec, run_id, &log, &backend);
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
                deny: vec![],
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
            manifest_dir: PathBuf::from("/proj"),
            harness_desc: Default::default(),
            profile: None,
            frozen_servers: Vec::new(),
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
            manifest_dir: PathBuf::from("/proj"),
            harness_desc: Default::default(),
            profile: None,
            frozen_servers: Vec::new(),
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

    /// The `/workspace` mount is the PROJECT root, not the manifest dir: under
    /// the nested `.agentstack/` layout the mount is the manifest dir's parent.
    /// Mounting `.agentstack/` itself would confine the sandboxed agent to the
    /// manifest folder and hide the project's code (the 2026-07-18 bug).
    #[test]
    fn nested_layout_mounts_the_project_root_not_the_manifest_dir() {
        let spec = build_sandbox_spec(
            Path::new("/home/me/proj/.agentstack"),
            vec!["claude".into()],
            CompiledRuleset::default(),
            "r-nested",
        );
        assert_eq!(spec.workspace(), "/home/me/proj");
        assert_eq!(spec.workdir, WORKSPACE);
    }

    /// A legacy root `agentstack.toml` keeps mounting the project dir itself.
    #[test]
    fn legacy_root_manifest_mounts_the_project_dir_unchanged() {
        let spec = build_sandbox_spec(
            Path::new("/home/me/proj"),
            vec!["claude".into()],
            CompiledRuleset::default(),
            "r-root",
        );
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

    /// Posture selection: lockdown → Lockdown, otherwise Sandbox. Host posture
    /// isn't reachable from an ExecutionPlan (a plan only ever describes a
    /// sandbox run); it's produced directly on the host-mode banner path.
    #[test]
    fn plan_posture_tracks_the_mode() {
        let plan = |lockdown| ExecutionPlan {
            run_id: "r".into(),
            server: "claude-code".into(),
            spec: build_sandbox_spec(
                Path::new("/proj"),
                vec!["claude".into()],
                CompiledRuleset::default(),
                "r",
            ),
            lockdown,
            trust: agentstack_trust::TrustState::Trusted,
            fs_readonly_reason: None,
            manifest_dir: PathBuf::from("/proj"),
            harness_desc: Default::default(),
            profile: None,
            frozen_servers: Vec::new(),
        };
        assert_eq!(plan(true).posture(), Posture::Lockdown);
        assert_eq!(plan(false).posture(), Posture::Sandbox);
    }

    /// Every posture's slug round-trips, and the human label states the
    /// enforcement strength honestly. ENFORCED is reserved for lockdown:
    /// plain --sandbox leaves the direct route open, and its label must say so
    /// rather than claim enforcement a proxy-ignoring process could bypass.
    #[test]
    fn posture_slug_roundtrips_and_labels_enforcement() {
        for p in [Posture::Host, Posture::Sandbox, Posture::Lockdown] {
            assert_eq!(Posture::from_slug(p.slug()), Some(p));
        }
        assert!(Posture::Host.to_string().contains("ADVISORY"));
        assert!(Posture::Sandbox.to_string().contains("DIRECT ROUTE OPEN"));
        assert!(!Posture::Sandbox.to_string().contains("ENFORCED"));
        assert!(Posture::Lockdown.to_string().contains("NO DIRECT ROUTE"));
        // Unknown / truncated slugs read back as None (the reader then omits the
        // label rather than guessing).
        assert_eq!(Posture::from_slug("host\n"), Some(Posture::Host)); // trims
        assert_eq!(Posture::from_slug("mystery"), None);
        assert_eq!(Posture::from_slug(""), None);
    }

    /// The `--plan`/banner display names the posture label, not just the mode.
    #[test]
    fn plan_display_names_the_posture() {
        let plan = ExecutionPlan {
            run_id: "r".into(),
            server: "codex".into(),
            spec: build_sandbox_spec(
                Path::new("/proj"),
                vec!["x".into()],
                CompiledRuleset::default(),
                "r",
            ),
            lockdown: false,
            trust: agentstack_trust::TrustState::Trusted,
            fs_readonly_reason: None,
            manifest_dir: PathBuf::from("/proj"),
            harness_desc: Default::default(),
            profile: None,
            frozen_servers: Vec::new(),
        };
        let out = plan.display(Path::new("/proj"), true);
        assert!(out.contains("posture:"), "{out}");
        assert!(
            out.contains("SANDBOX / PROXIED · DIRECT ROUTE OPEN"),
            "{out}"
        );
    }

    /// The gateway entry renders into the harness's real config format: a
    /// single HTTP server carrying the endpoint URL + token header at the
    /// container home path, plus an empty project-scope shadow that neutralizes
    /// any direct entries a prior `apply` left in the workspace.
    #[cfg(feature = "sandbox")]
    #[test]
    fn gateway_config_renders_into_the_harness_format() {
        let reg = crate::adapter::Registry::load().unwrap();
        let desc = reg.get("claude-code").expect("claude-code descriptor");
        let cfg = render_gateway_config(
            desc,
            "/root",
            "http://host.docker.internal:12345/mcp",
            "tok-abc",
        )
        .unwrap()
        .expect("claude-code hosts HTTP MCP entries");

        // Global config: mounts at the container home, carries the one gateway
        // entry with its URL and token header.
        assert_eq!(cfg.global_container, "/root/.claude.json");
        let v: serde_json::Value = serde_json::from_str(&cfg.global_body).unwrap();
        let entry = &v["mcpServers"]["agentstack-gateway"];
        assert_eq!(entry["url"], "http://host.docker.internal:12345/mcp");
        assert_eq!(entry["headers"]["X-Agentstack-Token"], "tok-abc");
        // The token appears ONLY in the mounted config, never leaks elsewhere.
        assert!(cfg.global_body.contains("tok-abc"));

        // Project shadow: an empty server map at the workspace project path, so
        // stale direct entries in the repo can't route around the gateway.
        assert_eq!(
            cfg.project_container.as_deref(),
            Some("/workspace/.mcp.json")
        );
        let empty: serde_json::Value = serde_json::from_str(&cfg.empty_body).unwrap();
        assert_eq!(empty["mcpServers"], serde_json::json!({}));
        assert!(!cfg.empty_body.contains("tok-abc"));
    }

    /// A NESTED global config path (`~/.codex/config.toml`) must keep its
    /// intermediate dir in the container — flattening to the basename would
    /// mount at a path the harness never reads, silently losing routing. This
    /// guards every non-claude-code adapter (claude-code's flat path can't
    /// catch it).
    #[cfg(feature = "sandbox")]
    #[test]
    fn gateway_config_preserves_nested_global_path() {
        let reg = crate::adapter::Registry::load().unwrap();
        let desc = reg.get("codex").expect("codex descriptor");
        assert!(
            desc.config.as_ref().unwrap().path.contains('/'),
            "test premise: codex's global config path is nested"
        );
        let cfg = render_gateway_config(desc, "/root", "http://egress-proxy:19080/mcp", "t")
            .unwrap()
            .expect("codex hosts HTTP MCP entries");
        assert_eq!(cfg.global_container, "/root/.codex/config.toml");
    }

    /// `read_recorded_posture` refuses run ids that aren't a single safe path
    /// segment — a stray `../` must never read outside the runs directory.
    #[test]
    fn read_recorded_posture_rejects_unsafe_ids() {
        for bad in ["", ".", "..", "../evil", "a/b", "x\0y"] {
            assert_eq!(read_recorded_posture(bad), None, "must reject {bad:?}");
        }
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

    /// P24: the sandbox prerequisite is checked BEFORE the posture banner. In a
    /// build without the `sandbox` feature (the default test build), a live
    /// `run --sandbox` refuses with the rebuild prerequisite plainly stated —
    /// and `--plan` still works with the feature off (it describes, never
    /// launches). Ordering: because `--plan` returns Ok while the live run
    /// refuses at the same prerequisite, the preflight sits between them —
    /// moving it above the `--plan` early return would break the plan case this
    /// asserts, so this test guards that the dry run keeps working with no
    /// Docker / no feature.
    #[test]
    #[cfg(not(feature = "sandbox"))]
    fn sandbox_prerequisite_refuses_before_launch_but_plan_still_works() {
        use assert_fs::prelude::*;
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        proj.child("agentstack.toml")
            .write_str("version = 1\n")
            .unwrap();

        let args = |plan: bool| RunArgs {
            harness: "claude-code".to_string(),
            locked: false,
            prompt: None,
            profile: None,
            scope: Some(agentstack_core::scope::Scope::Project),
            keep: false,
            sandbox: true,
            lockdown: false,
            plan,
            args: Vec::new(),
        };

        // Live run: refuses at the prerequisite, stating it plainly (rebuild).
        let err = run_sandboxed(Some(proj.path()), &args(false)).unwrap_err();
        let msg = format!("{err:#}");
        assert!(
            msg.contains("cargo build --features sandbox"),
            "prerequisite stated plainly: {msg}"
        );

        // --plan keeps working without the feature (or a daemon): it describes.
        run_sandboxed(Some(proj.path()), &args(true)).unwrap();

        std::env::remove_var("AGENTSTACK_HOME");
    }
}
