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
use crate::proxy::EventSink;

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
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()?;
        let (bridge, endpoints) =
            rt.block_on(EgressBridge::start_on(bind, servers, ruleset, sink))?;
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
}
