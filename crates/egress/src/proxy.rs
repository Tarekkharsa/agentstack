//! The async forward-proxy server — the transport that applies the
//! [`EgressGuard`](crate::decide::EgressGuard) to real connections.
//!
//! Model: one [`ServerProxy`] per MCP server (per-server identity =
//! per-listener). The runtime configures each server's process inside the
//! sandbox to use its own proxy listener (`HTTPS_PROXY`), so every outbound
//! connection is attributed to the server that opened it. A client opens a
//! tunnel with `CONNECT host:port`; the proxy consults the guard for
//! (server, host); on allow it dials the target and copies bytes both ways; on
//! block it refuses the tunnel. Every decision is emitted to the recorder.
//!
//! DNS: the container has no direct network, so it cannot resolve names
//! itself — it sends the hostname in the CONNECT line and the proxy resolves
//! it (via `TcpStream::connect`) ONLY for allowed hosts. So name resolution is
//! implicitly gated by the same allowlist, closing DNS as an exfil channel
//! without a separate resolver.
//!
//! This is testable end to end on loopback (see the tests) — no Docker.

use std::io;
use std::sync::Arc;

use agentstack_recorder::RunEvent;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::decide::EgressGuard;

/// Where decision events go. The runtime wraps `RunLog::append`; tests collect
/// them. Called from async tasks, so it is `Send + Sync`.
pub type EventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;

/// Cap on the CONNECT request head we buffer before giving up — hostile input
/// from inside a container must not let a client make us allocate without
/// bound.
const MAX_HEAD: usize = 8 * 1024;

/// A forward proxy dedicated to one MCP server.
pub struct ServerProxy {
    server: String,
    guard: Arc<EgressGuard>,
    on_event: EventSink,
}

impl ServerProxy {
    pub fn new(server: impl Into<String>, guard: EgressGuard, on_event: EventSink) -> Arc<Self> {
        Arc::new(ServerProxy {
            server: server.into(),
            guard: Arc::new(guard),
            on_event,
        })
    }

    /// Accept forever, handling each connection on its own task. Returns only
    /// on an accept error (the listener closing).
    pub async fn serve(self: Arc<Self>, listener: TcpListener) -> io::Result<()> {
        loop {
            let (client, _) = listener.accept().await?;
            let me = Arc::clone(&self);
            tokio::spawn(async move {
                // A per-connection error is logged by the client's failed
                // request, never fatal to the proxy.
                let _ = me.handle(client).await;
            });
        }
    }

    async fn handle(&self, mut client: TcpStream) -> io::Result<()> {
        let head = match read_head(&mut client).await {
            Some(h) => h,
            None => {
                let _ = client.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
                return Ok(());
            }
        };
        let target = match crate::connect::parse_connect_target(&head) {
            Some(t) => t,
            None => {
                let _ = client.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n").await;
                return Ok(());
            }
        };

        // The one enforcement point: consult the guard, record the decision.
        let decision = self.guard.decide(&self.server, &target.host);
        (self.on_event)(decision.event);
        if !decision.allowed {
            let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
            return Ok(());
        }

        // Allowed: dial the target (name resolution happens here, only for an
        // allowed host) and splice the two sockets together.
        let mut upstream = match TcpStream::connect((target.host.as_str(), target.port)).await {
            Ok(u) => u,
            Err(_) => {
                let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                return Ok(());
            }
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;
        tokio::io::copy_bidirectional(&mut client, &mut upstream)
            .await
            .map(|_| ())
    }
}

/// Read the request head up to the blank line (`\r\n\r\n`), bounded by
/// [`MAX_HEAD`]. `None` on EOF-before-terminator or overrun.
async fn read_head(client: &mut TcpStream) -> Option<Vec<u8>> {
    let mut buf = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        let n = client.read(&mut byte).await.ok()?;
        if n == 0 {
            return None; // EOF before we saw the terminator
        }
        buf.push(byte[0]);
        if buf.ends_with(b"\r\n\r\n") {
            return Some(buf);
        }
        if buf.len() >= MAX_HEAD {
            return None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentstack_core::manifest::Policy;
    use agentstack_policy::CompiledRuleset;
    use std::sync::Mutex;

    fn ruleset(entries: &[(&str, &[&str])]) -> CompiledRuleset {
        let mut m = Policy::default();
        for (k, pats) in entries {
            m.egress
                .insert(k.to_string(), pats.iter().map(|s| s.to_string()).collect());
        }
        agentstack_policy::compile(&m, &Policy::default(), &["web-search"])
    }

    /// A loopback echo server; returns its address.
    async fn echo_server() -> std::net::SocketAddr {
        let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = l.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut s, _)) = l.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut b = [0u8; 256];
                    if let Ok(n) = s.read(&mut b).await {
                        let _ = s.write_all(&b[..n]).await;
                    }
                });
            }
        });
        addr
    }

    /// Start a ServerProxy on a fresh loopback port; return its addr + the
    /// shared event log.
    async fn start_proxy(rs: CompiledRuleset) -> (std::net::SocketAddr, Arc<Mutex<Vec<RunEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = Arc::clone(&events);
        let proxy = ServerProxy::new(
            "web-search",
            EgressGuard::new(rs),
            Arc::new(move |e| ev.lock().unwrap().push(e)),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = proxy.serve(listener).await;
        });
        (addr, events)
    }

    async fn read_some(s: &mut TcpStream) -> String {
        let mut b = [0u8; 256];
        let n = s.read(&mut b).await.unwrap();
        String::from_utf8_lossy(&b[..n]).into_owned()
    }

    #[tokio::test]
    async fn allowed_connect_tunnels_bytes_and_records_allow() {
        let echo = echo_server().await;
        // Default ruleset → allow-by-default.
        let (paddr, events) = start_proxy(CompiledRuleset::default()).await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        c.write_all(format!("CONNECT {echo} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(
            read_some(&mut c).await.contains("200"),
            "tunnel established"
        );

        c.write_all(b"ping").await.unwrap();
        assert_eq!(
            read_some(&mut c).await,
            "ping",
            "bytes flow through the tunnel"
        );

        // Let the event task run.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter()
                .any(|e| matches!(e, RunEvent::Egress { allowed: true, .. })),
            "allow was recorded: {evs:?}"
        );
    }

    #[tokio::test]
    async fn denied_connect_is_refused_and_recorded() {
        let echo = echo_server().await;
        // Deny the echo host (loopback).
        let (paddr, events) = start_proxy(ruleset(&[("*", &["!127.0.0.1"])])).await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        c.write_all(format!("CONNECT {echo} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(read_some(&mut c).await.contains("403"), "tunnel refused");

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| match e {
                RunEvent::Egress { allowed, rule, .. } => !allowed && rule.is_some(),
                _ => false,
            }),
            "block + rule recorded: {evs:?}"
        );
    }

    #[tokio::test]
    async fn a_non_connect_request_is_rejected() {
        let (paddr, _events) = start_proxy(CompiledRuleset::default()).await;
        let mut c = TcpStream::connect(paddr).await.unwrap();
        c.write_all(b"GET / HTTP/1.1\r\n\r\n").await.unwrap();
        assert!(read_some(&mut c).await.contains("400"));
    }
}
