//! The Docker backend — implements [`Sandbox`] via `bollard`.
//!
//! **Verification status:** this module is compile-verified only. It needs a
//! real Docker daemon to exercise, which [`DockerSandbox::connect`] and the
//! daemon-gated integration test require; where no daemon is available (CI, a
//! dev box without Docker) those paths are skipped. Treat the container
//! configuration here as unverified against a live daemon until that gate has
//! run green somewhere.
//!
//! The [`Sandbox`] trait is synchronous by design (async/tokio proper is the
//! egress crate, 2.2). bollard's Docker API is async, so this backend owns a
//! current-thread tokio runtime and `block_on`s each call — keeping every
//! async detail inside this file.

use std::sync::Arc;

use bollard::models::{ContainerCreateBody, HostConfig};
use bollard::query_parameters::{
    CreateContainerOptionsBuilder, LogsOptionsBuilder, RemoveContainerOptionsBuilder,
    StartContainerOptions, WaitContainerOptions,
};
use bollard::Docker;
use futures_util::StreamExt;
use tokio::runtime::Runtime;

use crate::sandbox::{Exit, Sandbox, SandboxHandle, Stream, StreamChunk};
use crate::spec::{NetworkPolicy, SandboxSpec};
use crate::{Result, RuntimeError};

fn backend<E: std::fmt::Display>(e: E) -> RuntimeError {
    RuntimeError::Backend(e.to_string())
}

/// A Docker-backed sandbox. Holds the connection and the runtime that drives
/// bollard's async calls; the runtime is shared (via `Arc`) with each handle
/// it starts.
pub struct DockerSandbox {
    rt: Arc<Runtime>,
    docker: Docker,
}

impl DockerSandbox {
    /// Connect to the local Docker daemon (socket / named pipe / `DOCKER_HOST`).
    /// Errors when no daemon is reachable — the caller can fall back or refuse
    /// to run in sandbox mode.
    pub fn connect() -> Result<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(backend)?;
        let docker = Docker::connect_with_local_defaults().map_err(backend)?;
        // A cheap round-trip so "connected" means the daemon actually answered,
        // not just that a socket path exists.
        rt.block_on(docker.ping()).map_err(backend)?;
        Ok(DockerSandbox {
            rt: Arc::new(rt),
            docker,
        })
    }
}

impl Sandbox for DockerSandbox {
    fn start(&self, spec: &SandboxSpec) -> Result<Box<dyn SandboxHandle>> {
        let binds: Vec<String> = spec
            .mounts
            .iter()
            .map(|m| {
                let ro = if m.read_only { ":ro" } else { "" };
                format!("{}:{}{}", m.host, m.container, ro)
            })
            .collect();

        // Phase 2's core guarantee: no direct network. `none` gives the
        // container no interface at all; `ProxyOnly` (once the egress crate
        // exists) will instead attach an internal network whose only route is
        // the proxy. Until then a proxy endpoint is treated as still-no-network
        // rather than silently granting open egress.
        let network_mode = match &spec.network {
            NetworkPolicy::None => Some("none".to_string()),
            NetworkPolicy::ProxyOnly { .. } => Some("none".to_string()),
        };

        let env: Vec<String> = spec.env.iter().map(|(k, v)| format!("{k}={v}")).collect();

        let host_config = HostConfig {
            binds: Some(binds),
            network_mode,
            ..Default::default()
        };
        let body = ContainerCreateBody {
            image: Some(spec.image.clone()),
            cmd: Some(spec.command.clone()),
            working_dir: Some(spec.workdir.clone()),
            env: Some(env),
            host_config: Some(host_config),
            ..Default::default()
        };

        let docker = self.docker.clone();
        let rt = Arc::clone(&self.rt);
        let id = rt.block_on(async {
            let created = docker
                .create_container(Some(CreateContainerOptionsBuilder::default().build()), body)
                .await
                .map_err(backend)?;
            docker
                .start_container(&created.id, None::<StartContainerOptions>)
                .await
                .map_err(backend)?;
            Ok::<String, RuntimeError>(created.id)
        })?;

        Ok(Box::new(DockerHandle { rt, docker, id }))
    }
}

struct DockerHandle {
    rt: Arc<Runtime>,
    docker: Docker,
    id: String,
}

impl SandboxHandle for DockerHandle {
    fn wait_streaming(&mut self, on_output: &mut dyn FnMut(StreamChunk)) -> Result<Exit> {
        let docker = self.docker.clone();
        let id = self.id.clone();
        self.rt.block_on(async move {
            // Follow the container's logs (both streams) until it exits.
            let opts = LogsOptionsBuilder::default()
                .follow(true)
                .stdout(true)
                .stderr(true)
                .build();
            let mut logs = docker.logs(&id, Some(opts));
            while let Some(next) = logs.next().await {
                let out = next.map_err(backend)?;
                let (stream, bytes) = classify(out);
                if !bytes.is_empty() {
                    on_output(StreamChunk { stream, bytes });
                }
            }

            // The log stream ends at exit; read the final status code.
            let mut waits = docker.wait_container(&id, None::<WaitContainerOptions>);
            let mut code = None;
            while let Some(next) = waits.next().await {
                // A non-zero exit is reported by bollard as an error carrying
                // the status; treat both as "the container is done".
                if let Ok(resp) = next {
                    code = Some(resp.status_code as i32);
                }
            }
            Ok(Exit { code })
        })
    }

    fn teardown(&mut self) -> Result<()> {
        let docker = self.docker.clone();
        let id = self.id.clone();
        self.rt
            .block_on(async move {
                let opts = RemoveContainerOptionsBuilder::default().force(true).build();
                docker.remove_container(&id, Some(opts)).await
            })
            .map_err(|e| RuntimeError::Teardown(e.to_string()))
    }
}

/// Split a bollard log frame into our stream tag + bytes.
fn classify(out: bollard::container::LogOutput) -> (Stream, Vec<u8>) {
    use bollard::container::LogOutput as L;
    match out {
        L::StdErr { message } => (Stream::Stderr, message.to_vec()),
        L::StdOut { message } => (Stream::Stdout, message.to_vec()),
        L::Console { message } | L::StdIn { message } => (Stream::Stdout, message.to_vec()),
    }
}
