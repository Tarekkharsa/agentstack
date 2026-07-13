//! Authenticated, exact-grant relay for hostile executor runtimes.
//!
//! The executor container has no direct network route. Its runtime sends one
//! bounded JSON object per line through the lockdown sidecar's fixed relay to
//! this listener. Authentication establishes the execution identity; the
//! immutable exact grant authorizes each tool; the CLI-owned callback then
//! delegates to the existing gateway for policy enforcement and audit.

use std::collections::BTreeSet;
use std::io;
use std::net::{IpAddr, SocketAddr};
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

struct Shared {
    token: String,
    grant: BTreeSet<String>,
    max_calls: u32,
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
        if token.len() < 32 || grant.is_empty() || max_calls == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid execution relay authority",
            ));
        }
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
        if prior >= state.max_calls {
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
