// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! G9 — the RENDER path's write-time egress refusal leaves evidence.
//!
//! `docs/ENFORCEMENT.md` claimed the host path records its own refusals. Only
//! half of that was true: the secret-scope refusal recorded (at the decision,
//! in `secret::ScopedResolver`) and the gateway-build egress refusal recorded
//! (`gateway.rs`), but the write-time egress check in
//! `render::apply::plan_target_with_servers` — the one an ordinary
//! `agentstack apply` hits, the one whose sentence a user actually reads on
//! their terminal — pushed a string onto `TargetPlan::denied` and continued.
//! The refusal a person saw was the one refusal nobody could look up
//! afterwards, while its gateway twin was filed.
//!
//! So: the same seam, not a second one. `seatbelt::record` for the
//! machine-global audit row and `seatbelt::record_egress_denied` for the
//! run-scoped mirror — the two the gateway already uses — and nothing new on
//! the terminal.
//!
//! Recorded is not prevented. These tests assert that the decision the check
//! made is retrievable, never that anything was stopped at the wire.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::adapter::AdapterDescriptor;
use agentstack::manifest::Manifest;
use agentstack::render::apply::{plan_target, Selection};
use agentstack::scope::Scope;
use agentstack::secret::MapResolver;

// These tests set the process-global HOME/AGENTSTACK_HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup_home(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// Every line of the machine-global audit log.
fn audit_lines(home: &Path) -> Vec<serde_json::Value> {
    let path = home.join(".agentstack/audit/calls.jsonl");
    let Ok(body) = fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// Every event in one run's log.
fn run_events(home: &Path, run: &str) -> Vec<serde_json::Value> {
    let path = home.join(".agentstack/runs").join(run).join("events.jsonl");
    let Ok(body) = fs::read_to_string(path) else {
        return Vec::new();
    };
    body.lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect()
}

/// The egress denials in the audit log, for one server.
fn egress_denials(home: &Path, server: &str) -> Vec<serde_json::Value> {
    audit_lines(home)
        .into_iter()
        .filter(|l| l["tool"] == "egress" && l["outcome"] == "denied" && l["server"] == server)
        .collect()
}

/// A JSON adapter descriptor whose config path is a temp file, so the whole
/// render → merge path runs without touching a real harness config.
fn json_descriptor(config_path: &str) -> AdapterDescriptor {
    let yaml = format!(
        r#"
id: test-json
display: Test JSON
config:
  path: {config_path}
  format: json
mcp:
  location: mcpServers
  fields:
    url: url
    headers: headers
    command: command
    args: args
    env: env
  transport:
    key: type
    http_value: http
    stdio_value: stdio
  secret_mode: literal
"#
    );
    serde_yaml::from_str(&yaml).unwrap()
}

/// The URL carries a credential in its query string — the reason the recorded
/// line must name the HOST and never the URL.
const TOKEN: &str = "leak-me-not-xyz";

fn refused_manifest() -> Manifest {
    toml::from_str(&format!(
        "version = 1\n\
         [policy.egress]\n\
         \"*\" = [\"!evil.example\"]\n\
         [servers.reacher]\n\
         type = \"http\"\n\
         url = \"https://evil.example/mcp?api_key={TOKEN}\"\n"
    ))
    .unwrap()
}

fn plan(
    manifest: &Manifest,
    desc: &AdapterDescriptor,
    proj: &Path,
) -> agentstack::render::TargetPlan {
    plan_target(
        manifest,
        desc,
        &MapResolver::from([]),
        &Selection::All,
        &[],
        Scope::Global,
        proj,
        agentstack::render::PriorTrust::STRICT,
    )
    .unwrap()
    .unwrap()
}

/// The property: a write-time egress refusal lands in the audit log, saying
/// which server and why — and the server is still withheld from the render.
#[test]
fn a_render_time_egress_refusal_leaves_evidence() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    std::env::remove_var("AGENTSTACK_RUN_ID");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let cfg = tmp.path().join("target.json");
    let desc = json_descriptor(cfg.to_str().unwrap());

    let plan = plan(&refused_manifest(), &desc, &proj);

    // STILL REFUSES: the server is dropped from the render entirely, and the
    // caller is told. Evidence is added beside the refusal, never instead.
    assert!(
        !plan.managed.contains(&"reacher".to_string()),
        "an egress-refused server must not be rendered: {:?}",
        plan.managed
    );
    assert!(
        !plan.proposed.contains("evil.example"),
        "the refused server reached the proposed config: {}",
        plan.proposed
    );
    assert_eq!(
        plan.denied.len(),
        1,
        "the refusal must still be reported to the caller: {:?}",
        plan.denied
    );

    // Evidence: exactly one audit row, filed under the egress family.
    let denials = egress_denials(&home, "reacher");
    assert_eq!(
        denials.len(),
        1,
        "the render-path egress refusal must leave exactly one record: {:?}",
        audit_lines(&home)
    );
    let d = &denials[0];
    let detail = d["detail"].as_str().unwrap_or_default();
    // Which server, and why — the same two facts the user read.
    assert!(
        detail.contains("reacher"),
        "the record must name the server: {d}"
    );
    assert!(
        detail.contains("evil.example"),
        "the record must name the host that was refused: {d}"
    );
    assert!(
        detail.contains("policy"),
        "the record must name the rule that refused: {d}"
    );

    // Redaction: a declared URL can carry a token in its userinfo, path, or
    // query. Only the HOST is recorded, so none of the rest can reach the log.
    for line in audit_lines(&home) {
        let s = line.to_string();
        assert!(
            !s.contains(TOKEN),
            "a credential from the declared URL reached the audit log: {line}"
        );
        assert!(
            !s.contains("/mcp?"),
            "the declared URL reached the audit log: {line}"
        );
    }
}

