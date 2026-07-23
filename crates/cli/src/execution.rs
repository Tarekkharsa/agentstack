//! CLI composition for the policy-agnostic executor domain.
//!
//! This module is the only bridge between `agentstack-executor` and the
//! CLI-owned `Gateway`. It verifies project trust, freezes the gateway's
//! policy-filtered surface into an exact grant, and (with the sandbox feature)
//! dispatches the plan to the isolated hosted backend.

use std::sync::Arc;

use agentstack_executor::{
    AuthorityError, ExecuteError, ExecutePlan, ExecuteRequest, MachineLimits, PlanContext,
    RuntimeIdentity, ToolAuthority, ToolDescriptor, ToolGrant,
};
use serde_json::Value;

use crate::gateway::Gateway;

pub const EXECUTOR_IMAGE: &str =
    "node:22-slim@sha256:53ada149d435c38b14476cb57e4a7da73c15595aba79bd6971b547ceb6d018bf";
pub const EXECUTOR_PROTOCOL: u32 = 1;

pub struct GatewayAuthority {
    gateway: Arc<Gateway>,
}

impl GatewayAuthority {
    pub fn new(gateway: Arc<Gateway>) -> Self {
        Self { gateway }
    }
}

impl ToolAuthority for GatewayAuthority {
    fn describe(&self, name: &str) -> Option<ToolDescriptor> {
        self.gateway.describe(name).map(|detail| ToolDescriptor {
            name: detail.name,
            input_schema: detail.input_schema,
        })
    }

    fn call(&self, grant: &ToolGrant, name: &str, args: Value) -> Result<Value, AuthorityError> {
        if !grant.contains(name) {
            return Err(AuthorityError::NotGranted);
        }
        match self.gateway.try_call(name, &args) {
            Some(Ok(value)) => Ok(value),
            Some(Err(_)) => Err(AuthorityError::CallFailed),
            None => Err(AuthorityError::Unavailable),
        }
    }
}

#[cfg(feature = "sandbox")]
pub fn enabled() -> bool {
    matches!(
        agentstack_core::manifest::machine_experimental_health(),
        Some(Ok(config)) if config.tools_execute
    )
}

#[cfg(not(feature = "sandbox"))]
pub fn enabled() -> bool {
    false
}

fn configured_machine_limits() -> Result<MachineLimits, ExecuteError> {
    let defaults = MachineLimits::default();
    let config = match agentstack_core::manifest::machine_experimental_health() {
        None => return Ok(defaults),
        Some(Ok(config)) => config,
        Some(Err(_)) => return Err(ExecuteError::runtime_unavailable()),
    };
    let configured = MachineLimits {
        timeout_ms: config
            .tools_execute_limits
            .timeout_ms
            .unwrap_or(defaults.timeout_ms),
        max_calls: config
            .tools_execute_limits
            .max_calls
            .unwrap_or(defaults.max_calls),
        max_output_bytes: config
            .tools_execute_limits
            .max_output_bytes
            .unwrap_or(defaults.max_output_bytes),
    };
    configured
        .validate()
        .map_err(|_| ExecuteError::runtime_unavailable())
}

pub fn build(
    request: ExecuteRequest,
    dir: Option<&std::path::Path>,
    gateway: Arc<Gateway>,
) -> Result<(ExecutePlan, GatewayAuthority), ExecuteError> {
    let base = dir
        .map(crate::manifest::project_root_of)
        .or_else(|| std::env::current_dir().ok())
        .ok_or_else(ExecuteError::untrusted)?;
    if agentstack_trust::check(&base) != agentstack_trust::TrustState::Trusted {
        return Err(ExecuteError::untrusted());
    }
    let project_digest = agentstack_trust::digest_for(&base).ok_or_else(ExecuteError::untrusted)?;
    let tools = gateway.namespaced_tools();
    let authority_bytes = serde_json::to_vec(tools.as_ref()).unwrap_or_default();
    let authority_digest = agentstack_executor::digest_bytes(&authority_bytes);
    let context = PlanContext {
        execution_id: mint_id("x"),
        parent_run_id: std::env::var(agentstack_recorder::RUN_ID_ENV).ok(),
        project_digest,
        authority_digest,
        runtime: RuntimeIdentity {
            name: "node22-permission-model".into(),
            image: EXECUTOR_IMAGE.into(),
            protocol: EXECUTOR_PROTOCOL,
        },
        machine_limits: configured_machine_limits()?,
    };
    let authority = GatewayAuthority::new(gateway);
    let plan = agentstack_executor::build_plan(request, context, &authority)?;
    Ok((plan, authority))
}

