//! Authenticated, exact-grant relay for hostile executor runtimes.
//!
//! The executor container has no direct network route. Its runtime sends one
//! bounded JSON object per line through the lockdown sidecar's fixed relay to
//! this listener. Authentication establishes the execution identity; the
//! immutable exact grant authorizes each tool; the CLI-owned callback then
//! delegates to the existing gateway for policy enforcement and audit.

use std::collections::BTreeSet;
use std::io;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

use serde::Deserialize;
use serde_json::{json, Map, Value};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_CONNECTIONS: usize = 8;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RelayRequest {
    id: Value,
    token: String,
    tool: String,
    arguments: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelayCallError {
    PolicyDenied,
    Unavailable,
    Failed,
}

pub type ExecutionCall = Arc<dyn Fn(&str, Value) -> Result<Value, RelayCallError> + Send + Sync>;

/// The one opt-in that lets an operator name the host address every
/// container-reachable listener binds — the per-execution tool-call relay, the
/// run's MCP gateway endpoint, and the `--sandbox` egress proxy. It accepts a
/// bare IP address; `AGENTSTACK_RELAY_BIND=0.0.0.0` is the deliberate,
/// LAN-reachable wildcard choice that used to be taken silently.
///
/// One mechanism for all four bind paths, so "when may this listener be
/// LAN-reachable" cannot drift between them.
pub const RELAY_BIND_ENV: &str = "AGENTSTACK_RELAY_BIND";

/// Parse the value of [`RELAY_BIND_ENV`]. Trimmed, because an address that
/// arrived from a shell export with stray whitespace is an obvious typo, not a
/// reason to refuse the run; anything else that is not an IP literal IS a
/// refusal, since guessing what the operator meant by a wrong bind address is
/// exactly the silent widening this seam exists to stop.
pub fn parse_relay_bind(value: &str) -> io::Result<IpAddr> {
    value.trim().parse::<IpAddr>().map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{RELAY_BIND_ENV} is not an IP address: {value:?}"),
        )
    })
}

/// The operator's explicit bind choice, or `None` when unset/empty.
///
/// Split from [`parse_relay_bind`] so the decision itself stays a pure,
/// env-free function that tests can drive without touching process state.
pub fn relay_bind_opt_in() -> io::Result<Option<IpAddr>> {
    match std::env::var(RELAY_BIND_ENV) {
        Ok(value) if !value.trim().is_empty() => parse_relay_bind(&value).map(Some),
        _ => Ok(None),
    }
}

/// The single refusal every bind path raises when it has no narrow,
/// container-reachable host address. `listener` names what refused; `reason`
/// says why no narrow address was available.
///
/// It is an error, not a fallback: binding `0.0.0.0` publishes a
/// token-authenticated tool-execution listener to every host on the local
/// network, and that is now a choice the operator makes by name, never a
/// consequence of an undetectable `docker0`.
pub fn relay_bind_refusal(listener: &str, reason: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::AddrNotAvailable,
        format!(
            "{listener}: refusing to start — no container-reachable host address could be \
             determined ({reason}), and falling back to the 0.0.0.0 wildcard would make this \
             token-authenticated listener reachable from every host on your local network. \
             Set {RELAY_BIND_ENV} to bind explicitly: {RELAY_BIND_ENV}=<ip> for a private \
             host interface the container can reach (e.g. the Docker bridge gateway \
             172.17.0.1), or {RELAY_BIND_ENV}=0.0.0.0 to accept the LAN-reachable wildcard \
             deliberately."
        ),
    )
}

