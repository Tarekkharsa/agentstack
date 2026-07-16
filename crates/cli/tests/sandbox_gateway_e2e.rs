// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Gateway-unification Session 2, driven through the REAL `agentstack run
//! --sandbox` binary — Docker-gated.
//!
//! Proves the two ends of the milestone's `--sandbox` wiring:
//!
//! 1. **Hard trust gate.** An UNtrusted bundle gets NO gateway routing — no
//!    secret resolves, no endpoint config is mounted — even though the same
//!    bundle, trusted, would route. (Test 1; needs no lock/trust.)
//! 2. **Tool policy enforced + recorded.** A trusted bundle's MCP traffic goes
//!    through the gateway: a `[policy.tools]`-denied call is refused and lands
//!    in the run's own `events.jsonl` as a `denied` ToolCall. The container
//!    reads the mounted gateway config and calls the endpoint directly — the
//!    same path the real harness's MCP client takes. (Test 2.)
//!
//! Hermetic: the upstream (`figma`) is never contacted — the denial happens at
//! the gateway before dispatch — so no network is touched. Compiles only with
//! `--features sandbox`; SKIPS without a Docker daemon or the node sandbox
//! image. Run where Docker exists:
//!   cargo test -p agentstack --features sandbox --test sandbox_gateway_e2e -- --nocapture
#![cfg(feature = "sandbox")]

use std::fs;
use std::process::Command;

use agentstack::calllog::RunEvent;

/// Read a run's events.jsonl directly from its home, WITHOUT mutating the
/// process-global `AGENTSTACK_HOME` (which `RunLog::read` reads live). These
/// tests run concurrently in one binary; a global env mutation would race
/// between tests and clobber each other's home.
fn read_events(as_home: &std::path::Path, run_id: &str) -> Vec<RunEvent> {
    let path = as_home.join("runs").join(run_id).join("events.jsonl");
    let Ok(raw) = fs::read_to_string(&path) else {
        return Vec::new();
    };
    raw.lines()
        .filter_map(|l| serde_json::from_str::<RunEvent>(l).ok())
        .collect()
}

/// A node image is the sandbox runner (the shipped `docker/sandbox.Dockerfile`
/// base) — its `node` binary is both the fake harness and the MCP client.
const IMAGE: &str = "node:22-slim";
/// The egress sidecar image (lockdown), built from the workspace so it carries
/// the current relay code.
const EGRESS_IMAGE: &str = "agentstack/egress-proxy:gateway-e2e";

fn docker_and_image() -> bool {
    let up = Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);
    if !up {
        eprintln!("SKIP: no Docker daemon");
        return false;
    }
    let pulled = Command::new("docker")
        .args(["pull", IMAGE])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !pulled {
        eprintln!("SKIP: cannot pull {IMAGE}");
        return false;
    }
    true
}

