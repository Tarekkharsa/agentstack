//! `agentstack-egress-proxy` — the egress proxy as a standalone binary, built
//! to run as a **sidecar container** next to a sandboxed run (the
//! no-direct-route lockdown: the sandbox sits on an internal Docker network
//! whose only reachable peer is this proxy, so ignoring `HTTPS_PROXY` gets a
//! container nothing — there is no other route, not even DNS).
//!
//! Contract with the host CLI (all inputs via env, no argv parsing):
//! - `AGENTSTACK_RULESET`   — path to the compiled ruleset JSON, mounted
//!   read-only into the container by the host. Refuses to start if the
//!   ruleset's version is newer than this binary understands (and
//!   `EgressGuard` fails closed on every decision besides — defense in depth
//!   against version skew across the process boundary).
//! - `AGENTSTACK_SERVERS`   — comma-separated server identities; server *i*
//!   listens on `base_port + i`, so the host can compute every endpoint
//!   a priori (`<alias>:<port>` on the internal network).
//! - `AGENTSTACK_PROXY_BASE_PORT` — optional, default 18080.
//!
//! Stdout is the event channel back to the host, which tails this container's
//! logs: one `READY <server> <port>` line per endpoint once it is actually
//! listening, then one flight-recorder `RunEvent` as JSON per egress decision
//! (the identical lines `events.jsonl` holds — `{"event":"egress",…}`).
//! Diagnostics go to stderr, never stdout.

#![forbid(unsafe_code)]

use std::io::Write;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;

use agentstack_egress::proxy::ProxyConfig;
use agentstack_egress::{EgressBridge, EventSink};
use agentstack_policy::{CompiledRuleset, RULESET_VERSION};

/// The ruleset file comes from the trusted host CLI, but bound it anyway —
/// a sidecar should never allocate unboundedly off one input.
const MAX_RULESET_BYTES: u64 = 10 * 1024 * 1024;

const DEFAULT_BASE_PORT: u16 = 18080;

fn main() {
    if let Err(e) = run() {
        eprintln!("agentstack-egress-proxy: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let ruleset = load_ruleset()?;
    let servers = server_list()?;
    let base_port = base_port()?;

    // Server i listens on base+i; refuse (rather than wrap) if that overflows.
    let pairs: Vec<(String, u16)> = servers
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let off = u16::try_from(i).ok();
            let port = off.and_then(|o| base_port.checked_add(o));
            port.map(|p| (s.clone(), p))
                .ok_or_else(|| format!("too many servers for base port {base_port}"))
        })
        .collect::<Result<_, _>>()?;

    // Every decision goes to stdout as one self-describing JSON line, flushed
    // immediately — the host is tailing container logs, not reading a file.
    let sink: EventSink = Arc::new(|ev| {
        if let Ok(line) = serde_json::to_string(&ev) {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "{line}");
            let _ = out.flush();
        }
    });

    // Anti-SSRF is on by default: resolved targets must be global unicast.
    // The Docker demo dials the host gateway (host.docker.internal), so it opts
    // out via this env — never set in a real deployment.
    //
    // The peer token (if the host provided one) authenticates the sandbox: only
    // a client presenting it may tunnel. Absent → no auth (loopback/dev).
    let config = ProxyConfig {
        allow_local_targets: env_flag("AGENTSTACK_ALLOW_LOCAL_TARGETS"),
        auth_token: std::env::var("AGENTSTACK_PROXY_TOKEN")
            .ok()
            .filter(|t| !t.is_empty()),
    };

    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("starting the async runtime: {e}"))?;
    rt.block_on(async move {
        // 0.0.0.0: inside the container, "everyone" is only whoever shares a
        // Docker network with this sidecar — that scoping is the runtime's
        // topology, not a bind address's.
        let (bridge, endpoints) = EgressBridge::start_fixed_with(
            IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            &pairs,
            ruleset,
            sink,
            config,
        )
        .await
        .map_err(|e| format!("binding the proxy listeners: {e}"))?;

        // READY only after every listener is bound: the host waits for these
        // lines before starting the sandbox, so no request can race the bind.
        {
            let mut out = std::io::stdout().lock();
            for ep in &endpoints {
                writeln!(out, "READY {} {}", ep.server, ep.addr.port())
                    .map_err(|e| format!("writing READY: {e}"))?;
            }
            out.flush().map_err(|e| format!("flushing READY: {e}"))?;
        }

        // Serve until the container is stopped — teardown is the host
        // removing the container, not a signal protocol.
        let _bridge = bridge;
        std::future::pending::<()>().await;
        unreachable!("pending() never resolves")
    })
}

/// True when an env var is set to a truthy value (`1`/`true`/`yes`). Absent or
/// anything else is false — the SSRF check stays on unless explicitly disabled.
fn env_flag(name: &str) -> bool {
    matches!(
        std::env::var(name).ok().as_deref(),
        Some("1") | Some("true") | Some("yes")
    )
}

fn load_ruleset() -> Result<CompiledRuleset, String> {
    let path = std::env::var("AGENTSTACK_RULESET")
        .map_err(|_| "AGENTSTACK_RULESET is not set (path to the compiled ruleset JSON)")?;
    let meta = std::fs::metadata(&path).map_err(|e| format!("reading {path}: {e}"))?;
    if meta.len() > MAX_RULESET_BYTES {
        return Err(format!(
            "{path} is {} bytes — larger than the {MAX_RULESET_BYTES}-byte bound",
            meta.len()
        ));
    }
    let raw = std::fs::read(&path).map_err(|e| format!("reading {path}: {e}"))?;
    let ruleset: CompiledRuleset =
        serde_json::from_slice(&raw).map_err(|e| format!("parsing {path}: {e}"))?;
    // Fail closed at startup with a readable reason; EgressGuard repeats the
    // check per decision in case a guard is ever constructed another way.
    if ruleset.version > RULESET_VERSION {
        return Err(format!(
            "ruleset version {} is newer than this proxy understands (max {RULESET_VERSION}) \
             — rebuild the sidecar image",
            ruleset.version
        ));
    }
    Ok(ruleset)
}

fn server_list() -> Result<Vec<String>, String> {
    let raw = std::env::var("AGENTSTACK_SERVERS")
        .map_err(|_| "AGENTSTACK_SERVERS is not set (comma-separated server identities)")?;
    let servers: Vec<String> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect();
    if servers.is_empty() {
        return Err("AGENTSTACK_SERVERS names no servers".to_string());
    }
    Ok(servers)
}

fn base_port() -> Result<u16, String> {
    match std::env::var("AGENTSTACK_PROXY_BASE_PORT") {
        Ok(v) => v
            .parse::<u16>()
            .map_err(|_| format!("AGENTSTACK_PROXY_BASE_PORT is not a port: {v:?}")),
        Err(_) => Ok(DEFAULT_BASE_PORT),
    }
}