/// Decide the narrowest host interface the per-execution relay can bind to
/// while staying reachable from the sandbox's sidecar, which dials the host
/// through `host.docker.internal`. The goal is defence-in-depth: never expose
/// the token-authenticated relay on a LAN-facing interface (the `0.0.0.0`
/// wildcard did exactly that), yet keep the one Docker path in reach.
///
/// The right answer is platform-specific, because `host.docker.internal`
/// resolves differently depending on where the daemon runs:
///
/// * **Linux, native daemon** — the sidecar's `--add-host
///   host.docker.internal:host-gateway` resolves to the Docker bridge gateway
///   (the `docker0` address, e.g. `172.17.0.1`), which is a real *host*
///   interface on a private, non-routable bridge subnet. Binding there is
///   reachable from any Docker container via the gateway but never from other
///   LAN hosts. `docker_bridge_gateway` carries that address (looked up by the
///   caller); when it is unknown there is no narrow answer and we REFUSE.
/// * **macOS / Windows, Docker Desktop** — the daemon runs inside a VM, so the
///   bridge gateway (`172.17.0.1`) lives in that VM, *not* on the host, and
///   cannot be bound here. Docker Desktop instead forwards
///   `host.docker.internal` to the host's **loopback**, so `127.0.0.1` is both
///   reachable from containers and the tightest possible bind — narrower even
///   than the Linux case (empirically verified against Docker Desktop). We
///   ignore any reported bridge gateway on these platforms.
/// * **Anything else** — unknown platform, no known-good narrow address, so
///   there is nothing to guess at safely: REFUSE.
///
/// `explicit` is the operator's [`RELAY_BIND_ENV`] opt-in and outranks the
/// derivation entirely — including the wildcard, which is the one way back to
/// the old always-functional behaviour. The token, exact grant, and call
/// ceiling are unchanged by any of this; the bind scope is defence in depth on
/// top of them.
///
/// This is a pure function of its inputs so the decision can be unit tested
/// without a Docker daemon; the actual bind (and the "not assignable" refusal
/// for Docker-Desktop-on-Linux) lives in
/// [`BlockingExecutionRelay::start_on_or_refuse`].
pub fn relay_bind_address(
    os: &str,
    docker_bridge_gateway: Option<IpAddr>,
    explicit: Option<IpAddr>,
    listener: &str,
) -> io::Result<IpAddr> {
    if let Some(bind) = explicit {
        return Ok(bind);
    }
    match os {
        // Docker Desktop's VM forwards host.docker.internal to the host
        // loopback; the in-VM bridge gateway is not a host interface.
        "macos" | "windows" => Ok(IpAddr::V4(Ipv4Addr::LOCALHOST)),
        // Native Linux: bind the docker0 gateway when known. Loopback is
        // intentionally NOT a fallback here — a container cannot reach the host
        // loopback via host-gateway on native Linux, so binding it would
        // silently break tool calls — and neither is the wildcard.
        "linux" => docker_bridge_gateway.ok_or_else(|| {
            relay_bind_refusal(
                listener,
                "this Linux host reported no docker0 bridge gateway (no daemon, no bridge \
                 network, or no IPv4 gateway on it)",
            )
        }),
        other => Err(relay_bind_refusal(
            listener,
            &format!("no container-reachable address is known for platform {other:?}"),
        )),
    }
}

struct Shared {
    token: String,
    grant: BTreeSet<String>,
    // The per-run call ceiling. `NonZeroU32` carries "a zero ceiling is
    // invalid" in the type, so the boundary check in `start` is a construction
    // (`NonZeroU32::new`) rather than a `== 0` guard that a future refactor
    // could drop.
    max_calls: NonZeroU32,
    calls: AtomicU32,
    connections: AtomicUsize,
    call: ExecutionCall,
}

struct ConnectionGuard(Arc<Shared>);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.connections.fetch_sub(1, Ordering::Release);
    }
}

struct ExecutionRelay {
    addr: SocketAddr,
    task: JoinHandle<()>,
    shared: Arc<Shared>,
}