fn mint_id(prefix: &str) -> String {
    let random = agentstack_core::util::random_bytes();
    let suffix = random
        .iter()
        .take(12)
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    format!("{prefix}-{suffix}")
}

#[cfg(feature = "sandbox")]
pub fn execute(
    request: ExecuteRequest,
    dir: Option<&std::path::Path>,
    gateway: Arc<Gateway>,
) -> Result<agentstack_executor::ExecutionOutput, ExecuteError> {
    let (plan, authority) = build(request, dir, gateway)?;
    hosted::run(&plan, &authority)
}

#[cfg(not(feature = "sandbox"))]
pub fn execute(
    _request: ExecuteRequest,
    _dir: Option<&std::path::Path>,
    _gateway: Arc<Gateway>,
) -> Result<agentstack_executor::ExecutionOutput, ExecuteError> {
    Err(ExecuteError::runtime_unavailable())
}

#[cfg(feature = "sandbox")]
mod hosted {
    use super::*;
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{Duration, Instant};

    use agentstack_executor::{ExecutionOutput, Executor};
    use agentstack_runtime::docker::DockerSandbox;
    use agentstack_runtime::{
        Lockdown, Mount, NetworkPolicy, Sandbox, SandboxSecurity, SandboxSpec, Stream, StreamChunk,
    };

    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            make_tree_removable(&self.0);
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    struct ExecutionFiles {
        root: PathBuf,
        app: PathBuf,
        ruleset: PathBuf,
        result: PathBuf,
    }

