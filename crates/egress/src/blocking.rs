//! A synchronous facade over [`EgressBridge`](crate::EgressBridge) for callers
//! that aren't async — chiefly the `cli`, which drives a sandbox run on a
//! blocking thread. The bridge owns its own tokio runtime (kept alive for the
//! run so the proxies keep serving), exactly like the runtime crate's Docker
//! backend — so tokio stays confined to this crate (rule 6) and never becomes
//! a `cli` dependency.

use std::io;
use std::net::IpAddr;

use agentstack_policy::CompiledRuleset;

use crate::bridge::{EgressBridge, ProxyEndpoint};
use crate::proxy::{EventSink, ProxyConfig};

/// A running set of per-server egress proxies plus the runtime driving them.
/// Hold it for the life of the sandbox run; dropping it stops every proxy and
/// shuts the runtime down.
pub struct BlockingBridge {
    endpoints: Vec<ProxyEndpoint>,
    // Drop order matters: the bridge (which aborts the proxy tasks) must drop
    // before the runtime it ran on. Fields drop in declaration order.
    _bridge: EgressBridge,
    _rt: tokio::runtime::Runtime,
}

impl BlockingBridge {
    /// Start one proxy per server, bound on `bind`, on a fresh runtime.
    pub fn start_on(
        bind: IpAddr,
        servers: &[String],
        ruleset: CompiledRuleset,
        sink: EventSink,
    ) -> io::Result<BlockingBridge> {
        Self::start_on_with(bind, servers, ruleset, sink, ProxyConfig::default())
    }

    /// [`start_on`](Self::start_on) with explicit transport config — lets the
    /// `cli` opt into `allow_local_targets` for a demo without a rebuild.
    pub fn start_on_with(
        bind: IpAddr,
        servers: &[String],
        ruleset: CompiledRuleset,
        sink: EventSink,
        config: ProxyConfig,
    ) -> io::Result<BlockingBridge> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let (bridge, endpoints) = rt.block_on(EgressBridge::start_on_with(
            bind, servers, ruleset, sink, config,
        ))?;
        Ok(BlockingBridge {
            endpoints,
            _bridge: bridge,
            _rt: rt,
        })
    }

    /// The endpoints to point each server's traffic at.
    pub fn endpoints(&self) -> &[ProxyEndpoint] {
        &self.endpoints
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::EventSink;
    use agentstack_recorder::RunEvent;
    use std::net::Ipv4Addr;
    use std::sync::{Arc, Mutex};

    #[test]
    fn blocking_bridge_serves_and_records_on_a_worker_thread() {
        // Prove the proxies keep serving after start_on returns (the runtime
        // is kept alive), by driving one endpoint with a blocking TCP client.
        let events = Arc::new(Mutex::new(Vec::<RunEvent>::new()));
        let ev = Arc::clone(&events);
        let sink: EventSink = Arc::new(move |e| ev.lock().unwrap().push(e));

        let bridge = BlockingBridge::start_on(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            &["demo".to_string()],
            CompiledRuleset::default(),
            sink,
        )
        .unwrap();
        let addr = bridge.endpoints()[0].addr;

        // A non-CONNECT request gets a 400 — enough to prove the proxy is live
        // and reachable synchronously.
        use std::io::{Read, Write};
        let mut c = std::net::TcpStream::connect(addr).unwrap();
        c.write_all(b"GET / HTTP/1.1\r\n\r\n").unwrap();
        let mut buf = [0u8; 64];
        let n = c.read(&mut buf).unwrap();
        assert!(String::from_utf8_lossy(&buf[..n]).contains("400"));
    }

    /// One slow event-sink write must not stall other tunnels. This runtime
    /// has a SINGLE worker thread, so before the spool a sink doing blocking
    /// I/O inside `(self.on_event)(…)` parked that worker and froze every
    /// in-flight connection on the listener; with the spool the proxy task
    /// only enqueues and the blocking write happens on the writer thread.
    #[test]
    fn a_slow_event_sink_does_not_stall_other_tunnels() {
        use std::io::{Read, Write};
        use std::sync::mpsc;
        use std::time::Duration;

        // The writer parks on `release` inside the first write — simulating a
        // stalled filesystem append — and `entered` proves it got the event.
        let (entered_tx, entered_rx) = mpsc::channel::<()>();
        let (release_tx, release_rx) = mpsc::channel::<()>();
        let spool = crate::spool::WriterSpool::spawn("test-slow", move |_ev: RunEvent| {
            let _ = entered_tx.send(());
            let _ = release_rx.recv(); // parked until release_tx drops below
        })
        .unwrap();
        let events = spool.sender();
        let sink: EventSink = Arc::new(move |ev| events.send(ev));

        // With an auth token set, an unauthenticated CONNECT emits a block
        // event and a 407 without resolving or dialing anything — the pure
        // sink-path exercise.
        let bridge = BlockingBridge::start_on_with(
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            &["demo".to_string()],
            CompiledRuleset::default(),
            sink,
            crate::proxy::ProxyConfig {
                auth_token: Some("token".to_string()),
                ..Default::default()
            },
        )
        .unwrap();
        let addr = bridge.endpoints()[0].addr;

        let connect = || {
            let mut c = std::net::TcpStream::connect(addr).unwrap();
            c.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            c.write_all(b"CONNECT example.com:443 HTTP/1.1\r\n\r\n")
                .unwrap();
            let mut buf = [0u8; 128];
            let n = c.read(&mut buf).expect("proxy answered within timeout");
            String::from_utf8_lossy(&buf[..n]).into_owned()
        };

        // First tunnel: refused, and its block event is now stuck mid-write…
        assert!(connect().contains("407"));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();

        // …yet a second tunnel is still answered on the single worker.
        assert!(
            connect().contains("407"),
            "a second tunnel was answered while the sink write was stalled"
        );

        drop(release_tx); // unpark the writer so the spool can drain and join
        drop(bridge); // proxies release their sender handles first
        drop(spool); // then the flush-join returns
    }
}