/// The control. A clean render records nothing: the audit log is evidence of
/// refusals, and a family that files a row when nothing was refused is worse
/// than one that files none — it makes every row unreadable.
#[test]
fn a_clean_render_records_nothing() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    std::env::remove_var("AGENTSTACK_RUN_ID");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let cfg = tmp.path().join("target.json");
    let desc = json_descriptor(cfg.to_str().unwrap());

    // Same policy, a host it allows.
    let manifest: Manifest = toml::from_str(
        "version = 1\n\
         [policy.egress]\n\
         \"*\" = [\"!evil.example\"]\n\
         [servers.reacher]\n\
         type = \"http\"\n\
         url = \"https://good.example/mcp\"\n",
    )
    .unwrap();

    let plan = plan(&manifest, &desc, &proj);
    assert!(
        plan.managed.contains(&"reacher".to_string()),
        "the allowed server must render: {:?}",
        plan.denied
    );
    assert!(plan.denied.is_empty(), "{:?}", plan.denied);
    assert!(
        audit_lines(&home).is_empty(),
        "a clean render must leave no denial evidence: {:?}",
        audit_lines(&home)
    );
}

/// The run-scoped mirror: a refusal inside a tracked run is openable with
/// `agentstack report run <id>`, like its gateway twin. Same event variant
/// (`Egress`, `allowed: false`), reached through the same
/// `seatbelt::record_egress_denied` — not a second evidence channel.
#[test]
fn a_refusal_inside_a_run_lands_in_that_runs_event_log() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup_home(&home);
    std::env::set_var("AGENTSTACK_RUN_ID", "r-render-egress01");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let cfg = tmp.path().join("target.json");
    let desc = json_descriptor(cfg.to_str().unwrap());

    let plan = plan(&refused_manifest(), &desc, &proj);
    assert_eq!(plan.denied.len(), 1);

    let events = run_events(&home, "r-render-egress01");
    let egress: Vec<_> = events
        .iter()
        .filter(|e| e["event"] == "egress" && e["allowed"] == false)
        .collect();
    assert_eq!(
        egress.len(),
        1,
        "the render-path refusal must appear in the run's log: {events:?}"
    );
    assert_eq!(egress[0]["server"], "reacher");
    assert_eq!(
        egress[0]["host"], "evil.example",
        "the event identifies the host, not the URL: {}",
        egress[0]
    );
    for e in &events {
        assert!(
            !e.to_string().contains(TOKEN),
            "a credential from the declared URL reached a run event: {e}"
        );
    }

    std::env::remove_var("AGENTSTACK_RUN_ID");
}

/// Recording must never gate the refusal. Proven, not asserted: point
/// `AGENTSTACK_HOME` at a regular FILE, so every attempt to create the audit
/// directory and the run log fails. The refusal must be identical.
#[test]
fn a_failed_write_to_the_log_does_not_soften_the_refusal() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    // A FILE where the recorder expects a directory: nothing under it can be
    // created, so both the audit row and the run event fail to write.
    let blocked = home.join("not-a-dir");
    fs::write(&blocked, "").unwrap();
    std::env::set_var("AGENTSTACK_HOME", &blocked);
    std::env::set_var("AGENTSTACK_RUN_ID", "r-render-egress02");

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    let cfg = tmp.path().join("target.json");
    let desc = json_descriptor(cfg.to_str().unwrap());

    let plan = plan(&refused_manifest(), &desc, &proj);

    assert!(
        !plan.managed.contains(&"reacher".to_string()),
        "a failed log write must not let the refused server render"
    );
    assert!(
        !plan.proposed.contains("evil.example"),
        "a failed log write must not let the refused host reach the config"
    );
    assert_eq!(
        plan.denied.len(),
        1,
        "a failed log write must not swallow the refusal: {:?}",
        plan.denied
    );
    assert!(
        blocked.is_file(),
        "the recorder must not have replaced the blocking file"
    );

    std::env::remove_var("AGENTSTACK_RUN_ID");
    std::env::remove_var("AGENTSTACK_HOME");
}

/// The module invariant, at the source. Making a denial legible must not be a
/// way to acquire permission: the `continue` that drops the server is still
/// the render loop's own, and the recorder it now calls returns `()` — there
/// is no value it could hand back that a future edit might branch on.
#[test]
fn making_the_refusal_evidenced_did_not_make_it_optional() {
    let src = include_str!("../src/render/apply.rs");
    let check = src
        .split("// Write-time egress check (HTTP only)")
        .nth(1)
        .expect("the write-time egress check must exist");
    // Just the check, up to the per-server secret scoping that follows it.
    let check = check
        .split("// Per-server secret scoping")
        .next()
        .expect("the check is followed by secret scoping");
    assert_eq!(
        check.matches("continue;").count(),
        2,
        "both fail-closed drops must survive: {check}"
    );
    assert!(
        !check.contains("return ") && !check.contains('?'),
        "the egress check must not have grown an early exit that skips the drop: {check}"
    );
    let seat = include_str!("../src/seatbelt.rs");
    assert!(
        seat.contains("pub fn record(d: &Denial, project: Option<String>, run: Option<&str>) {"),
        "record must still return () — it is the structural reason evidence \
         cannot become permission"
    );
    assert!(
        seat.contains(
            "pub fn record_egress_denied(run: Option<&str>, server: &str, host: &str, rule: &str) {"
        ),
        "the run-scoped mirror must still return () for the same reason"
    );
}
