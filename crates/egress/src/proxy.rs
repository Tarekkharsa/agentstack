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
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use agentstack_recorder::RunEvent;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;

use crate::decide::{guard_block_event, EgressGuard};
use crate::netguard::is_forbidden_target;
use crate::sni::extract_sni;

/// Where decision events go. The runtime wraps `RunLog::append`; tests collect
/// them. Called from async tasks, so it is `Send + Sync`.
pub type EventSink = Arc<dyn Fn(RunEvent) + Send + Sync>;

/// Cap on the CONNECT request head we buffer before giving up — hostile input
/// from inside a container must not let a client make us allocate without
/// bound.
const MAX_HEAD: usize = 8 * 1024;

/// Cap on the client's first post-CONNECT flight (the TLS ClientHello) we read
/// to check SNI. A TLS record maxes at 16 KiB + 5 header bytes; past that we
/// stop reading and proceed without an SNI assertion rather than buffer forever.
const MAX_HELLO: usize = 16 * 1024 + 5;

/// Deadline for each blocking network step (reading the CONNECT head, reading
/// the ClientHello, resolving, dialing). Bounds slowloris-style clients that
/// open a tunnel and then dribble bytes to pin a task indefinitely.
const STEP_TIMEOUT: Duration = Duration::from_secs(15);

/// Transport-level knobs for a proxy, separate from the policy the guard holds.
#[derive(Debug, Clone, Default)]
pub struct ProxyConfig {
    /// Permit resolved targets in loopback / private / link-local ranges. Only
    /// for tests and the Docker demo, which dial the host gateway on purpose.
    /// NEVER set in production: it disables the anti-SSRF address-class check.
    pub allow_local_targets: bool,
    /// A shared secret the sandbox must present as HTTP Basic proxy credentials
    /// (`Proxy-Authorization`). The listener necessarily binds a broad address
    /// so a container can reach it, so this — not the bind address — is what
    /// authenticates the peer: a LAN neighbor without the per-run token gets a
    /// 407, and every allowed connection is provably the sandbox's. `None`
    /// disables the check (unit tests / fixtures that drive the proxy directly).
    pub auth_token: Option<String>,
    /// Lockdown transport hardening (D4): when set, refuse a literal-IP CONNECT
    /// target and a non-TLS first flight. The gateway-only fence is by
    /// hostname, so an agent could otherwise dodge it by dialing an upstream's
    /// IP directly, or by opening a plaintext tunnel the SNI check never
    /// inspects. Driven by the explicit lockdown flag — NOT inferred from a
    /// nonempty gateway-only set, since an all-stdio lockdown run has an empty
    /// set yet still needs these restrictions. Off for `--sandbox` (proxy-only)
    /// and dev/tests.
    pub lockdown: bool,
}

/// A forward proxy dedicated to one MCP server.
pub struct ServerProxy {
    server: String,
    guard: Arc<EgressGuard>,
    on_event: EventSink,
    config: ProxyConfig,
}

impl ServerProxy {
    /// Strict proxy: resolved targets must be global unicast (anti-SSRF on).
    pub fn new(server: impl Into<String>, guard: EgressGuard, on_event: EventSink) -> Arc<Self> {
        Self::with_config(server, guard, on_event, ProxyConfig::default())
    }

