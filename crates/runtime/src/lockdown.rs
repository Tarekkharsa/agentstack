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
    CreateContainerOptionsBuilder, CreateImageOptionsBuilder, LogsOptionsBuilder,
    RemoveContainerOptionsBuilder, StartContainerOptions,
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

/// The port the sidecar's OPTIONAL gateway relay listens on (lockdown gateway
/// routing). Chosen well clear of `PROXY_BASE_PORT + i` server listeners so it
/// never collides regardless of server count. The host renders the container's
/// gateway config to `http://<PROXY_ALIAS>:<this>` and tells the sidecar to
/// listen here via env, so the number is owned in one place.
pub const GATEWAY_RELAY_PORT: u16 = 19080;

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
    auth_token: Option<String>,
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
    #[allow(clippy::too_many_arguments)]
    pub fn start(
        backend: &DockerSandbox,
        run_id: &str,
        servers: &[String],
        ruleset_json_path: &str,
        proxy_image: &str,
        auth_token: Option<String>,
        gateway_relay_dest: Option<&str>,
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
            // The sidecar image is pulled if absent (bollard's create_container
            // does NOT auto-pull like the docker CLI) — this is what makes
            // `--lockdown` zero-config against the published, version-pinned
            // GHCR image. Only the sidecar gets this treatment: the sandbox
            // RUNNER image is user-built by design and is never pulled.
            ensure_image(&docker, proxy_image).await?;

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
                auth_token.as_deref(),
                gateway_relay_dest,
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
            auth_token,
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

    /// The `HTTPS_PROXY` value the sandbox should carry. When a peer token is
    /// set it rides in the URL userinfo (`http://agentstack:<token>@alias:port`)
    /// so the container's client sends `Proxy-Authorization` on CONNECT.
    pub fn proxy_endpoint(&self) -> String {
        match &self.auth_token {
            Some(t) => format!("http://agentstack:{t}@{PROXY_ALIAS}:{PROXY_BASE_PORT}"),
            None => format!("http://{PROXY_ALIAS}:{PROXY_BASE_PORT}"),
        }
    }

    /// The base URL the sandbox uses to reach the host gateway through the
    /// sidecar relay (lockdown gateway routing): `http://<alias>:<relay port>`.
    /// No auth in the URL — the relay is a dumb pipe; the gateway checks the
    /// per-run bearer token the container's config carries as a header.
    pub fn relay_endpoint(&self) -> String {
        format!("http://{PROXY_ALIAS}:{GATEWAY_RELAY_PORT}")
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
/// Make sure `image` exists locally, pulling it if not. A locally present
/// image is NEVER re-pulled — the default tag is version-pinned, so "present"
/// means "the one this binary was released with", and a floating local
/// override stays exactly what the user built.
async fn ensure_image(docker: &Docker, image: &str) -> Result<()> {
    match docker.inspect_image(image).await {
        Ok(_) => return Ok(()),
        // 404 = not present locally → pull. Any other error (daemon down,
        // permission) is real and propagates.
        Err(bollard::errors::Error::DockerResponseServerError {
            status_code: 404, ..
        }) => {}
        Err(e) => return Err(RuntimeError::Backend(e.to_string())),
    }

    let opts = CreateImageOptionsBuilder::default()
        .from_image(image)
        .build();
    // create_image streams pull progress; drain it, keeping only errors.
    let mut pull = docker.create_image(Some(opts), None, None);
    while let Some(step) = pull.next().await {
        step.map_err(|e| {
            RuntimeError::Backend(format!(
                "pulling egress sidecar image {image}: {e} \
                 (build it locally from docker/egress-proxy.Dockerfile and set \
                 AGENTSTACK_EGRESS_IMAGE to use your own tag)"
            ))
        })?;
    }
    Ok(())
}

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
#[allow(clippy::too_many_arguments)]
async fn start_sidecar(
    docker: &Docker,
    image: &str,
    egress_net: &str,
    ruleset_json_path: &str,
    servers_env: &str,
    auth_token: Option<&str>,
    gateway_relay_dest: Option<&str>,
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
    let mut env = vec![
        format!("AGENTSTACK_RULESET={RULESET_IN}"),
        format!("AGENTSTACK_SERVERS={servers_env}"),
        format!("AGENTSTACK_PROXY_BASE_PORT={PROXY_BASE_PORT}"),
        // D4: this sidecar exists ONLY for a lockdown run (the no-direct-route
        // topology is lockdown by definition), so the proxy's lockdown
        // transport hardening — refuse literal-IP CONNECT targets and non-TLS
        // tunnels — is always on here. Set at this single point so every
        // lockdown entry (run --lockdown and the executor) inherits it, and it
        // never leaks into the host-proxy `--sandbox` mode, which never starts
        // this sidecar.
        "AGENTSTACK_LOCKDOWN=1".to_string(),
    ];
    // Propagate the anti-SSRF opt-out into the sidecar when the host set it to a
    // TRUTHY value (the demo dials the host gateway). A mere-presence check would
    // wrongly forward it for `=0`/`=false`; match the sidecar/CLI truthy parse so
    // an explicit false keeps the address-class check ON.
    if matches!(
        std::env::var("AGENTSTACK_ALLOW_LOCAL_TARGETS")
            .ok()
            .as_deref(),
        Some("1") | Some("true") | Some("yes")
    ) {
        env.push("AGENTSTACK_ALLOW_LOCAL_TARGETS=1".to_string());
    }
    // The peer token the sandbox must present; the proxy binary reads it and
    // rejects any CONNECT that doesn't carry it.
    if let Some(token) = auth_token {
        env.push(format!("AGENTSTACK_PROXY_TOKEN={token}"));
    }
    // Gateway relay (lockdown gateway routing): a fixed-destination splice to
    // the host gateway. The host owns the listen port so its rendered config
    // matches; the sidecar's `extra_hosts` already maps host.docker.internal on
    // the egress leg, so the relay's destination resolves.
    if let Some(dest) = gateway_relay_dest {
        env.push(format!("AGENTSTACK_GATEWAY_RELAY_DEST={dest}"));
        env.push(format!(
            "AGENTSTACK_GATEWAY_RELAY_PORT={GATEWAY_RELAY_PORT}"
        ));
    }
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
    // Raw byte buffer, deliberately not a `String`: log chunks arrive split at
    // arbitrary byte boundaries, so a single multi-byte UTF-8 character (an
    // em-dash in an enforcement reason, say) can straddle two chunks. We
    // accumulate raw bytes and decode only whole lines — see
    // `split_complete_lines`. The old code pushed `b as char` per byte, which
    // reinterprets each byte as a Latin-1 scalar and mangles anything
    // non-ASCII in the recorded evidence.
    let mut buf: Vec<u8> = Vec::new();

    while let Some(next) = logs.next().await {
        let bytes = match next {
            Ok(out) => out.into_bytes(),
            Err(_) => break,
        };
        for line in split_complete_lines(&mut buf, &bytes) {
            handle_line(&line, &sink, &ready_tx, &mut signalled, &mut tail);
        }
    }
    // Flush any unterminated final line (decoded lossily, like the rest), then,
    // if we never saw READY, report the sidecar's tail as the failure reason.
    if !buf.is_empty() {
        let last = String::from_utf8_lossy(&buf).into_owned();
        handle_line(&last, &sink, &ready_tx, &mut signalled, &mut tail);
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

/// Fold a freshly-arrived log chunk into `buf` and return every line that is
/// now complete, each decoded lossily. `buf` carries the bytes left over from
/// earlier chunks (everything after the last newline seen so far); on return it
/// holds the bytes after the last newline in `chunk`, ready for the next call.
///
/// Working in bytes and decoding only whole lines is what keeps multi-byte
/// UTF-8 intact when a character's bytes are split across two chunks: the
/// partial bytes simply wait in `buf` until the rest arrive. Pure (no I/O, no
/// shared state), so it is unit-testable without touching Docker.
fn split_complete_lines(buf: &mut Vec<u8>, chunk: &[u8]) -> Vec<String> {
    buf.extend_from_slice(chunk);
    let mut lines = Vec::new();
    // Drain each newline-terminated run from the front of the buffer, leaving
    // any trailing partial line behind for the next chunk. `drain(..=nl)`
    // removes the line together with its `\n`; `from_utf8_lossy` returns a
    // `Cow<str>` that `into_owned` turns into an owned `String`.
    while let Some(nl) = buf.iter().position(|&b| b == b'\n') {
        let line: Vec<u8> = buf.drain(..=nl).collect();
        lines.push(String::from_utf8_lossy(&line[..line.len() - 1]).into_owned());
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::split_complete_lines;

    #[test]
    fn reassembles_a_multibyte_char_split_across_chunks() {
        // An em-dash (U+2014, bytes E2 80 94) inside an enforcement reason.
        let mut buf = Vec::new();
        let full = "blocked — egress denied\n".as_bytes();
        // Split the chunk *inside* the em-dash's 3 bytes: "blocked " is 8 bytes,
        // so byte 9 lands after the first em-dash byte (E2), leaving 80 94 for
        // the next chunk.
        let (first, second) = full.split_at(9);

        // No newline in the first chunk yet, and the em-dash is still partial —
        // nothing complete to emit.
        assert!(split_complete_lines(&mut buf, first).is_empty());

        // The second chunk completes both the character and the line.
        let lines = split_complete_lines(&mut buf, second);
        assert_eq!(lines, vec!["blocked — egress denied".to_string()]);
        assert!(buf.is_empty(), "buffer fully drained after the newline");
    }

    #[test]
    fn splits_multiple_lines_and_keeps_the_partial_remainder() {
        let mut buf = Vec::new();
        // Two complete lines, then a partial third carrying a multi-byte '…'
        // (U+2026, bytes E2 80 A6) with no trailing newline.
        let lines = split_complete_lines(&mut buf, "READY\n{\"e\":1}\ntail…".as_bytes());
        assert_eq!(lines, vec!["READY".to_string(), "{\"e\":1}".to_string()]);
        // The unterminated final line waits in the buffer, its char intact.
        assert_eq!(String::from_utf8_lossy(&buf), "tail…");
    }
}
