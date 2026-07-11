//! Start one [`ServerProxy`] per MCP server and hand back their addresses —
//! the API the runtime/cli integration calls to stand up a sandbox's egress
//! layer. Per-server identity is per-listener: each server's process inside
//! the container is pointed at its own proxy endpoint, so every outbound
//! connection is attributed to the server that opened it.
//!
//! The bridge itself is fully verifiable on loopback (the multi-server test
//! below proves attribution). Routing a real *container's* traffic to these
//! endpoints — the `HTTPS_PROXY` injection + the sandbox's proxy-only network —
//! is the runtime's remaining Docker-dependent step.

use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;

use agentstack_policy::CompiledRuleset;
use tokio::net::TcpListener;
use tokio::task::JoinHandle;

use crate::decide::EgressGuard;
use crate::proxy::{EventSink, ProxyConfig, ServerProxy};

/// One server's proxy endpoint: point that server's process at `addr`.
#[derive(Debug, Clone)]
pub struct ProxyEndpoint {
    pub server: String,
    pub addr: SocketAddr,
}

/// The live set of per-server egress proxies for one sandboxed run. Dropping
/// it stops every proxy (aborts their accept loops).
pub struct EgressBridge {
    tasks: Vec<JoinHandle<()>>,
}

impl EgressBridge {
    /// Bind and start one proxy per server on loopback ephemeral ports — the
    /// convenience for tests and single-host use.
    pub async fn start(
        servers: &[String],
        ruleset: CompiledRuleset,
        sink: EventSink,
    ) -> io::Result<(EgressBridge, Vec<ProxyEndpoint>)> {
        Self::start_on(IpAddr::V4(Ipv4Addr::LOCALHOST), servers, ruleset, sink).await
    }

    /// Like [`start`](Self::start) but for tests/demos that dial loopback: turns
    /// the anti-SSRF address check off (`allow_local_targets`).
    pub async fn start_allowing_local(
        servers: &[String],
        ruleset: CompiledRuleset,
        sink: EventSink,
    ) -> io::Result<(EgressBridge, Vec<ProxyEndpoint>)> {
        let pairs: Vec<(String, u16)> = servers.iter().map(|s| (s.clone(), 0)).collect();
        Self::start_fixed_with(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            &pairs,
            ruleset,
            sink,
            ProxyConfig {
                allow_local_targets: true,
            },
        )
        .await
    }

    /// Bind and start one proxy per server on `bind` (ephemeral ports), each
    /// filtering for its own server name against the shared compiled ruleset
    /// and reporting to `sink`. Returns the bridge (keep it alive for the run)
    /// and the endpoints to route each server's traffic to. A sandbox binds
    /// `0.0.0.0` so a container can reach the proxies via `host.docker.internal`.
    pub async fn start_on(
        bind: IpAddr,
        servers: &[String],
        ruleset: CompiledRuleset,
        sink: EventSink,
    ) -> io::Result<(EgressBridge, Vec<ProxyEndpoint>)> {
        Self::start_on_with(bind, servers, ruleset, sink, ProxyConfig::default()).await
    }

    /// [`start_on`](Self::start_on) with explicit transport config (the SSRF
    /// address-class check). Production leaves it default (strict).
    pub async fn start_on_with(
        bind: IpAddr,
        servers: &[String],
        ruleset: CompiledRuleset,
        sink: EventSink,
        config: ProxyConfig,
    ) -> io::Result<(EgressBridge, Vec<ProxyEndpoint>)> {
        let pairs: Vec<(String, u16)> = servers.iter().map(|s| (s.clone(), 0)).collect();
        Self::start_fixed_with(bind, &pairs, ruleset, sink, config).await
    }

    /// Bind one proxy per (server, port) on `bind` — the sidecar form. Inside
    /// the proxy *container* the sandbox must know each endpoint a priori
    /// (`alias:port` on the internal network), so ports are caller-chosen
    /// instead of ephemeral. Port 0 still means "any" (used by the loopback
    /// forms above).
    pub async fn start_fixed(
        bind: IpAddr,
        servers: &[(String, u16)],
        ruleset: CompiledRuleset,
        sink: EventSink,
    ) -> io::Result<(EgressBridge, Vec<ProxyEndpoint>)> {
        Self::start_fixed_with(bind, servers, ruleset, sink, ProxyConfig::default()).await
    }

    /// [`start_fixed`](Self::start_fixed) with explicit transport config.
    pub async fn start_fixed_with(
        bind: IpAddr,
        servers: &[(String, u16)],
        ruleset: CompiledRuleset,
        sink: EventSink,
        config: ProxyConfig,
    ) -> io::Result<(EgressBridge, Vec<ProxyEndpoint>)> {
        let mut tasks = Vec::new();
        let mut endpoints = Vec::new();
        for (server, port) in servers {
            let listener = TcpListener::bind((bind, *port)).await?;
            let addr = listener.local_addr()?;
            let proxy = ServerProxy::with_config(
                server.clone(),
                EgressGuard::new(ruleset.clone()),
                Arc::clone(&sink),
                config,
            );
            tasks.push(tokio::spawn(async move {
                let _ = proxy.serve(listener).await;
            }));
            endpoints.push(ProxyEndpoint {
                server: server.clone(),
                addr,
            });
        }
        Ok((EgressBridge { tasks }, endpoints))
    }
}

impl Drop for EgressBridge {
    fn drop(&mut self) {
        for t in &self.tasks {
            t.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstack_recorder::RunEvent;
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpStream;

    async fn echo_server() -> SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut s, _)) = l.accept().await {
                tokio::spawn(async move {
                    let mut b = [0u8; 128];
                    if let Ok(n) = s.read(&mut b).await {
                        let _ = s.write_all(&b[..n]).await;
                    }
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn one_proxy_per_server_attributes_egress_correctly() {
        let echo = echo_server().await;
        let events = Arc::new(Mutex::new(Vec::<RunEvent>::new()));
        let ev = Arc::clone(&events);
        let sink: EventSink = Arc::new(move |e| ev.lock().unwrap().push(e));

        let servers = vec!["alpha".to_string(), "beta".to_string()];
        // Loopback echo target → allow local (the SSRF check would else refuse).
        let (_bridge, endpoints) =
            EgressBridge::start_allowing_local(&servers, CompiledRuleset::default(), sink)
                .await
                .unwrap();
        assert_eq!(endpoints.len(), 2);

        // Drive each server's own proxy.
        for ep in &endpoints {
            let mut c = TcpStream::connect(ep.addr).await.unwrap();
            c.write_all(format!("CONNECT {echo} HTTP/1.1\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut b = [0u8; 128];
            let n = c.read(&mut b).await.unwrap();
            assert!(String::from_utf8_lossy(&b[..n]).contains("200"));
        }

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        // Each server's connection was attributed to that server, not the other.
        let attributed = |name: &str| {
            evs.iter()
                .any(|e| matches!(e, RunEvent::Egress { server, .. } if server == name))
        };
        assert!(attributed("alpha"), "alpha attributed: {evs:?}");
        assert!(attributed("beta"), "beta attributed: {evs:?}");
    }
}