    struct HostedExecutor<'a> {
        authority: &'a GatewayAuthority,
    }

    impl Executor for HostedExecutor<'_> {
        fn execute(
            &self,
            plan: &ExecutePlan,
            _authority: &dyn ToolAuthority,
        ) -> Result<ExecutionOutput, ExecuteError> {
            execute_plan(plan, self.authority)
        }
    }

    pub(super) fn run(
        plan: &ExecutePlan,
        authority: &GatewayAuthority,
    ) -> Result<agentstack_executor::ExecutionOutput, ExecuteError> {
        HostedExecutor { authority }.execute(plan, authority)
    }

    fn execute_plan(
        plan: &ExecutePlan,
        authority: &GatewayAuthority,
    ) -> Result<ExecutionOutput, ExecuteError> {
        let started = Instant::now();
        let log_id = plan.parent_run_id.as_deref().unwrap_or(&plan.execution_id);
        let log = agentstack_recorder::RunLog::create(log_id)
            .ok_or_else(ExecuteError::runtime_unavailable)?;
        log.append(&agentstack_recorder::RunEvent::ExecutionStarted {
            ts: agentstack_recorder::now_epoch(),
            execution_id: plan.execution_id.clone(),
            parent_run_id: plan.parent_run_id.clone(),
            source_digest: plan.source_digest.clone(),
            input_digest: plan.input_digest.clone(),
            authority_digest: plan.authority_digest.clone(),
            runtime_digest: agentstack_executor::digest_bytes(plan.runtime.image.as_bytes()),
            granted_tools: plan.grant.iter().map(str::to_string).collect(),
            limits: serde_json::to_value(plan.limits).unwrap_or(Value::Null),
        });

        macro_rules! setup {
            ($result:expr, $outcome:literal) => {
                match $result {
                    Ok(value) => value,
                    Err(error) => {
                        finish_error(&log, plan, started, $outcome, 0, 0, 0);
                        return Err(error);
                    }
                }
            };
        }

        let files = setup!(prepare_files(plan), "setup-error");
        let _cleanup = Cleanup(files.root.clone());
        let token = mint_token();
        setup!(write_file(&files.app.join("token"), &token), "setup-error");

        let gateway = Arc::clone(&authority.gateway);
        let execution_id = plan.execution_id.clone();
        let parent_run_id = plan.parent_run_id.clone();
        let call: agentstack_egress::ExecutionCall = Arc::new(move |tool, args| {
            match gateway.try_call_for_execution(
                tool,
                &args,
                &execution_id,
                parent_run_id.as_deref(),
            ) {
                Some(Ok(value)) => Ok(value),
                Some(Err(_)) => Err(agentstack_egress::RelayCallError::Failed),
                None => Err(agentstack_egress::RelayCallError::Unavailable),
            }
        });
        let grant = plan
            .grant
            .iter()
            .map(str::to_string)
            .collect::<BTreeSet<_>>();

        // Connect to Docker before binding the relay: the narrowest safe bind
        // depends on the daemon's bridge gateway, so the listener must learn it
        // first.
        let backend = setup!(
            DockerSandbox::connect()
                .map_err(|error| runtime_unavailable("connecting to Docker", error)),
            "runtime-unavailable"
        );

        // Bind the per-execution tool-call relay to the narrowest host
        // interface that the sandbox sidecar can still reach via
        // host.docker.internal — NOT the `0.0.0.0` wildcard, which exposed the
        // token-authenticated relay on every (including LAN-facing) interface.
        // On a native Linux daemon the reachable-yet-narrow address is the
        // docker0 bridge gateway (a private, non-routable host interface); on
        // Docker Desktop it is the host loopback. The bridge gateway is only
        // looked up on Linux, because on Docker Desktop that gateway lives
        // inside the daemon's VM and is not a host interface. The per-run random
        // token, exact grant, call cap, and bounded frames remain the authority
        // boundary regardless of bind scope.
        let bridge_gateway = if std::env::consts::OS == "linux" {
            backend.default_bridge_gateway()
        } else {
            None
        };
        let relay_bind =
            agentstack_egress::relay_bind_address(std::env::consts::OS, bridge_gateway);
        let relay = setup!(
            agentstack_egress::BlockingExecutionRelay::start_on_or_unspecified(
                relay_bind,
                token,
                grant,
                plan.limits.max_calls,
                call,
            )
            .map_err(|error| runtime_unavailable("starting execution relay", error)),
            "relay-error"
        );

        // D4: the executor is always a lockdown run, so it gets the identical
        // gateway-only fence as `run --lockdown`. A selected server that can't
        // be dispatched to (resolve/pin failure, denied egress host, unbuildable
        // transport) means the run can't reach a tool it was asked for — refuse
        // rather than run a half-wired lockdown container. Route through `setup!`
        // so the refusal still appends the terminal ExecutionFinished event —
        // `ExecutionStarted` was already recorded above, and every exit from this
        // function must close the run (no started-without-terminal audit gap).
        setup!(
            refuse_if_servers_skipped(&authority.gateway),
            "lockdown-refused"
        );
        // Classify the D4 gateway-only fence from the gateway's FROZEN set with
        // the SAME function `run --lockdown` uses — one fence source, one
        // extractor, and fail-closed if any selected HTTP server has no
        // classifiable host or is unavailable. No second derivation from the
        // built upstreams. The literal-IP / non-TLS transport guards are already
        // on via `AGENTSTACK_LOCKDOWN`. Also through `setup!` so a fence-
        // classification failure closes the audit record.
        let mut executor_ruleset = authority.gateway.ruleset();
        executor_ruleset.gateway_only_hosts = setup!(
            crate::resolve::gateway_only_hosts(authority.gateway.frozen())
                .map_err(|e| runtime_unavailable("classifying gateway-only hosts", e)),
            "lockdown-refused"
        );
        let ruleset_json = setup!(
            serde_json::to_string(&executor_ruleset)
                .map_err(|_| ExecuteError::runtime_unavailable()),
            "setup-error"
        );
        setup!(write_file(&files.ruleset, &ruleset_json), "setup-error");
        setup!(make_app_readonly(&files.app), "setup-error");
        // The sink runs on the lockdown log-follower's tokio task, so parse +
        // append happen on a spool thread, not an async worker. Declared
        // before `lockdown` so it drops (and flushes) after the follower's
        // sender handle.
        let sink_log = agentstack_recorder::RunLog::create(log_id);
        let spool = setup!(
            agentstack_egress::WriterSpool::spawn("executor-events", move |line: String| {
                if let (Some(run_log), Ok(event)) = (
                    sink_log.as_ref(),
                    serde_json::from_str::<agentstack_recorder::RunEvent>(&line),
                ) {
                    run_log.append(&event);
                }
            })
            .map_err(|error| runtime_unavailable("starting the event writer", error)),
            "runtime-unavailable"
        );
        let events = spool.sender();
        let sink: agentstack_runtime::LockdownSink = Arc::new(move |line: &str| {
            events.send(line.to_string());
        });
        let relay_dest = format!("host.docker.internal:{}", relay.addr().port());
        // The executor never receives this token. Even if guest code imports
        // `node:net` and addresses the sidecar's normal proxy port directly,
        // the sidecar rejects it; the separately authenticated raw relay is
        // the only usable route out of the container.
        let proxy_token = mint_token();
        let lockdown = setup!(
            Lockdown::start(
                &backend,
                &plan.execution_id,
                &["executor".to_string()],
                &files.ruleset.display().to_string(),
                &egress_image(),
                Some(proxy_token),
                Some(&relay_dest),
                sink,
            )
            .map_err(|error| runtime_unavailable("starting lockdown topology", error)),
            "runtime-unavailable"
        );

        let spec = SandboxSpec {
            image: plan.runtime.image.clone(),
            command: vec![
                "node".into(),
                "--no-warnings".into(),
                "--experimental-permission".into(),
                "--allow-fs-read=/app".into(),
                "--allow-fs-write=/agentstack-result.json".into(),
                "--experimental-strip-types".into(),
                "/app/bootstrap.mjs".into(),
            ],
            mounts: vec![
                Mount {
                    host: files.app.display().to_string(),
                    container: "/app".into(),
                    read_only: true,
                },
                Mount {
                    host: files.result.display().to_string(),
                    container: "/agentstack-result.json".into(),
                    read_only: false,
                },
            ],
            workdir: "/app".into(),
            env: vec![],
            network: NetworkPolicy::Lockdown {
                network: lockdown.internal_network().into(),
            },
            ruleset: executor_ruleset,
            security: SandboxSecurity::hardened_executor(),
        };

        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let mut handle = setup!(
            backend
                .start(&spec)
                .map_err(|error| runtime_unavailable("starting executor container", error)),
            "runtime-unavailable"
        );
        let wait = handle.wait_streaming_bounded(
            Duration::from_millis(plan.limits.timeout_ms),
            plan.limits.max_output_bytes,
            &mut |chunk: StreamChunk| match chunk.stream {
                Stream::Stdout => stdout.extend_from_slice(&chunk.bytes),
                Stream::Stderr => stderr.extend_from_slice(&chunk.bytes),
            },
        );
        let teardown = handle.teardown();
        drop(lockdown);
        if teardown.is_err() {
            finish_error(
                &log,
                plan,
                started,
                "teardown-error",
                relay.call_count(),
                stdout.len(),
                stderr.len(),
            );
            return Err(ExecuteError::execution_error());
        }
        let exit = match wait {
            Ok(exit) => exit,
            Err(agentstack_runtime::RuntimeError::Timeout) => {
                log.append(&agentstack_recorder::RunEvent::ExecutionLimitHit {
                    ts: agentstack_recorder::now_epoch(),
                    execution_id: plan.execution_id.clone(),
                    limit: "timeoutMs".into(),
                    observed: plan.limits.timeout_ms,
                });
                finish_error(
                    &log,
                    plan,
                    started,
                    "timeout",
                    relay.call_count(),
                    stdout.len(),
                    stderr.len(),
                );
                return Err(ExecuteError::timeout());
            }
            Err(agentstack_runtime::RuntimeError::OutputLimit) => {
                log.append(&agentstack_recorder::RunEvent::ExecutionLimitHit {
                    ts: agentstack_recorder::now_epoch(),
                    execution_id: plan.execution_id.clone(),
                    limit: "maxOutputBytes".into(),
                    observed: plan.limits.max_output_bytes as u64 + 1,
                });
                finish_error(
                    &log,
                    plan,
                    started,
                    "resource-limit",
                    relay.call_count(),
                    stdout.len(),
                    stderr.len(),
                );
                return Err(ExecuteError::resource_limit());
            }
            Err(_) => {
                finish_error(
                    &log,
                    plan,
                    started,
                    "runtime-error",
                    relay.call_count(),
                    stdout.len(),
                    stderr.len(),
                );
                return Err(ExecuteError::execution_error());
            }
        };
        if exit.code != Some(0) {
            #[cfg(test)]
            eprintln!("executor stderr: {}", String::from_utf8_lossy(&stderr));
            finish_error(
                &log,
                plan,
                started,
                "execution-error",
                relay.call_count(),
                stdout.len(),
                stderr.len(),
            );
            return Err(ExecuteError::execution_error());
        }
        let result = match parse_result(&files.result) {
            Ok(result) => result,
            Err(error) => {
                finish_error(
                    &log,
                    plan,
                    started,
                    "invalid-result",
                    relay.call_count(),
                    stdout.len(),
                    stderr.len(),
                );
                return Err(error);
            }
        };
        let output = match (ExecutionOutput {
            result,
            calls: relay.call_count(),
            duration_ms: started.elapsed().as_millis() as u64,
            stdout_bytes: stdout.len(),
            stderr_bytes: stderr.len(),
        })
        .validate(plan)
        {
            Ok(output) => output,
            Err(error) => {
                finish_error(
                    &log,
                    plan,
                    started,
                    "resource-limit",
                    relay.call_count(),
                    stdout.len(),
                    stderr.len(),
                );
                return Err(error);
            }
        };
        log.append(&agentstack_recorder::RunEvent::ExecutionFinished {
            ts: agentstack_recorder::now_epoch(),
            execution_id: plan.execution_id.clone(),
            outcome: "ok".into(),
            duration_ms: output.duration_ms,
            calls: output.calls,
            result_digest: Some(agentstack_executor::digest_bytes(
                &serde_json::to_vec(&output.result).unwrap_or_default(),
            )),
            stdout_bytes: output.stdout_bytes,
            stderr_bytes: output.stderr_bytes,
        });
        Ok(output)
    }

    fn finish_error(
        log: &agentstack_recorder::RunLog,
        plan: &ExecutePlan,
        started: Instant,
        outcome: &str,
        calls: u32,
        stdout_bytes: usize,
        stderr_bytes: usize,
    ) {
        log.append(&agentstack_recorder::RunEvent::ExecutionFinished {
            ts: agentstack_recorder::now_epoch(),
            execution_id: plan.execution_id.clone(),
            outcome: outcome.into(),
            duration_ms: started.elapsed().as_millis() as u64,
            calls,
            result_digest: None,
            stdout_bytes,
            stderr_bytes,
        });
    }

    fn prepare_files(plan: &ExecutePlan) -> Result<ExecutionFiles, ExecuteError> {
        let root = agentstack_core::util::paths::agentstack_home()
            .join("runs")
            .join(&plan.execution_id)
            .join("executor");
        let app = root.join("app");
        let control = root.join("control");
        let ruleset = control.join("ruleset.json");
        let result = root.join("result.json");
        fs::create_dir_all(&app).map_err(|_| ExecuteError::runtime_unavailable())?;
        fs::create_dir_all(&control).map_err(|_| ExecuteError::runtime_unavailable())?;
        write_file(&app.join("source.ts"), &resolve_virtual_import(&plan.code))?;
        write_file(
            &app.join("input.json"),
            &serde_json::to_string(&plan.input).map_err(|_| ExecuteError::execution_error())?,
        )?;
        write_file(&app.join("bootstrap.mjs"), BOOTSTRAP)?;
        write_file(&app.join("runtime.mjs"), &runtime_sdk(plan))?;
        write_file(&result, "")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // The surrounding run directory is private. This single bind file
            // must be writable by the container's non-root uid.
            fs::set_permissions(&result, fs::Permissions::from_mode(0o666))
                .map_err(|_| ExecuteError::runtime_unavailable())?;
            fs::set_permissions(&control, fs::Permissions::from_mode(0o700))
                .map_err(|_| ExecuteError::runtime_unavailable())?;
        }
        Ok(ExecutionFiles {
            root,
            app,
            ruleset,
            result,
        })
    }

    fn write_file(path: &Path, content: &str) -> Result<(), ExecuteError> {
        fs::write(path, content).map_err(|_| ExecuteError::runtime_unavailable())
    }

    fn make_app_readonly(root: &Path) -> Result<(), ExecuteError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for entry in fs::read_dir(root).map_err(|_| ExecuteError::runtime_unavailable())? {
                let path = entry
                    .map_err(|_| ExecuteError::runtime_unavailable())?
                    .path();
                fs::set_permissions(path, fs::Permissions::from_mode(0o444))
                    .map_err(|_| ExecuteError::runtime_unavailable())?;
            }
            fs::set_permissions(root, fs::Permissions::from_mode(0o555))
                .map_err(|_| ExecuteError::runtime_unavailable())?;
        }
        Ok(())
    }

    fn make_tree_removable(root: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(entries) = fs::read_dir(root) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        make_tree_removable(&path);
                    }
                    let mode = if path.is_dir() { 0o700 } else { 0o600 };
                    let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
                }
            }
            let _ = fs::set_permissions(root, fs::Permissions::from_mode(0o700));
        }
    }

    fn parse_result(path: &Path) -> Result<Value, ExecuteError> {
        let text = agentstack_core::util::read_to_string_bounded(
            path,
            agentstack_executor::MAX_RESULT_BYTES as u64,
        )
        .map_err(|_| ExecuteError::invalid_result())?;
        serde_json::from_str(&text).map_err(|_| ExecuteError::invalid_result())
    }

    fn resolve_virtual_import(source: &str) -> String {
        source
            .replace("from \"agentstack:runtime\"", "from \"./runtime.mjs\"")
            .replace("from 'agentstack:runtime'", "from './runtime.mjs'")
    }

    fn mint_token() -> String {
        agentstack_core::util::random_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }

    fn egress_image() -> String {
        std::env::var("AGENTSTACK_EGRESS_IMAGE").unwrap_or_else(|_| {
            concat!(
                "ghcr.io/tarekkharsa/agentstack-egress-proxy:v",
                env!("CARGO_PKG_VERSION")
            )
            .to_string()
        })
    }

    /// Refuse a lockdown executor run when the gateway skipped a selected server
    /// (D4). The gateway-only fence is now classified from the frozen set, so a
    /// skipped server's host is still fenced — but a skipped server also can't be
    /// dispatched to, so the run couldn't reach a tool it was asked for. Fail
    /// closed, naming the skipped servers, rather than run a half-wired
    /// container.
    pub(super) fn refuse_if_servers_skipped(gateway: &Gateway) -> Result<(), ExecuteError> {
        let skipped = gateway.skipped_servers();
        if skipped.is_empty() {
            return Ok(());
        }
        Err(runtime_unavailable(
            "lockdown executor refusing to run",
            format!(
                "{} selected server(s) could not be served and would escape the \
                 gateway-only fence: {}",
                skipped.len(),
                skipped.join(", ")
            ),
        ))
    }

    fn runtime_unavailable(context: &str, error: impl std::fmt::Display) -> ExecuteError {
        eprintln!("tools_execute: {context}: {error}");
        ExecuteError::runtime_unavailable()
    }

    fn runtime_sdk(plan: &ExecutePlan) -> String {
        let mut servers = serde_json::Map::new();
        for name in plan.grant.iter() {
            let Some((server, tool)) = name.split_once("__") else {
                continue;
            };
            let server_value = servers
                .entry(server.to_string())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            if let Value::Object(tools) = server_value {
                tools.insert(tool.to_string(), Value::String(name.to_string()));
            }
        }
        RUNTIME_TEMPLATE.replace(
            "__BINDINGS__",
            &serde_json::to_string(&Value::Object(servers)).unwrap_or_else(|_| "{}".into()),
        )
    }

    const BOOTSTRAP: &str = r#"import fs from "node:fs";