/// Build the egress sidecar image from the workspace (so it has the current
/// relay code). Returns false to SKIP if Docker or the build is unavailable.
fn build_egress_image() -> bool {
    let repo_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    eprintln!("building {EGRESS_IMAGE} (first run compiles the workspace — cached after)…");
    Command::new("docker")
        .args([
            "build",
            "-f",
            "docker/egress-proxy.Dockerfile",
            "-t",
            EGRESS_IMAGE,
            ".",
        ])
        .current_dir(repo_root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for n in chars.by_ref() {
                if n == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Machine home with the tool-denying policy and the `node` throwaway harness
/// (whose adapter renders an HTTP MCP entry, so the gateway config lands in
/// `/root/.config/gwtest.json`). Returns (home, as_home).
fn machine_home(tmp: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = tmp.join("home");
    let as_home = home.join(".agentstack");
    fs::create_dir_all(as_home.join("adapters")).unwrap();
    // Machine policy denies figma post_* (rename-proof "*" would also work; a
    // named server is enough here).
    fs::write(
        as_home.join("agentstack.toml"),
        "version = 1\n[policy.tools]\nfigma = [\"!post_*\"]\n",
    )
    .unwrap();
    // A throwaway harness: launch binary `node`, and an adapter that renders an
    // HTTP MCP entry into `~/.gwtest.json` (→ container `/root/.config/gwtest.json`).
    // A NESTED global config path (like codex's ~/.codex/config.toml) — proves
    // the container mount preserves the intermediate dir and Docker creates it.
    fs::write(
        as_home.join("adapters/gwtest.yaml"),
        "id: gwtest\n\
         display: GW Test\n\
         detect:\n  bin: node\n\
         config:\n  path: ~/.config/gwtest.json\n  format: json\n\
         mcp:\n  location: mcpServers\n  fields:\n    url: url\n    headers: headers\n\
         project:\n  config: .mcp.json\n",
    )
    .unwrap();
    (home, as_home)
}

/// The in-container MCP client: reads the mounted gateway config, confirms the
/// stale project config was shadowed empty, then calls a DENIED tool through
/// the gateway. Retries a few times so a cold container's first dial can race
/// DNS/endpoint warmup; each attempt has its own timeout. Reused by both the
/// `--sandbox` (direct host route) and `--lockdown` (sidecar relay) tests — it
/// reads the URL from the config, so it doesn't care which route it takes.
const CLIENT_SCRIPT: &str = r#"
const fs=require('fs');
const c=JSON.parse(fs.readFileSync('/root/.config/gwtest.json','utf8'));
const s=c.mcpServers['agentstack-gateway'];
const body=JSON.stringify({jsonrpc:'2.0',id:1,method:'tools/call',params:{name:'figma__post_comment',arguments:{}}});
const sleep=ms=>new Promise(r=>setTimeout(r,ms));
(async()=>{
  try{const proj=JSON.parse(fs.readFileSync('/workspace/.mcp.json','utf8'));console.log('SHADOW',JSON.stringify(proj.mcpServers||{}));}
  catch(e){console.log('SHADOW none');}
  for(let i=0;i<8;i++){
    try{
      const r=await fetch(s.url,{method:'POST',headers:{'content-type':'application/json','X-Agentstack-Token':s.headers['X-Agentstack-Token']},body,signal:AbortSignal.timeout(4000)});
      console.log('GWRESP',await r.text());
      return;
    }catch(e){console.error('GWTRY',i,String(e));await sleep(500);}
  }
  console.error('GWERR gave up');process.exit(7);
})();
"#;

/// A project declaring one HTTP MCP server the gateway will proxy.
fn project(tmp: &std::path::Path) -> std::path::PathBuf {
    let proj = tmp.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[servers.figma]\ntype = \"http\"\nurl = \"https://figma.invalid/mcp\"\n",
    )
    .unwrap();
    proj
}

#[test]
fn untrusted_bundle_gets_no_gateway_routing() {
    if !docker_and_image() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, as_home) = machine_home(tmp.path());
    let proj = project(tmp.path());

    // NOT trusted (no `lock`/`trust`). The container just prints and exits.
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args([
            "run",
            "--sandbox",
            "gwtest",
            "--",
            "-e",
            "console.log('ran')",
        ])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", IMAGE)
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&out.stderr));
    eprintln!("--- untrusted run stdout ---\n{stdout}\n--- stderr ---\n{stderr}");

    // The hard gate: an untrusted bundle is NOT routed through the gateway.
    assert!(
        !stdout.contains("routed through the gateway"),
        "untrusted bundle must not get gateway routing: {stdout}"
    );
    // And the run still warns it's unreviewed.
    assert!(
        stderr.contains("unreviewed") || stdout.contains("UNTRUSTED"),
        "untrusted run should be flagged: {stdout} / {stderr}"
    );
}

#[test]
fn trusted_bundle_routes_denied_tool_and_records_it() {
    if !docker_and_image() {
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, as_home) = machine_home(tmp.path());
    let proj = project(tmp.path());

    // A STALE project config with a direct entry + baked secret, as a prior
    // `agentstack apply` would leave. The gateway wiring must SHADOW it so the
    // container can't reach that upstream around the gateway.
    fs::write(
        proj.join(".mcp.json"),
        "{\"mcpServers\":{\"evil-direct\":{\"type\":\"http\",\"url\":\"https://evil.invalid/mcp\",\"headers\":{\"Authorization\":\"Bearer STALE-SECRET\"}}}}",
    )
    .unwrap();

    // Lock + trust the bundle so the gateway builds a live surface. `lock`
    // takes no positional (uses cwd); `trust` defaults to `.`.
    let bin = env!("CARGO_BIN_EXE_agentstack");
    for step in [vec!["lock"], vec!["trust"]] {
        let s = Command::new(bin)
            .args(&step)
            .current_dir(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", &as_home)
            .output()
            .unwrap();
        assert!(
            s.status.success(),
            "`agentstack {}` failed: {}",
            step[0],
            strip_ansi(&String::from_utf8_lossy(&s.stderr))
        );
    }

    // The container reads the mounted gateway config and calls a DENIED tool
    // through the endpoint. node's fetch dials host.docker.internal directly.
    let out = Command::new(bin)
        .args(["run", "--sandbox", "gwtest", "--", "-e", CLIENT_SCRIPT])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", IMAGE)
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&out.stderr));
    eprintln!("--- trusted run stdout ---\n{stdout}\n--- stderr ---\n{stderr}");

    // Routing happened.
    assert!(
        stdout.contains("routed through the gateway"),
        "trusted bundle should route through the gateway: {stdout}"
    );
    // The stale project config was shadowed to an empty server map — the
    // baked secret never reached the container.
    assert!(
        stdout.contains("SHADOW {}"),
        "stale project .mcp.json must be shadowed empty: {stdout}"
    );
    assert!(
        !stdout.contains("STALE-SECRET") && !stdout.contains("evil-direct"),
        "the stale direct entry/secret must not be visible in the container: {stdout}"
    );

    // Parse the run id and read its flight recorder.
    let run_id = stdout
        .split_whitespace()
        .find(|w| w.starts_with("r-"))
        .map(|w| w.trim_end_matches([')', '.']).to_string())
        .expect("run --sandbox prints a run id");
    let events = read_events(&as_home, &run_id);
    eprintln!("--- run {run_id} events ---\n{events:#?}");

    // The gateway recorded the denied tool call in the run's own log.
    let denied = events.iter().any(|e| {
        matches!(
            e,
            RunEvent::ToolCall { server, tool, outcome, .. }
                if server == "figma" && tool == "post_comment" && outcome == "denied"
        )
    });
    assert!(
        denied,
        "the run log must carry the DENIED tool call recorded by the gateway: {events:?}"
    );
}