impl Drop for ExecutionRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl ExecutionRelay {
    async fn start(
        bind: SocketAddr,
        token: String,
        grant: BTreeSet<String>,
        max_calls: u32,
        call: ExecutionCall,
    ) -> io::Result<Self> {
        let max_calls = match NonZeroU32::new(max_calls) {
            Some(n) if token.len() >= 32 && !grant.is_empty() => n,
            _ => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "invalid execution relay authority",
                ))
            }
        };
        let listener = TcpListener::bind(bind).await?;
        let addr = listener.local_addr()?;
        let shared = Arc::new(Shared {
            token,
            grant,
            max_calls,
            calls: AtomicU32::new(0),
            connections: AtomicUsize::new(0),
            call,
        });
        let task_shared = Arc::clone(&shared);
        let task = tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                if task_shared.connections.fetch_add(1, Ordering::AcqRel) >= MAX_CONNECTIONS {
                    task_shared.connections.fetch_sub(1, Ordering::Release);
                    drop(stream);
                    continue;
                }
                let state = Arc::clone(&task_shared);
                tokio::spawn(async move {
                    let _guard = ConnectionGuard(Arc::clone(&state));
                    let _ = serve(stream, state).await;
                });
            }
        });
        Ok(Self { addr, task, shared })
    }
}

async fn serve(stream: TcpStream, state: Arc<Shared>) -> io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut read = BufReader::new(read);
    loop {
        let mut frame = Vec::new();
        // `read_until` alone can grow without bound before it sees a newline.
        // Cap the reader itself so a hostile peer costs at most one frame plus
        // one sentinel byte.
        let n = (&mut read)
            .take((MAX_FRAME_BYTES + 1) as u64)
            .read_until(b'\n', &mut frame)
            .await?;
        if n == 0 {
            return Ok(());
        }
        if frame.len() > MAX_FRAME_BYTES {
            write_response(&mut write, json!({"ok":false,"error":"frame-too-large"})).await?;
            return Ok(());
        }
        if frame.last() == Some(&b'\n') {
            frame.pop();
        }
        let request: RelayRequest = match serde_json::from_slice(&frame) {
            Ok(v) => v,
            Err(_) => {
                write_response(&mut write, json!({"ok":false,"error":"invalid-request"})).await?;
                continue;
            }
        };
        let id = request.id;
        if !constant_time_eq(request.token.as_bytes(), state.token.as_bytes()) {
            write_response(
                &mut write,
                json!({"id":id,"ok":false,"error":"unauthorized"}),
            )
            .await?;
            return Ok(());
        }
        if !state.grant.contains(&request.tool) {
            write_response(
                &mut write,
                json!({"id":id,"ok":false,"error":"tool-not-granted"}),
            )
            .await?;
            continue;
        }
        let prior = state.calls.fetch_add(1, Ordering::AcqRel);
        if prior >= state.max_calls.get() {
            state.calls.fetch_sub(1, Ordering::Release);
            write_response(&mut write, json!({"id":id,"ok":false,"error":"call-limit"})).await?;
            continue;
        }
        let callback = Arc::clone(&state.call);
        let tool = request.tool;
        let arguments = Value::Object(request.arguments);
        let dispatched = tokio::task::spawn_blocking(move || callback(&tool, arguments)).await;
        let response = match dispatched {
            Err(_) => json!({"id":id,"ok":false,"error":"tool-call-failed"}),
            Ok(Ok(value)) => json!({"id":id,"ok":true,"result":value}),
            Ok(Err(RelayCallError::PolicyDenied)) => {
                json!({"id":id,"ok":false,"error":"policy-denied"})
            }
            Ok(Err(RelayCallError::Unavailable)) => {
                json!({"id":id,"ok":false,"error":"tool-unavailable"})
            }
            Ok(Err(RelayCallError::Failed)) => {
                json!({"id":id,"ok":false,"error":"tool-call-failed"})
            }
        };
        write_response(&mut write, response).await?;
    }
}