import result from "./source.ts";
const value = await result;
const encoded = JSON.stringify(value);
if (encoded === undefined) throw new Error("result is not JSON serializable");
fs.writeFileSync("/agentstack-result.json", encoded, "utf8");
"#;

    const RUNTIME_TEMPLATE: &str = r#"import net from "node:net";
import fs from "node:fs";
export const input = JSON.parse(fs.readFileSync("/app/input.json", "utf8"));
const token = fs.readFileSync("/app/token", "utf8").trim();
let nextId = 1;
function invoke(tool, args) {
  if (!args || Array.isArray(args) || typeof args !== "object") return Promise.reject(new Error("tool arguments must be an object"));
  return new Promise((resolve, reject) => {
    const socket = net.createConnection({ host: "egress-proxy", port: 19080 });
    let buffer = "";
    socket.setEncoding("utf8");
    socket.on("connect", () => socket.write(JSON.stringify({ id: nextId++, token, tool, arguments: args }) + "\n"));
    socket.on("data", chunk => {
      buffer += chunk;
      const end = buffer.indexOf("\n");
      if (end < 0) return;
      socket.end();
      try {
        const response = JSON.parse(buffer.slice(0, end));
        response.ok ? resolve(response.result) : reject(new Error(response.error || "tool call failed"));
      } catch { reject(new Error("invalid tool response")); }
    });
    socket.on("error", () => reject(new Error("tool relay unavailable")));
  });
}
const bindings = __BINDINGS__;
export const tools = Object.fromEntries(Object.entries(bindings).map(([server, entries]) => [
  server,
  Object.fromEntries(Object.entries(entries).map(([tool, wire]) => [tool, args => invoke(wire, args)]))
]));
"#;
}

