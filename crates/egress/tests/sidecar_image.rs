//! The egress-proxy **sidecar image**, exercised for real — Docker-gated.
//!
//! Builds `docker/egress-proxy.Dockerfile` from the repo root, runs the
//! resulting container, and proves the whole sidecar contract from the
//! outside: READY lines appear on stdout once the listeners are bound, a
//! denied CONNECT is refused (403) and an allowed one tunnels end-to-end to a
//! host sink, every decision comes back as a parseable `RunEvent` JSON line
//! in the container's logs, and a ruleset from a future version is refused at
//! startup (fail closed).
//!
//! SKIPS (never fails) without a Docker daemon. The first run compiles the
//! workspace inside rust:alpine (~minutes); Docker caches it after. Run:
//!   cargo test -p agentstack-egress --test sidecar_image -- --nocapture

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use agentstack_core::manifest::Policy;
use agentstack_policy::{CompiledRuleset, RULESET_VERSION};
use agentstack_recorder::RunEvent;

const IMAGE_TAG: &str = "agentstack/egress-proxy:test";
const DENIED: &str = "blocked.invalid";

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// Build the sidecar image once per test process (both tests need it).
/// `None` = no daemon / build failed → skip.
fn image_ready() -> bool {
    static BUILT: OnceLock<bool> = OnceLock::new();
    *BUILT.get_or_init(|| {
        let up = Command::new("docker")
            .arg("info")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !up {
            eprintln!("SKIP: no Docker daemon");
            return false;
        }
        eprintln!("building {IMAGE_TAG} (first run compiles the workspace — cached after)…");
        let ok = Command::new("docker")
            .args([
                "build",
                "-f",
                "docker/egress-proxy.Dockerfile",
                "-t",
                IMAGE_TAG,
                ".",
            ])
            .current_dir(repo_root())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if !ok {
            eprintln!("SKIP: docker build failed");
        }
        ok
    })
}

/// A ruleset denying `DENIED` for every server (machine `"*"` key).
fn test_ruleset() -> CompiledRuleset {
    let mut machine = Policy::default();
    machine
        .egress
        .insert("*".to_string(), vec![format!("!{DENIED}")]);
    agentstack_policy::compile(&machine, &Policy::default(), &["demo"])
}

/// Write `ruleset` to a fresh temp file Docker Desktop can bind-mount.
fn write_ruleset(name: &str, ruleset: &CompiledRuleset) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("asb-sidecar-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, serde_json::to_vec(ruleset).unwrap()).unwrap();
    path
}