async fn write_response(
    write: &mut tokio::net::tcp::OwnedWriteHalf,
    value: Value,
) -> io::Result<()> {
    let mut bytes = serde_json::to_vec(&value)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "response serialization failed"))?;
    if bytes.len() > MAX_FRAME_BYTES {
        bytes = br#"{"ok":false,"error":"response-too-large"}"#.to_vec();
    }
    bytes.push(b'\n');
    write.write_all(&bytes).await
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (left, right) in a.iter().zip(b) {
        diff |= left ^ right;
    }
    diff == 0
}

/// Synchronous facade that owns the async runtime and relay task. Hold it for
/// the complete executor lifetime; dropping it aborts the listener and closes
/// the runtime.
pub struct BlockingExecutionRelay {
    addr: SocketAddr,
    relay: Option<ExecutionRelay>,
    rt: Option<tokio::runtime::Runtime>,
}

impl BlockingExecutionRelay {
    pub fn start_on(
        bind: IpAddr,
        token: String,
        grant: BTreeSet<String>,
        max_calls: u32,
        call: ExecutionCall,
    ) -> io::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        let relay = rt.block_on(ExecutionRelay::start(
            SocketAddr::new(bind, 0),
            token,
            grant,
            max_calls,
            call,
        ))?;
        Ok(Self {
            addr: relay.addr,
            relay: Some(relay),
            rt: Some(rt),
        })
    }

    /// Bind to `preferred`, and REFUSE with [`relay_bind_refusal`] when that
    /// address is not assignable on this host.
    ///
    /// The unassignable case is Docker-Desktop-on-Linux, where
    /// [`relay_bind_address`] hands back the in-VM bridge gateway
    /// (`172.17.0.1`) that has no host interface. That used to re-widen to
    /// `0.0.0.0`, publishing this relay to the LAN; now it stops the execution
    /// and names [`RELAY_BIND_ENV`], so the wildcard is only ever an operator's
    /// explicit choice. Loopback and native-Linux gateway binds never hit this
    /// path.
    ///
    /// Any other error (invalid authority, port exhaustion) is real and
    /// propagates unchanged.
    pub fn start_on_or_refuse(
        preferred: IpAddr,
        listener: &str,
        token: String,
        grant: BTreeSet<String>,
        max_calls: u32,
        call: ExecutionCall,
    ) -> io::Result<Self> {
        Self::start_on(preferred, token, grant, max_calls, call).map_err(|error| {
            if error.kind() == io::ErrorKind::AddrNotAvailable {
                relay_bind_refusal(
                    listener,
                    &format!("{preferred} is not assignable on this host ({error})"),
                )
            } else {
                error
            }
        })
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn call_count(&self) -> u32 {
        self.relay
            .as_ref()
            .map(|relay| relay.shared.calls.load(Ordering::Acquire))
            .unwrap_or(0)
    }
}