#[cfg(all(test, feature = "sandbox"))]
mod tests {
    use super::*;
    use assert_fs::prelude::*;
    use serde_json::json;

    /// A selected server the gateway could NOT serve (here: an unresolvable
    /// frozen entry) leaves a non-empty `skipped_servers()`, and the lockdown
    /// executor must refuse — the skipped host would escape the gateway-only
    /// fence. An all-served gateway does not refuse.
    #[test]
    fn skipped_selected_server_refuses_the_lockdown_executor() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        std::env::remove_var(crate::calllog::RUN_ID_ENV);
        let proj = assert_fs::TempDir::new().unwrap();
        proj.child(".agentstack/agentstack.toml")
            .write_str("version = 1\n[servers.x]\ntype = \"http\"\nurl = \"https://x/mcp\"\n")
            .unwrap();
        crate::trust::trust_unreviewed(proj.path()).unwrap();
        let ruleset = || {
            agentstack_policy::compile(
                &crate::manifest::Policy::default(),
                &crate::manifest::Policy::default(),
                &["x"],
            )
        };

        // An unresolvable frozen entry → the gateway skips it → refuse.
        let skipped = crate::gateway::Gateway::from_frozen(
            Some(proj.path()),
            ruleset(),
            vec![("x".to_string(), Err("resolve failed".to_string()))],
            "r-skip",
        );
        assert_eq!(skipped.skipped_servers(), ["x"]);
        assert!(super::hosted::refuse_if_servers_skipped(&skipped).is_err());

