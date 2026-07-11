//! No-direct-route lockdown topology for `run --sandbox --lockdown`.
//!
//! The [`ProxyOnly`](crate::NetworkPolicy::ProxyOnly) mode points the
//! container's `HTTPS_PROXY` at a proxy on the host — real enforcement of the
//! agent's *configured* egress, but a container that ignored the env could
//! still reach the open internet directly. Lockdown removes that escape hatch
//! by construction:
//!
//! ```text
//!   ┌ internal network (no host route, no internet, no DNS beyond it) ┐
//!   │   sandbox container ──HTTPS_PROXY──▶ egress-proxy (alias)        │
//!   └──────────────────────────────────────────────│─────────────────┘
//!                                                   │ (dual-homed)
//!                              egress network (ordinary bridge → internet)
//! ```
//!
//! The sidecar proxy is the ONLY peer the sandbox can reach; it is dual-homed
//! onto a second, ordinary network so it (and only it) can forward allowed
//! traffic out. Ignoring the proxy env gets the sandbox nothing — there is no
//! other route.
//!
//! [`Lockdown::start`] creates both networks, launches the sidecar (from the
//! image built by `docker/egress-proxy.Dockerfile`), follows its logs on the
//! backend's runtime — turning each emitted [`RunEvent`] into a `sink` call
//! and watching for the `READY` lines that mean every listener is bound — and
//! returns once the proxy is ready. Dropping the returned handle tears the
//! sidecar and both networks down; the caller runs (and reaps) the sandbox
//! container in between.
//!
//! Docker-only: this module compiles under the `docker` feature and is
//! behavior-verified by a daemon-gated test (skipped where none is available).

use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;

use bollard::models::{
    ContainerCreateBody, EndpointSettings, HostConfig, NetworkConnectRequest, NetworkCreateRequest,
};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    StartContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;

use crate::docker::DockerSandbox;
use crate::{Result, RuntimeError};

/// The alias the sidecar proxy answers to on the internal network. The
/// caller points the sandbox's `HTTPS_PROXY` at `http://<this>:<base_port>`.
pub const PROXY_ALIAS: &str = "egress-proxy";

/// The port the sidecar's first (and, today, only) server-proxy listens on —
/// matches the binary's `DEFAULT_BASE_PORT`.
pub const PROXY_BASE_PORT: u16 = 18080;

/// How long to wait for the sidecar to print `READY` before giving up.
const READY_TIMEOUT: Duration = Duration::from_secs(20);

/// A callback the lockdown log-follower invokes with each candidate event line
/// the sidecar emits — a raw JSON line (one serialized `RunEvent`, the same
/// `{"event":…}` form `events.jsonl` holds). Runtime forwards the bytes rather
/// than deserializing them: parsing lives with the cli, which owns the
/// recorder + `serde_json` (keeping runtime on its `core`/`policy`/`recorder`
/// edges). Send + Sync + 'static so it can run on the spawned follow task.
pub type LockdownSink = Arc<dyn Fn(&str) + Send + Sync>;

/// A live lockdown topology for one run. Hold it for the sandbox's lifetime;
/// dropping it removes the sidecar and both networks.
pub struct Lockdown {
    rt: Arc<Runtime>,
    docker: Docker,
    internal_net: String,
    egress_net: String,
    sidecar_id: String,
    follow: Option<JoinHandle<()>>,
}