impl Drop for BlockingExecutionRelay {
    fn drop(&mut self) {
        // Abort the listener first, then make runtime shutdown non-blocking.
        // An upstream call already dispatched cannot be revoked, but a hung
        // callback must not prevent executor teardown from returning.
        self.relay.take();
        if let Some(rt) = self.rt.take() {
            rt.shutdown_background();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, BufReader, Write};
    use std::net::{Ipv4Addr, TcpStream};
    use std::sync::{mpsc, Barrier};
    use std::time::Duration;

    fn call(relay: &BlockingExecutionRelay, value: Value) -> Value {
        let mut stream = TcpStream::connect(relay.addr()).unwrap();
        writeln!(stream, "{}", serde_json::to_string(&value).unwrap()).unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        serde_json::from_str(&line).unwrap()
    }

    fn relay(max_calls: u32) -> BlockingExecutionRelay {
        BlockingExecutionRelay::start_on(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "a".repeat(64),
            BTreeSet::from(["github__get_issue".into()]),
            max_calls,
            Arc::new(|tool, args| Ok(json!({"tool":tool,"args":args}))),
        )
        .unwrap()
    }

    #[test]
    fn bind_address_is_never_the_wildcard_when_a_narrow_route_exists() {
        let gw: IpAddr = "172.17.0.1".parse().unwrap();
        // Native Linux with a known bridge gateway → bind that gateway.
        assert_eq!(
            relay_bind_address("linux", Some(gw), None, "relay").unwrap(),
            gw
        );
        // Docker Desktop forwards host.docker.internal to the host loopback;
        // the reported in-VM gateway must be ignored.
        assert_eq!(
            relay_bind_address("macos", Some(gw), None, "relay").unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
        assert_eq!(
            relay_bind_address("windows", None, None, "relay").unwrap(),
            IpAddr::V4(Ipv4Addr::LOCALHOST)
        );
    }

    /// The two undetermined cases fail CLOSED and say how to opt in, rather
    /// than publishing the relay on `0.0.0.0`.
    #[test]
    fn undetermined_bind_refuses_and_names_the_opt_in() {
        for (os, gateway) in [
            ("linux", None),
            ("freebsd", Some("172.17.0.1".parse().unwrap())),
        ] {
            let error = relay_bind_address(os, gateway, None, "tools_execute relay")
                .expect_err("an undetermined bind address must refuse");
            let message = error.to_string();
            assert!(message.contains("tools_execute relay"), "{message}");
            assert!(message.contains("refusing to start"), "{message}");
            assert!(message.contains("local network"), "{message}");
            assert!(message.contains(RELAY_BIND_ENV), "{message}");
            assert!(
                message.contains("AGENTSTACK_RELAY_BIND=0.0.0.0"),
                "{message}"
            );
        }
    }

    /// The opt-in outranks the derivation everywhere, so an operator who names
    /// the wildcard gets exactly the old behaviour back — on the platforms that
    /// now refuse and on the ones that already had a narrow answer.
    #[test]
    fn explicit_opt_in_restores_the_wildcard() {
        let wildcard = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        for os in ["linux", "macos", "windows", "freebsd"] {
            assert_eq!(
                relay_bind_address(os, None, Some(wildcard), "relay").unwrap(),
                wildcard
            );
        }
        // Any explicit address is honoured, not just the wildcard.
        let lan: IpAddr = "192.168.7.5".parse().unwrap();
        assert_eq!(
            relay_bind_address("linux", None, Some(lan), "relay").unwrap(),
            lan
        );
        // A set-but-unparseable opt-in is itself a refusal — never a silent
        // fall-through to the derivation.
        assert!(parse_relay_bind("localhost").is_err());
        assert_eq!(
            parse_relay_bind(" 0.0.0.0\n").unwrap(),
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        );
    }

    #[test]
    fn unassignable_preferred_bind_refuses_instead_of_widening() {
        // 203.0.113.9 is TEST-NET-3 (RFC 5737): guaranteed not assigned to any
        // local interface, so the preferred bind fails.
        let unassignable: IpAddr = "203.0.113.9".parse().unwrap();
        let error = BlockingExecutionRelay::start_on_or_refuse(
            unassignable,
            "tools_execute relay",
            "a".repeat(64),
            BTreeSet::from(["github__get_issue".into()]),
            1,
            Arc::new(|_, _| Ok(Value::Null)),
        )
        .err()
        .expect("an unassignable bind must refuse, never widen to 0.0.0.0");
        let message = error.to_string();
        assert!(message.contains("203.0.113.9"), "{message}");
        assert!(message.contains(RELAY_BIND_ENV), "{message}");

        // With the opt-in the operator can still name the wildcard, and the
        // relay starts exactly as it used to.
        let relay = BlockingExecutionRelay::start_on_or_refuse(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            "tools_execute relay",
            "a".repeat(64),
            BTreeSet::from(["github__get_issue".into()]),
            1,
            Arc::new(|_, _| Ok(Value::Null)),
        )
        .expect("an explicit wildcard still binds");
        assert_eq!(relay.addr().ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }

    #[test]
    fn requires_token_and_exact_grant() {
        let relay = relay(2);
        let denied = call(
            &relay,
            json!({"id":1,"token":"wrong","tool":"github__get_issue","arguments":{}}),
        );
        assert_eq!(denied["error"], "unauthorized");
        let not_granted = call(
            &relay,
            json!({"id":2,"token":"a".repeat(64),"tool":"github__list_comments","arguments":{}}),
        );
        assert_eq!(not_granted["error"], "tool-not-granted");
        let ok = call(
            &relay,
            json!({"id":3,"token":"a".repeat(64),"tool":"github__get_issue","arguments":{"n":1}}),
        );
        assert_eq!(ok["ok"], true);
    }

    #[test]
    fn call_limit_is_global_to_the_execution() {
        let relay = relay(1);
        let req =
            || json!({"id":1,"token":"a".repeat(64),"tool":"github__get_issue","arguments":{}});
        assert_eq!(call(&relay, req())["ok"], true);
        assert_eq!(call(&relay, req())["error"], "call-limit");
    }

    #[test]
    fn malformed_and_extra_fields_fail_before_dispatch() {
        let dispatches = Arc::new(AtomicU32::new(0));
        let seen = Arc::clone(&dispatches);
        let relay = BlockingExecutionRelay::start_on(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            "a".repeat(64),
            BTreeSet::from(["github__get_issue".into()]),
            2,
            Arc::new(move |_, _| {
                seen.fetch_add(1, Ordering::AcqRel);
                Ok(Value::Null)
            }),
        )
        .unwrap();
        assert_eq!(
            call(&relay, json!({"not":"a request"}))["error"],
            "invalid-request"
        );
        assert_eq!(
            call(
                &relay,
                json!({"id":1,"token":"a".repeat(64),"tool":"github__get_issue","arguments":{},"extra":true})
            )["error"],
            "invalid-request"
        );
        assert_eq!(dispatches.load(Ordering::Acquire), 0);
    }

    #[test]
    fn blocking_calls_do_not_stall_auth_or_accept_loop() {
        let barrier = Arc::new(Barrier::new(3));
        let (started_tx, started_rx) = mpsc::channel();
        let callback_barrier = Arc::clone(&barrier);
        let relay = Arc::new(
            BlockingExecutionRelay::start_on(
                IpAddr::V4(Ipv4Addr::LOCALHOST),
                "a".repeat(64),
                BTreeSet::from(["github__get_issue".into()]),
                4,
                Arc::new(move |_, _| {
                    started_tx.send(()).unwrap();
                    callback_barrier.wait();
                    Ok(Value::Null)
                }),
            )
            .unwrap(),
        );
        let request =
            || json!({"id":1,"token":"a".repeat(64),"tool":"github__get_issue","arguments":{}});
        let one = {
            let relay = Arc::clone(&relay);
            let request = request();
            std::thread::spawn(move || call(&relay, request))
        };
        let two = {
            let relay = Arc::clone(&relay);
            let request = request();
            std::thread::spawn(move || call(&relay, request))
        };
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        started_rx.recv_timeout(Duration::from_secs(1)).unwrap();

        let (response_tx, response_rx) = mpsc::channel();
        let auth_relay = Arc::clone(&relay);
        let auth = std::thread::spawn(move || {
            let response = call(
                &auth_relay,
                json!({"id":2,"token":"wrong","tool":"github__get_issue","arguments":{}}),
            );
            response_tx.send(response).unwrap();
        });
        let response = response_rx
            .recv_timeout(Duration::from_millis(250))
            .expect("reactor must answer auth while callbacks are blocked");
        assert_eq!(response["error"], "unauthorized");
        barrier.wait();
        one.join().unwrap();
        two.join().unwrap();
        auth.join().unwrap();
    }
}