        // The same server served → nothing skipped → no refusal.
        let served = crate::gateway::Gateway::from_frozen(
            Some(proj.path()),
            ruleset(),
            vec![(
                "x".to_string(),
                Ok(crate::resolve::ResolvedServer {
                    name: "x".into(),
                    origin: crate::resolve::ServerOrigin::Inline,
                    server: toml::from_str("type = \"http\"\nurl = \"https://x/mcp\"\n").unwrap(),
                    checksum: String::new(),
                    provenance: None,
                }),
            )],
            "r-ok",
        );
        assert!(served.skipped_servers().is_empty());
        assert!(super::hosted::refuse_if_servers_skipped(&served).is_ok());
        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn machine_manifest_configures_execute_limits_with_hard_ceiling_validation() {
        let _guard = agentstack_core::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        home.child("agentstack.toml")
            .write_str(
                r#"version = 1
[experimental]
tools_execute = true

[experimental.tools_execute_limits]
timeout_ms = 30000
max_calls = 40
max_output_bytes = 131072
"#,
            )
            .unwrap();
        assert_eq!(
            configured_machine_limits().unwrap(),
            MachineLimits {
                timeout_ms: 30_000,
                max_calls: 40,
                max_output_bytes: 128 * 1024,
            }
        );

        home.child("agentstack.toml")
            .write_str(
                r#"version = 1
[experimental.tools_execute_limits]
timeout_ms = 60001
"#,
            )
            .unwrap();
        assert_eq!(
            configured_machine_limits().unwrap_err().category,
            agentstack_executor::ErrorCategory::RuntimeUnavailable
        );
        std::env::remove_var("AGENTSTACK_HOME");
    }