/// LOCKDOWN: the container has NO host route — it reaches the host gateway only
/// through the sidecar's fixed-destination relay. Proves the relay bridges an
/// internal-only network to the host gateway, and tool policy is enforced +
/// recorded there. This is what earns the unqualified "enforced" cell.
#[test]
fn lockdown_routes_denied_tool_through_the_sidecar_relay() {
    if !docker_and_image() {
        return;
    }
    if !build_egress_image() {
        eprintln!("SKIP: cannot build the egress sidecar image");
        return;
    }
    let tmp = assert_fs::TempDir::new().unwrap();
    let (home, as_home) = machine_home(tmp.path());
    let proj = project(tmp.path());
    // Also exercise the shadow under lockdown: a stale direct entry must be
    // neutralized here too.
    fs::write(
        proj.join(".mcp.json"),
        "{\"mcpServers\":{\"evil-direct\":{\"type\":\"http\",\"url\":\"https://evil.invalid/mcp\",\"headers\":{\"Authorization\":\"Bearer STALE-SECRET\"}}}}",
    )
    .unwrap();

    let bin = env!("CARGO_BIN_EXE_agentstack");
    for step in [vec!["lock"], vec!["trust"]] {
        let s = Command::new(bin)
            .args(&step)
            .current_dir(&proj)
            .env("HOME", &home)
            .env("AGENTSTACK_HOME", &as_home)
            .output()
            .unwrap();
        assert!(s.status.success(), "`agentstack {}` failed", step[0]);
    }

    let out = Command::new(bin)
        .args(["run", "--lockdown", "gwtest", "--", "-e", CLIENT_SCRIPT])
        .current_dir(&proj)
        .env("HOME", &home)
        .env("AGENTSTACK_HOME", &as_home)
        .env("AGENTSTACK_SANDBOX_IMAGE", IMAGE)
        .env("AGENTSTACK_EGRESS_IMAGE", EGRESS_IMAGE)
        .output()
        .unwrap();
    let stdout = strip_ansi(&String::from_utf8_lossy(&out.stdout));
    let stderr = strip_ansi(&String::from_utf8_lossy(&out.stderr));
    eprintln!("--- lockdown run stdout ---\n{stdout}\n--- stderr ---\n{stderr}");

    assert!(
        stdout.contains("routed through the gateway"),
        "lockdown bundle should route through the gateway: {stdout}"
    );

    let run_id = stdout
        .split_whitespace()
        .find(|w| w.starts_with("r-"))
        .map(|w| w.trim_end_matches([')', '.']).to_string())
        .expect("run --lockdown prints a run id");
    let events = read_events(&as_home, &run_id);
    eprintln!("--- run {run_id} events ---\n{events:#?}");

    // The denied tool call was refused and recorded — reached the host gateway
    // through the relay despite the container having no direct host route.
    let denied = events.iter().any(|e| {
        matches!(
            e,
            RunEvent::ToolCall { server, tool, outcome, .. }
                if server == "figma" && tool == "post_comment" && outcome == "denied"
        )
    });
    assert!(
        denied,
        "the run log must carry the DENIED tool call routed through the relay: {events:?}"
    );
}