fn docker_logs(id: &str) -> String {
    let out = Command::new("docker")
        .args(["logs", id])
        .output()
        .expect("docker logs");
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn remove_container(id: &str) {
    let _ = Command::new("docker").args(["rm", "-f", id]).status();
}

/// One request/response over a raw TCP stream: send, then read whatever
/// arrives within the timeout.
fn send_and_read(stream: &mut TcpStream, payload: &[u8]) -> String {
    stream.write_all(payload).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let mut buf = [0u8; 512];
    let n = stream.read(&mut buf).unwrap_or(0);
    String::from_utf8_lossy(&buf[..n]).to_string()
}

#[test]
fn sidecar_container_enforces_policy_and_reports_on_stdout() {
    if !image_ready() {
        return;
    }

    // A host sink the ALLOWED tunnel should reach (via host.docker.internal
    // from the sidecar, which sits on an ordinary bridge network here).
    let sink = TcpListener::bind("0.0.0.0:0").unwrap();
    let sink_port = sink.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::<u8>::new()));
    let recv = Arc::clone(&received);
    std::thread::spawn(move || {
        if let Ok((mut s, _)) = sink.accept() {
            let mut buf = [0u8; 256];
            if let Ok(n) = s.read(&mut buf) {
                recv.lock().unwrap().extend_from_slice(&buf[..n]);
                let _ = s.write_all(b"ack");
            }
        }
    });

    let ruleset_path = write_ruleset("ruleset.json", &test_ruleset());
    let out = Command::new("docker")
        .args([
            "run",
            "-d",
            "-e",
            "AGENTSTACK_RULESET=/ruleset.json",
            "-e",
            "AGENTSTACK_SERVERS=demo",
            "-e",
            "AGENTSTACK_PROXY_BASE_PORT=18080",
            "-v",
            &format!("{}:/ruleset.json:ro", ruleset_path.display()),
            "--add-host",
            "host.docker.internal:host-gateway",
            "-p",
            "127.0.0.1:0:18080",
            IMAGE_TAG,
        ])
        .output()
        .expect("docker run");
    assert!(
        out.status.success(),
        "docker run failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let id = String::from_utf8_lossy(&out.stdout).trim().to_string();

    // Wait for the READY line — the bind-complete signal the host relies on.
    let deadline = Instant::now() + Duration::from_secs(15);
    while !docker_logs(&id).contains("READY demo 18080") {
        if Instant::now() > deadline {
            let logs = docker_logs(&id);
            remove_container(&id);
            panic!("sidecar never printed READY; logs:\n{logs}");
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // The host-side port Docker mapped for the proxy.
    let port_out = Command::new("docker")
        .args(["port", &id, "18080/tcp"])
        .output()
        .expect("docker port");
    let mapped = String::from_utf8_lossy(&port_out.stdout)
        .lines()
        .next()
        .expect("a mapped port")
        .trim()
        .to_string();

    // 1) A denied host is refused at the proxy.
    let mut c = TcpStream::connect(&mapped).unwrap();
    let resp = send_and_read(
        &mut c,
        format!("CONNECT {DENIED}:443 HTTP/1.1\r\n\r\n").as_bytes(),
    );
    assert!(
        resp.contains("403"),
        "denied host must get 403, got: {resp:?}"
    );

    // 2) An allowed host tunnels end-to-end: CONNECT, then bytes reach the
    //    host sink through the sidecar.
    let mut c = TcpStream::connect(&mapped).unwrap();
    let resp = send_and_read(
        &mut c,
        format!("CONNECT host.docker.internal:{sink_port} HTTP/1.1\r\n\r\n").as_bytes(),
    );
    assert!(
        resp.contains("200"),
        "allowed host must tunnel, got: {resp:?}"
    );
    let ack = send_and_read(&mut c, b"hello-through-the-sidecar");
    assert_eq!(ack, "ack", "the tunnel must carry bytes both ways");
    assert_eq!(
        String::from_utf8_lossy(&received.lock().unwrap()),
        "hello-through-the-sidecar",
        "the sink must receive what the tunnel carried"
    );

    // 3) Both decisions came back as parseable RunEvent JSON on stdout.
    let logs = docker_logs(&id);
    remove_container(&id);
    let events: Vec<RunEvent> = logs
        .lines()
        .filter(|l| l.starts_with('{'))
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::Egress { allowed: false, host, server, .. }
                if host == DENIED && server == "demo"
        )),
        "the BLOCK must be a RunEvent line; logs:\n{logs}"
    );
    assert!(
        events.iter().any(|e| matches!(
            e,
            RunEvent::Egress { allowed: true, host, .. } if host == "host.docker.internal"
        )),
        "the ALLOW must be a RunEvent line; logs:\n{logs}"
    );
}

#[test]
fn sidecar_refuses_a_future_ruleset_version_at_startup() {
    if !image_ready() {
        return;
    }

    let future = CompiledRuleset {
        version: RULESET_VERSION + 1,
        ..CompiledRuleset::default()
    };
    let path = write_ruleset("ruleset-future.json", &future);

    // Foreground run: the binary must refuse to start, with the reason on
    // stderr and a non-zero exit — fail closed, never guess.
    let out = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-e",
            "AGENTSTACK_RULESET=/ruleset.json",
            "-e",
            "AGENTSTACK_SERVERS=demo",
            "-v",
            &format!("{}:/ruleset.json:ro", path.display()),
            IMAGE_TAG,
        ])
        .output()
        .expect("docker run");
    assert!(
        !out.status.success(),
        "a future ruleset version must refuse to start"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("newer than this proxy understands"),
        "the refusal names the version mismatch: {stderr}"
    );
}