    pub fn with_config(
        server: impl Into<String>,
        guard: EgressGuard,
        on_event: EventSink,
        config: ProxyConfig,
    ) -> Arc<Self> {
        Arc::new(ServerProxy {
            server: server.into(),
            guard: Arc::new(guard),
            on_event,
            config,
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
        let head = match timeout(STEP_TIMEOUT, read_head(&mut client)).await {
            Ok(Some(h)) => h,
            _ => {
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

        // Peer authentication: the listener binds a broad address so a container
        // can reach it, so the token — not the bind — is what proves this client
        // is the sandbox. Without a valid `Proxy-Authorization`, refuse (407)
        // before any policy/resolve/dial happens, and record the rejection.
        if let Some(token) = self.config.auth_token.as_deref() {
            if !head_has_valid_auth(&head, token) {
                (self.on_event)(guard_block_event(
                    &self.server,
                    &target.host,
                    "proxy client did not present the expected credentials".to_string(),
                ));
                let _ = client
                    .write_all(
                        b"HTTP/1.1 407 Proxy Authentication Required\r\n\
                          Proxy-Authenticate: Basic realm=\"agentstack\"\r\n\r\n",
                    )
                    .await;
                return Ok(());
            }
        }

        // Lockdown transport guard (D4): refuse a literal-IP CONNECT target.
        // The gateway-only fence matches by hostname, so an agent that dialed a
        // declared upstream by its IP would never hit the set; under lockdown
        // all legitimate egress is to named hosts (the gateway relay is a
        // separate listener), so a literal IP is refused outright. Detection is
        // LEXICAL and fail-closed (`is_numeric_host`): Rust's `IpAddr` parse only
        // catches the canonical dotted-decimal / colon-hex forms, but the
        // platform resolver also accepts octal, hex, shortened, and
        // single-integer encodings — every one of which is a numeric bypass of
        // the hostname fence. We do NOT resolve DNS to decide (that would invite
        // rebinding / TOCTOU). Gated on the explicit lockdown flag, never
        // inferred from the host set — an all-stdio run has an empty set but
        // still needs this. After the auth check so an unauthenticated client
        // still gets the 407 first.
        if self.config.lockdown && crate::connect::is_numeric_host(&target.host) {
            (self.on_event)(guard_block_event(
                &self.server,
                &target.host,
                "literal-IP (numeric-host) CONNECT target refused under lockdown — \
                 reach declared hosts by name through the gateway"
                    .to_string(),
            ));
            let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
            return Ok(());
        }

        // Policy layer: does the machine/bundle ruleset allow this host? Hold
        // the decision — we emit exactly one event reflecting the FINAL outcome,
        // so a policy-allow that a transport guard later refuses is recorded as
        // the block it actually was, not a misleading allow.
        let decision = self.guard.decide(&self.server, &target.host, target.port);
        if !decision.allowed {
            (self.on_event)(decision.event);
            let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
            return Ok(());
        }

        // Anti-SSRF: resolve the (allowed) name ONCE and require every resolved
        // address to be global unicast, then dial a validated address directly
        // (never a second implicit resolution that could return a different,
        // forbidden answer — closing the DNS-rebinding window). A literal-IP
        // CONNECT resolves to itself and is judged the same way.
        let addr = match self.resolve_validated(&target.host, target.port).await {
            Ok(a) => a,
            Err(reason) => {
                (self.on_event)(guard_block_event(&self.server, &target.host, reason));
                let _ = client.write_all(b"HTTP/1.1 403 Forbidden\r\n\r\n").await;
                return Ok(());
            }
        };

        let mut upstream = match timeout(STEP_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(u)) => u,
            _ => {
                (self.on_event)(decision.event);
                let _ = client.write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n").await;
                return Ok(());
            }
        };
        client
            .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            .await?;

        // Domain-fronting guard: the client now speaks TLS. Peek its first
        // flight and, if it carries an SNI, require it to match the host it
        // CONNECTed to — otherwise a client could tunnel to an allowed front and
        // then ask (via SNI) for a denied host behind it.
        let flight = match read_client_hello(&mut client).await? {
            // Not TLS — no SNI concept, and we already dialed the validated
            // CONNECT host. Forward whatever it sent and splice.
            FirstFlight::NonTls(bytes) => {
                // Lockdown transport guard (D4): a non-TLS tunnel carries no SNI
                // for the domain-fronting check to verify and could ferry
                // arbitrary plaintext to a permitted host. Under lockdown all
                // egress must be TLS, so refuse it. (Off elsewhere: sandbox/dev
                // may legitimately speak plaintext.) This belongs here in
                // first-flight, not in the policy decision — TLS-vs-not is known
                // only after the CONNECT succeeds and the client speaks.
                if self.config.lockdown {
                    (self.on_event)(guard_block_event(
                        &self.server,
                        &target.host,
                        "non-TLS tunnel refused under lockdown — only TLS egress is \
                         permitted"
                            .to_string(),
                    ));
                    return Ok(()); // tunnel established; drop it
                }
                bytes
            }
            // A complete ClientHello: assert SNI == CONNECT host (absent SNI is
            // fine — the client isn't claiming a different host).
            FirstFlight::Tls(bytes) => {
                if let Some(sni) = extract_sni(&bytes) {
                    let sni = crate::connect::normalize_host(&sni);
                    if sni != target.host {
                        (self.on_event)(guard_block_event(
                            &self.server,
                            &target.host,
                            format!(
                                "TLS SNI '{sni}' does not match CONNECT host '{}'",
                                target.host
                            ),
                        ));
                        return Ok(()); // tunnel established; drop it
                    }
                }
                bytes
            }
            // TLS-looking but the ClientHello never completed (stall past the
            // timeout, oversize, or EOF). Fail CLOSED: a partial hello is a way
            // to slip a mismatched SNI in AFTER the splice starts, dodging the
            // check above.
            FirstFlight::Incomplete => {
                (self.on_event)(guard_block_event(
                    &self.server,
                    &target.host,
                    "incomplete TLS ClientHello — cannot verify SNI, failing closed".to_string(),
                ));
                return Ok(());
            }
        };

        // Final outcome is allow: record it, replay the buffered first flight to
        // upstream, then splice the sockets.
        (self.on_event)(decision.event);
        upstream.write_all(&flight).await?;
        tokio::io::copy_bidirectional(&mut client, &mut upstream)
            .await
            .map(|_| ())
    }

    /// Resolve `host:port` once and return the first global-unicast address, or
    /// an `Err(reason)` describing why every candidate was refused. With
    /// `allow_local_targets` the address-class check is skipped (tests/demo).
    async fn resolve_validated(
        &self,
        host: &str,
        port: u16,
    ) -> Result<std::net::SocketAddr, String> {
        let addrs = match timeout(STEP_TIMEOUT, tokio::net::lookup_host((host, port))).await {
            Ok(Ok(it)) => it.collect::<Vec<_>>(),
            Ok(Err(e)) => return Err(format!("could not resolve '{host}': {e}")),
            Err(_) => return Err(format!("resolving '{host}' timed out")),
        };
        if addrs.is_empty() {
            return Err(format!("'{host}' resolved to no addresses"));
        }
        if self.config.allow_local_targets {
            return Ok(addrs[0]);
        }
        // Pick the first safe address; refuse if the name resolves ONLY to
        // forbidden ranges (loopback/private/link-local/metadata) — the SSRF
        // pivot a locked-down sandbox must not get.
        match addrs.iter().find(|a| !is_forbidden_target(a.ip())) {
            Some(a) => Ok(*a),
            None => Err(format!(
                "'{host}' resolves only to non-global addresses ({}) — refused (SSRF guard)",
                forbidden_summary(&addrs),
            )),
        }
    }
}

/// A short, bounded description of the forbidden addresses a name resolved to,
/// for the audit record. Caps the list so a hostile name with many answers
/// can't bloat the log line.
fn forbidden_summary(addrs: &[std::net::SocketAddr]) -> String {
    addrs
        .iter()
        .map(|a| a.ip())
        .take(3)
        .map(|ip: IpAddr| ip.to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Does the CONNECT head carry the expected Basic proxy credentials? The client
/// (curl et al.) turns a `http://agentstack:<token>@host` proxy URL into a
/// `Proxy-Authorization: Basic base64("agentstack:<token>")` header on CONNECT.
fn head_has_valid_auth(head: &[u8], token: &str) -> bool {
    let expected = format!(
        "Basic {}",
        base64_encode(format!("agentstack:{token}").as_bytes())
    );
    let text = String::from_utf8_lossy(head);
    text.lines().any(|line| {
        line.split_once(':').is_some_and(|(name, value)| {
            name.trim().eq_ignore_ascii_case("proxy-authorization") && value.trim() == expected
        })
    })
}

/// Standard base64 (RFC 4648, `+/`, `=` padding). Hand-rolled so the egress
/// crate keeps its minimal dependency set — the input here is a short token.
fn base64_encode(input: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
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

/// The client's first flight after CONNECT, classified so the SNI check can
/// fail closed on an unverifiable TLS handshake instead of waving it through.
enum FirstFlight {
    /// Not a TLS handshake (first byte ≠ 0x16) or nothing sent — no SNI to
    /// assert. Carries the bytes to replay upstream.
    NonTls(Vec<u8>),
    /// A complete TLS ClientHello record. Carries the bytes to replay upstream.
    Tls(Vec<u8>),
    /// A TLS-looking flight that never completed (timeout / oversize / EOF
    /// mid-handshake). The caller must refuse it.
    Incomplete,
}

/// Read the client's first flight after CONNECT — enough to classify it and, if
/// TLS, parse the ClientHello's SNI. Reads whole TLS records up to
/// [`MAX_HELLO`], returning as soon as it can decide. Bounded and time-limited
/// so a client can't stall the tunnel here.
async fn read_client_hello(client: &mut TcpStream) -> io::Result<FirstFlight> {
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 4096];
    loop {
        let n = match timeout(STEP_TIMEOUT, client.read(&mut chunk)).await {
            Ok(Ok(n)) => n,
            // Timeout or read error before a complete hello.
            _ => return Ok(classify_incomplete(buf)),
        };
        if n == 0 {
            // EOF before a complete hello.
            return Ok(classify_incomplete(buf));
        }
        buf.extend_from_slice(&chunk[..n]);
        // A non-handshake first byte means this isn't TLS at all.
        if buf[0] != 0x16 {
            return Ok(FirstFlight::NonTls(buf));
        }
        // Once the first TLS record is complete, we have the ClientHello.
        if buf.len() >= 5 {
            let record_len = ((buf[3] as usize) << 8) | buf[4] as usize;
            if buf.len() >= 5 + record_len {
                return Ok(FirstFlight::Tls(buf));
            }
        }
        // TLS-looking but growing past the cap without completing → refuse.
        if buf.len() >= MAX_HELLO {
            return Ok(FirstFlight::Incomplete);
        }
    }
}

/// Classify a first flight that ended (EOF/timeout) without a complete TLS
/// record. Empty or non-TLS bytes carry no SNI to bypass, so they pass; a
/// partial TLS handshake fails closed — a stalled ClientHello is exactly how a
/// client would try to smuggle a mismatched SNI in after the splice begins.
fn classify_incomplete(buf: Vec<u8>) -> FirstFlight {
    if buf.is_empty() || buf[0] != 0x16 {
        FirstFlight::NonTls(buf)
    } else {
        FirstFlight::Incomplete
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
    /// shared event log. Loopback targets → `allow_local_targets` on, else the
    /// anti-SSRF check would refuse the test's own 127.0.0.1 echo server.
    async fn start_proxy(rs: CompiledRuleset) -> (std::net::SocketAddr, Arc<Mutex<Vec<RunEvent>>>) {
        start_proxy_cfg(
            rs,
            ProxyConfig {
                allow_local_targets: true,
                ..ProxyConfig::default()
            },
        )
        .await
    }

    async fn start_proxy_cfg(
        rs: CompiledRuleset,
        cfg: ProxyConfig,
    ) -> (std::net::SocketAddr, Arc<Mutex<Vec<RunEvent>>>) {
        let events = Arc::new(Mutex::new(Vec::new()));
        let ev = Arc::clone(&events);
        let proxy = ServerProxy::with_config(
            "web-search",
            EgressGuard::new(rs),
            Arc::new(move |e| ev.lock().unwrap().push(e)),
            cfg,
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

    /// Policy allows the name, but it resolves to a forbidden (loopback)
    /// address — the anti-SSRF guard must refuse and record the block. This is
    /// the SSRF/pivot case a locked-down sandbox must not achieve.
    #[tokio::test]
    async fn resolved_forbidden_address_is_blocked_by_ssrf_guard() {
        // Strict proxy (default): local targets NOT allowed.
        let (paddr, events) =
            start_proxy_cfg(CompiledRuleset::default(), ProxyConfig::default()).await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        // "localhost" is allowed by (empty) policy but resolves to 127.0.0.1/::1.
        c.write_all(b"CONNECT localhost:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        assert!(
            read_some(&mut c).await.contains("403"),
            "SSRF target refused"
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| match e {
                RunEvent::Egress { allowed, rule, .. } =>
                    !allowed && rule.as_deref().is_some_and(|r| r.contains("SSRF")),
                _ => false,
            }),
            "SSRF block recorded with reason: {evs:?}"
        );
    }

    /// A tunnel opened to an allowed host, then a TLS ClientHello whose SNI
    /// names a DIFFERENT host, must be refused (domain-fronting guard).
    #[tokio::test]
    async fn sni_not_matching_connect_host_is_blocked() {
        let echo = echo_server().await;
        let (paddr, events) = start_proxy(CompiledRuleset::default()).await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        c.write_all(format!("CONNECT {echo} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(read_some(&mut c).await.contains("200"), "tunnel opened");

        // Speak TLS with an SNI for a host we did NOT CONNECT to.
        let hello = crate::sni::client_hello_with_sni("evil.example");
        c.write_all(&hello).await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| match e {
                RunEvent::Egress { allowed, rule, .. } =>
                    !allowed && rule.as_deref().is_some_and(|r| r.contains("SNI")),
                _ => false,
            }),
            "SNI mismatch block recorded: {evs:?}"
        );
    }

    /// A partial TLS ClientHello (the record header claims more bytes than the
    /// client sends) must be refused, not waved through — otherwise a client
    /// could stall, get spliced, and then send a mismatched SNI. Fail closed.
    #[tokio::test]
    async fn incomplete_tls_client_hello_is_refused() {
        let echo = echo_server().await;
        let (paddr, events) = start_proxy(CompiledRuleset::default()).await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        c.write_all(format!("CONNECT {echo} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(read_some(&mut c).await.contains("200"), "tunnel opened");

        // TLS record header (0x16) declaring a 300-byte record, but we send only
        // a handful of bytes and then close the write half — the hello never
        // completes, so the proxy hits EOF mid-handshake.
        c.write_all(&[0x16, 0x03, 0x01, 0x01, 0x2c, 0x01, 0x00, 0x00])
            .await
            .unwrap();
        c.shutdown().await.unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| match e {
                RunEvent::Egress { allowed, rule, .. } =>
                    !allowed && rule.as_deref().is_some_and(|r| r.contains("incomplete")),
                _ => false,
            }),
            "incomplete ClientHello blocked and recorded: {evs:?}"
        );
    }

    /// With an auth token configured, a CONNECT lacking the credentials is
    /// refused (407) and recorded; one presenting them tunnels.
    #[tokio::test]
    async fn proxy_auth_token_is_required_when_set() {
        let echo = echo_server().await;
        let (paddr, events) = start_proxy_cfg(
            CompiledRuleset::default(),
            ProxyConfig {
                allow_local_targets: true,
                auth_token: Some("s3cr3t-token".to_string()),
                ..ProxyConfig::default()
            },
        )
        .await;

        // No credentials → 407.
        let mut c = TcpStream::connect(paddr).await.unwrap();
        c.write_all(format!("CONNECT {echo} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(
            read_some(&mut c).await.contains("407"),
            "unauth CONNECT refused"
        );

        // Correct Basic credentials → tunnel opens.
        let creds = base64_encode(b"agentstack:s3cr3t-token");
        let mut c2 = TcpStream::connect(paddr).await.unwrap();
        c2.write_all(
            format!("CONNECT {echo} HTTP/1.1\r\nProxy-Authorization: Basic {creds}\r\n\r\n")
                .as_bytes(),
        )
        .await
        .unwrap();
        assert!(
            read_some(&mut c2).await.contains("200"),
            "authed CONNECT tunnels"
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                RunEvent::Egress { allowed: false, rule: Some(r), .. } if r.contains("credentials")
            )),
            "the 407 rejection was recorded: {evs:?}"
        );
    }

    #[test]
    fn base64_matches_known_vectors() {
        // RFC 4648 test vectors — proves the hand-rolled encoder is correct.
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    /// A matching SNI passes the guard and the tunnel carries bytes.
    #[tokio::test]
    async fn sni_matching_connect_host_is_allowed() {
        let echo = echo_server().await;
        let (paddr, _events) = start_proxy(CompiledRuleset::default()).await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        // CONNECT to the echo by an IP:port authority; SNI must equal that host.
        c.write_all(format!("CONNECT {echo} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(read_some(&mut c).await.contains("200"));

        let host = echo.ip().to_string();
        let hello = crate::sni::client_hello_with_sni(&host);
        c.write_all(&hello).await.unwrap();
        // The echo server bounces the ClientHello bytes back through the tunnel.
        let mut b = [0u8; 64];
        let n = c.read(&mut b).await.unwrap();
        assert!(n > 0, "matching-SNI tunnel forwards bytes");
    }

    /// Under lockdown, a literal-IP CONNECT target is refused (403) and
    /// recorded — the gateway-only fence matches by hostname, so dialing an
    /// upstream by its IP must not be a way around it. A GLOBAL-unicast IP is
    /// used so the block is provably the lockdown guard, not the SSRF check.
    #[tokio::test]
    async fn lockdown_refuses_literal_ip_connect() {
        let (paddr, events) = start_proxy_cfg(
            CompiledRuleset::default(),
            ProxyConfig {
                lockdown: true,
                ..ProxyConfig::default()
            },
        )
        .await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        // Global-unicast IP literal — refused before any resolve/dial happens.
        c.write_all(b"CONNECT 93.184.216.34:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        assert!(
            read_some(&mut c).await.contains("403"),
            "literal-IP CONNECT refused under lockdown"
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                RunEvent::Egress { allowed: false, rule: Some(r), .. } if r.contains("literal-IP")
            )),
            "literal-IP block recorded: {evs:?}"
        );
    }

    /// Under lockdown, a NON-canonical numeric host — a single-integer IPv4
    /// (`2130706433` = 127.0.0.1) that Rust's `IpAddr::parse` rejects but the
    /// platform resolver accepts — is still refused (403) and recorded. This is
    /// the bypass the lexical `is_numeric_host` guard closes over a bare
    /// `parse::<IpAddr>()`; refused BEFORE any resolve/dial, so no rebinding
    /// window opens.
    #[tokio::test]
    async fn lockdown_refuses_non_canonical_numeric_host() {
        let (paddr, events) = start_proxy_cfg(
            CompiledRuleset::default(),
            ProxyConfig {
                lockdown: true,
                ..ProxyConfig::default()
            },
        )
        .await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        // Single-integer IPv4 literal — not a canonical IpAddr, but inet_aton
        // would resolve it to 127.0.0.1. Must be refused lexically.
        c.write_all(b"CONNECT 2130706433:443 HTTP/1.1\r\n\r\n")
            .await
            .unwrap();
        assert!(
            read_some(&mut c).await.contains("403"),
            "non-canonical numeric CONNECT refused under lockdown"
        );

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                RunEvent::Egress { allowed: false, rule: Some(r), .. } if r.contains("numeric-host")
            )),
            "numeric-host block recorded: {evs:?}"
        );
    }

    /// Under lockdown, a non-TLS first flight is refused (the tunnel opens, then
    /// is dropped) and recorded — a plaintext tunnel carries no SNI for the
    /// domain-fronting check. CONNECT by hostname so the literal-IP guard above
    /// doesn't fire first.
    #[tokio::test]
    async fn lockdown_refuses_non_tls_first_flight() {
        // Bind the upstream on whatever "localhost" resolves to FIRST, so the
        // proxy's own resolve_validated (which also takes the first address)
        // dials a listener that actually exists — avoids an IPv4/IPv6 family
        // mismatch that would 502 before the non-TLS check is reached.
        let first_ip = tokio::net::lookup_host(("localhost", 0))
            .await
            .unwrap()
            .next()
            .unwrap()
            .ip();
        let upstream = TcpListener::bind((first_ip, 0)).await.unwrap();
        let uport = upstream.local_addr().unwrap().port();
        tokio::spawn(async move {
            while let Ok((_s, _)) = upstream.accept().await { /* hold the socket open */ }
        });
        let (paddr, events) = start_proxy_cfg(
            CompiledRuleset::default(),
            ProxyConfig {
                allow_local_targets: true,
                lockdown: true,
                ..ProxyConfig::default()
            },
        )
        .await;

        let mut c = TcpStream::connect(paddr).await.unwrap();
        // "localhost" is a name (not an IP literal), so the literal-IP guard
        // doesn't fire; it resolves to the loopback upstream above.
        c.write_all(format!("CONNECT localhost:{uport} HTTP/1.1\r\n\r\n").as_bytes())
            .await
            .unwrap();
        assert!(read_some(&mut c).await.contains("200"), "tunnel opens");

        // Speak plaintext HTTP, not TLS (first byte != 0x16).
        c.write_all(b"GET / HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let evs = events.lock().unwrap();
        assert!(
            evs.iter().any(|e| matches!(
                e,
                RunEvent::Egress { allowed: false, rule: Some(r), .. } if r.contains("non-TLS")
            )),
            "non-TLS block recorded: {evs:?}"
        );
    }
}