impl Lockdown {
    /// Stand up the lockdown networks + sidecar for `run_id`.
    ///
    /// `servers` are the egress identities the sidecar proxies (one listener
    /// each, `PROXY_BASE_PORT + i`); `ruleset_json_path` is a host file
    /// holding the serialized compiled ruleset, mounted read-only into the
    /// sidecar; `proxy_image` is the sidecar image tag. Every [`RunEvent`] the
    /// sidecar emits is handed to `sink`. Returns once the proxy is READY, or
    /// an error (after cleaning up whatever it created) if it never is.
    pub fn start(
        backend: &DockerSandbox,
        run_id: &str,
        servers: &[String],
        ruleset_json_path: &str,
        proxy_image: &str,
        sink: LockdownSink,
    ) -> Result<Lockdown> {
        let rt = Arc::clone(backend.runtime());
        let docker = backend.client();
        let internal_net = format!("agentstack-lock-{run_id}");
        let egress_net = format!("agentstack-egress-{run_id}");
        let servers_env = servers.join(",");

        // A oneshot-ish signal from the follow task: Ok when READY seen, Err
        // with the sidecar's own output if it exited/failed first.
        let (ready_tx, ready_rx) = mpsc::channel::<std::result::Result<(), String>>();

        let created = rt.block_on(async {
            // Two networks: internal (no route out) for the sandbox↔proxy hop,
            // and an ordinary bridge so the dual-homed proxy can forward out.
            create_network(&docker, &internal_net, true).await?;
            create_network(&docker, &egress_net, false).await?;

            // The sidecar: created on the egress net (its route to the
            // internet), then also connected to the internal net under the
            // alias the sandbox resolves.
            let sidecar_id = start_sidecar(
                &docker,
                proxy_image,
                &egress_net,
                ruleset_json_path,
                &servers_env,
            )
            .await?;
            connect_with_alias(&docker, &internal_net, &sidecar_id, PROXY_ALIAS).await?;
            Ok::<String, RuntimeError>(sidecar_id)
        });

        let sidecar_id = match created {
            Ok(id) => id,
            Err(e) => {
                // Best-effort cleanup of any half-built topology.
                rt.block_on(async {
                    let _ = remove_network(&docker, &internal_net).await;
                    let _ = remove_network(&docker, &egress_net).await;
                });
                return Err(e);
            }
        };

        // Follow the sidecar's logs for the run's lifetime: parse READY (once)
        // and forward every RunEvent to the sink.
        let follow = rt.spawn(follow_sidecar_logs(
            docker.clone(),
            sidecar_id.clone(),
            sink,
            ready_tx,
        ));

        // Block until READY, the sidecar dies, or the timeout — then hand back
        // a live handle whose Drop tears everything down.
        let lock = Lockdown {
            rt: Arc::clone(&rt),
            docker: docker.clone(),
            internal_net,
            egress_net,
            sidecar_id,
            follow: Some(follow),
        };
        match ready_rx.recv_timeout(READY_TIMEOUT) {
            Ok(Ok(())) => Ok(lock),
            Ok(Err(reason)) => {
                Err(RuntimeError::Backend(format!(
                    "egress sidecar failed: {reason}"
                )))
                // lock drops here → cleanup.
            }
            Err(_) => Err(RuntimeError::Backend(
                "egress sidecar did not become ready in time".to_string(),
            )),
        }
    }

    /// The internal network the sandbox must attach to
    /// ([`NetworkPolicy::Lockdown`](crate::NetworkPolicy::Lockdown)).
    pub fn internal_network(&self) -> &str {
        &self.internal_net
    }

    /// The `HTTPS_PROXY` value the sandbox should carry (`http://alias:port`).
    pub fn proxy_endpoint(&self) -> String {
        format!("http://{PROXY_ALIAS}:{PROXY_BASE_PORT}")
    }
}

impl Drop for Lockdown {
    fn drop(&mut self) {
        // Stop following, then remove the sidecar (which frees both networks),
        // then the networks. All best-effort: a teardown failure must not mask
        // the run's own result, and a leaked network is visible, not silent.
        if let Some(f) = self.follow.take() {
            f.abort();
        }
        let docker = self.docker.clone();
        let sidecar = self.sidecar_id.clone();
        let internal = self.internal_net.clone();
        let egress = self.egress_net.clone();
        self.rt.block_on(async move {
            let opts = RemoveContainerOptionsBuilder::default().force(true).build();
            let _ = docker.remove_container(&sidecar, Some(opts)).await;
            let _ = remove_network(&docker, &internal).await;
            let _ = remove_network(&docker, &egress).await;
        });
    }
}

/// Create a user-defined bridge network; `internal` cuts its route to the host
/// and the outside world.
async fn create_network(docker: &Docker, name: &str, internal: bool) -> Result<()> {
    docker
        .create_network(NetworkCreateRequest {
            name: name.to_string(),
            driver: Some("bridge".to_string()),
            internal: Some(internal),
            ..Default::default()
        })
        .await
        .map_err(|e| RuntimeError::Backend(format!("creating network {name}: {e}")))?;
    Ok(())
}

async fn remove_network(docker: &Docker, name: &str) -> Result<()> {
    docker
        .remove_network(name)
        .await
        .map_err(|e| RuntimeError::Backend(format!("removing network {name}: {e}")))
}