    #[test]
    fn docker_executor_runs_typescript_without_workspace_or_direct_network() {
        let _guard = agentstack_core::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if std::process::Command::new("docker")
            .args(["image", "inspect", "agentstack/egress-proxy:test"])
            .output()
            .map(|output| !output.status.success())
            .unwrap_or(true)
        {
            eprintln!("SKIP: agentstack/egress-proxy:test is unavailable");
            return;
        }
        let home = assert_fs::TempDir::new().unwrap();
        let project = assert_fs::TempDir::new().unwrap();
        project
            .child(".agentstack/agentstack.toml")
            .write_str(
                r#"version = 1
[servers.demo]
type = "stdio"
command = "sh"
args = ["server.sh"]
"#,
            )
            .unwrap();
        project
            .child("server.sh")
            .write_str(
                r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *\"method\":\"initialize\"*) printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2025-03-26","capabilities":{},"serverInfo":{"name":"fixture","version":"1"}}}' ;;
    *\"method\":\"tools/list\"*) printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"tools":[{"name":"echo","description":"echo","inputSchema":{"type":"object"}}]}}' ;;
    *\"method\":\"tools/call\"*) printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"content":[{"type":"text","text":"relay-ok"}],"isError":false}}' ;;
  esac
done
"#,
            )
            .unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        std::env::set_var("AGENTSTACK_EGRESS_IMAGE", "agentstack/egress-proxy:test");
        agentstack_trust::trust_unreviewed(project.path()).unwrap();
        let gateway = Arc::new(crate::gateway::Gateway::from_manifest(Some(project.path())));
        let request: ExecuteRequest = serde_json::from_value(json!({
            "code": "import { tools, input } from 'agentstack:runtime'; import fs from 'node:fs'; import net from 'node:net'; let fsDenied = false; try { fs.readFileSync('/etc/passwd'); } catch { fsDenied = true; } let policyHidden = false; try { fs.readFileSync('/app/ruleset.json'); } catch { policyHidden = true; } const directBlocked = await new Promise(resolve => { const socket = net.createConnection({ host: '1.1.1.1', port: 443 }); socket.setTimeout(500); socket.on('connect', () => { socket.destroy(); resolve(false); }); socket.on('error', () => resolve(true)); socket.on('timeout', () => { socket.destroy(); resolve(true); }); }); const proxyBlocked = await new Promise(resolve => { let data = ''; const socket = net.createConnection({ host: 'egress-proxy', port: 18080 }); socket.setTimeout(1000); socket.on('connect', () => socket.write('CONNECT 1.1.1.1:443 HTTP/1.1\\r\\nHost: 1.1.1.1:443\\r\\n\\r\\n')); socket.on('data', chunk => { data += chunk.toString(); if (data.includes('\\r\\n\\r\\n')) { socket.destroy(); resolve(data.includes(' 407 ')); } }); socket.on('error', () => resolve(true)); socket.on('timeout', () => { socket.destroy(); resolve(true); }); }); const reply = await tools.demo.echo({ msg: input.msg }); export default { text: reply.content[0].text, fsDenied, policyHidden, directBlocked, proxyBlocked };",
            "allowTools": ["demo__echo"],
            "input": { "msg": "hello" },
            "limits": { "timeoutMs": 10000 }
        }))
        .unwrap();
        let result = execute(request, Some(project.path()), Arc::clone(&gateway)).unwrap();
        assert_eq!(
            result.result,
            json!({ "text": "relay-ok", "fsDenied": true, "policyHidden": true, "directBlocked": true, "proxyBlocked": true })
        );
        assert_eq!(result.calls, 1);

        let timeout: ExecuteRequest = serde_json::from_value(json!({
            "code": "while (true) {} export default 1;",
            "allowTools": ["demo__echo"],
            "limits": { "timeoutMs": 300 }
        }))
        .unwrap();
        assert_eq!(
            execute(timeout, Some(project.path()), Arc::clone(&gateway))
                .unwrap_err()
                .category,
            agentstack_executor::ErrorCategory::Timeout
        );

        let excessive_output: ExecuteRequest = serde_json::from_value(json!({
            "code": "console.log('x'.repeat(100000)); export default 1;",
            "allowTools": ["demo__echo"],
            "limits": { "maxOutputBytes": 1024 }
        }))
        .unwrap();
        assert_eq!(
            execute(excessive_output, Some(project.path()), gateway)
                .unwrap_err()
                .category,
            agentstack_executor::ErrorCategory::ResourceLimit
        );
        let large_result: ExecuteRequest = serde_json::from_value(json!({
            "code": "export default { blob: 'x'.repeat(100000) };",
            "allowTools": ["demo__echo"],
            "limits": { "maxOutputBytes": 1024 }
        }))
        .unwrap();
        let gateway = Arc::new(crate::gateway::Gateway::from_manifest(Some(project.path())));
        let output = execute(large_result, Some(project.path()), gateway).unwrap();
        assert_eq!(output.result["blob"].as_str().unwrap().len(), 100000);

        for entry in std::fs::read_dir(home.path().join("runs")).unwrap() {
            let entry = entry.unwrap();
            assert!(
                !entry.path().join("executor").exists(),
                "executor staging tree should be removed: {}",
                entry.path().display()
            );
            let execution_id = entry.file_name().to_string_lossy().into_owned();
            if !execution_id.starts_with("x-") {
                continue;
            }
            for network in [
                format!("agentstack-lock-{execution_id}"),
                format!("agentstack-egress-{execution_id}"),
            ] {
                let output = std::process::Command::new("docker")
                    .args(["network", "inspect", &network])
                    .output()
                    .unwrap();
                assert!(
                    !output.status.success(),
                    "execution leaked Docker network {network}"
                );
            }
        }
        std::env::remove_var("AGENTSTACK_EGRESS_IMAGE");
        std::env::remove_var("AGENTSTACK_HOME");
    }
}