/// Create + start the sidecar proxy on `egress_net`, mounting the ruleset file
/// read-only and passing the server list through the env contract the binary
/// reads. Returns the container id.
async fn start_sidecar(
    docker: &Docker,
    image: &str,
    egress_net: &str,
    ruleset_json_path: &str,
    servers_env: &str,
) -> Result<String> {
    const RULESET_IN: &str = "/agentstack/ruleset.json";
    let host_config = HostConfig {
        binds: Some(vec![format!("{ruleset_json_path}:{RULESET_IN}:ro")]),
        network_mode: Some(egress_net.to_string()),
        // The sidecar needs to reach real allowed hosts; in the demo the
        // "allowed" target is on the host, so map host.docker.internal on its
        // egress leg (the internal leg the sandbox uses has no such entry).
        extra_hosts: Some(vec!["host.docker.internal:host-gateway".to_string()]),
        ..Default::default()
    };
    let env = vec![
        format!("AGENTSTACK_RULESET={RULESET_IN}"),
        format!("AGENTSTACK_SERVERS={servers_env}"),
        format!("AGENTSTACK_PROXY_BASE_PORT={PROXY_BASE_PORT}"),
    ];
    let body = ContainerCreateBody {
        image: Some(image.to_string()),
        env: Some(env),
        host_config: Some(host_config),
        ..Default::default()
    };
    let created = docker
        .create_container(Some(CreateContainerOptionsBuilder::default().build()), body)
        .await
        .map_err(|e| RuntimeError::Backend(format!("creating egress sidecar: {e}")))?;
    docker
        .start_container(&created.id, None::<StartContainerOptions>)
        .await
        .map_err(|e| RuntimeError::Backend(format!("starting egress sidecar: {e}")))?;
    Ok(created.id)
}

/// Attach an already-running container to `network` under `alias`, so peers on
/// that network resolve `alias` to it via Docker's embedded DNS.
async fn connect_with_alias(
    docker: &Docker,
    network: &str,
    container_id: &str,
    alias: &str,
) -> Result<()> {
    docker
        .connect_network(
            network,
            NetworkConnectRequest {
                container: container_id.to_string(),
                endpoint_config: Some(EndpointSettings {
                    aliases: Some(vec![alias.to_string()]),
                    ..Default::default()
                }),
            },
        )
        .await
        .map_err(|e| {
            RuntimeError::Backend(format!("attaching sidecar to {network} as {alias}: {e}"))
        })?;
    Ok(())
}

/// Follow the sidecar container's logs to end-of-stream. Each `READY` line
/// signals (once) via `ready_tx`; each `{…}` line is forwarded verbatim to
/// `sink` (the cli parses it into a `RunEvent`). If the stream ends before
/// READY, report the tail so the caller sees why.
async fn follow_sidecar_logs(
    docker: Docker,
    id: String,
    sink: LockdownSink,
    ready_tx: mpsc::Sender<std::result::Result<(), String>>,
) {
    let opts = LogsOptionsBuilder::default()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .build();
    let mut logs = docker.logs(&id, Some(opts));
    let mut signalled = false;
    let mut tail = String::new();
    let mut line = String::new();

    while let Some(next) = logs.next().await {
        let bytes = match next {
            Ok(out) => out.into_bytes(),
            Err(_) => break,
        };
        for &b in bytes.iter() {
            if b == b'\n' {
                handle_line(&line, &sink, &ready_tx, &mut signalled, &mut tail);
                line.clear();
            } else {
                line.push(b as char);
            }
        }
    }
    // Flush any unterminated final line, then, if we never saw READY, report
    // the sidecar's tail as the failure reason.
    if !line.is_empty() {
        handle_line(&line, &sink, &ready_tx, &mut signalled, &mut tail);
    }
    if !signalled {
        let _ = ready_tx.send(Err(if tail.is_empty() {
            "sidecar exited before becoming ready".to_string()
        } else {
            tail
        }));
    }
}

/// Classify one sidecar log line: `READY …` → the ready signal; a `{…}` line →
/// forwarded to the sink (the cli parses it); anything else → remembered as
/// diagnostic tail.
fn handle_line(
    line: &str,
    sink: &LockdownSink,
    ready_tx: &mpsc::Sender<std::result::Result<(), String>>,
    signalled: &mut bool,
    tail: &mut String,
) {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return;
    }
    if trimmed.starts_with("READY") {
        if !*signalled {
            *signalled = true;
            let _ = ready_tx.send(Ok(()));
        }
        return;
    }
    if trimmed.starts_with('{') {
        sink(trimmed);
        return;
    }
    // Diagnostic (e.g. a startup error on stderr): keep the most recent line.
    *tail = trimmed.to_string();
}
