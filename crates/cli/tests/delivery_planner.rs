// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]
#![cfg(unix)]

//! W4 — the delivery planner, the one override, and the flip
//! (`docs/design/automatic-delivery.md` §"The decision", §"Honesty rules").
//!
//! Five witnesses, one per claim the contract makes about routing:
//!
//! 1. **The matrix holds.** Every kind goes to the lane the matrix names, on an
//!    MCP-capable harness and on one that reads files only — and hooks and
//!    extensions carry the full consent ceremony wherever they land.
//! 2. **Both lanes at once is the normal case**, not an edge case. A project
//!    with one MCP-capable tool and one file-only tool is in both, and each
//!    lane is reported on its own terms.
//! 3. **Render locally really produces files** where the live channel would
//!    have worked — per project, and per harness.
//! 4. **The flip.** With no override, skills and MCP servers on an MCP-capable
//!    harness default to the dynamic lane.
//! 5. **The honesty rules hold against real command output**: no surface says
//!    "0 files", and no surface describes an instruction as going live.
//!
//! `pi` is the file-only harness throughout (no MCP channel; skills,
//! instructions, settings and extensions) and `claude-code` the MCP-capable one
//! (MCP plus skills, instructions, settings and hooks). Both are shipped
//! descriptors, so the matrix is exercised against the real registry rather
//! than a fixture that could drift from it.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde_json::Value;

use agentstack::adapter::Registry;
use agentstack::delivery::{route, Kind, Lane, Plan, Reason};
use agentstack_core::manifest::Delivery;

const BIN: &str = env!("CARGO_BIN_EXE_agentstack");

const MCP_HARNESS: &str = "claude-code";
const FILE_ONLY_HARNESS: &str = "pi";

fn registry() -> Registry {
    Registry::load().expect("the shipped adapter registry loads")
}

fn plan_for(delivery: &Delivery, ids: &[&str]) -> Plan {
    let ids: Vec<String> = ids.iter().map(|s| s.to_string()).collect();
    Plan::build(delivery, &registry(), &ids)
}

fn lane_of(plan: &Plan, harness: &str, kind: Kind) -> Option<Lane> {
    plan.harnesses
        .iter()
        .find(|h| h.id == harness)?
        .routes
        .iter()
        .find(|r| r.kind == kind)
        .map(|r| r.lane)
}

// ─────────────────────────────────────────────────────────── 1. the matrix

#[test]
fn the_planner_routes_each_kind_to_its_lane() {
    let plan = plan_for(&Delivery::default(), &[MCP_HARNESS, FILE_ONLY_HARNESS]);

    // MCP-capable: skills and servers live, everything else to files.
    assert_eq!(
        lane_of(&plan, MCP_HARNESS, Kind::Skill),
        Some(Lane::Dynamic)
    );
    assert_eq!(
        lane_of(&plan, MCP_HARNESS, Kind::Server),
        Some(Lane::Dynamic)
    );
    assert_eq!(
        lane_of(&plan, MCP_HARNESS, Kind::Instruction),
        Some(Lane::Rendered),
        "MCP cannot inject an instruction"
    );
    assert_eq!(
        lane_of(&plan, MCP_HARNESS, Kind::Setting),
        Some(Lane::Rendered)
    );
    assert_eq!(
        lane_of(&plan, MCP_HARNESS, Kind::Hook),
        Some(Lane::Rendered),
        "hooks are an executable kind and never leave the rendered lane"
    );

    // Extensions: `pi` is the harness that carries them, and it is file-only —
    // so assert the executable-kind rule on the routing function directly, in
    // the one combination the registry cannot supply (MCP-capable + extensions).
    let ext = route(Kind::Extension, true, false);
    assert_eq!(ext.lane, Lane::Rendered);
    assert_eq!(ext.reason, Reason::ExecutableKind);

    // Every kind, on a harness that reads files only, renders — automatically.
    let file_only = plan
        .harnesses
        .iter()
        .find(|h| h.id == FILE_ONLY_HARNESS)
        .expect("pi is in the plan");
    assert!(!file_only.mcp_capable);
    assert!(
        !file_only.routes.is_empty(),
        "pi must carry some kinds, or this asserts nothing"
    );
    for r in &file_only.routes {
        assert_eq!(r.lane, Lane::Rendered, "{:?} on a file-only tool", r.kind);
        assert_eq!(r.reason, Reason::NoLiveChannel);
    }

    // The ceremony travels with the kind, not with the lane or the harness.
    for mcp in [true, false] {
        for local in [true, false] {
            for kind in [Kind::Hook, Kind::Extension] {
                let r = route(kind, mcp, local);
                assert_eq!(r.lane, Lane::Rendered);
                assert!(
                    r.full_ceremony(),
                    "{kind:?} lost its ceremony at mcp={mcp} render_locally={local}"
                );
            }
        }
    }
    for kind in [Kind::Skill, Kind::Server, Kind::Instruction, Kind::Setting] {
        assert!(!route(kind, true, false).full_ceremony(), "{kind:?}");
    }
}

// ────────────────────────────────────────────────── 2. both lanes at once

#[test]
fn a_project_can_be_in_both_lanes_at_once() {
    let plan = plan_for(&Delivery::default(), &[MCP_HARNESS, FILE_ONLY_HARNESS]);

    assert!(plan.has_dynamic_lane(), "skills and servers go live");
    assert!(
        plan.has_rendered_lane(),
        "house rules, settings and the file-only tool are written"
    );

    // The same project, both lanes — and the MCP-capable tool is itself in
    // both, which is what makes this the normal case and not a split by tool.
    let claude = plan
        .harnesses
        .iter()
        .find(|h| h.id == MCP_HARNESS)
        .expect("claude-code is in the plan");
    assert!(!claude.kinds_in(Lane::Dynamic).is_empty());
    assert!(!claude.kinds_in(Lane::Rendered).is_empty());

    // The rendered lane is reported on its own line, naming what is written and
    // where — never blended into the live claim.
    let rendered = agentstack::delivery::rendered_lane_line(&plan)
        .expect("a project with house rules has a rendered lane");
    assert!(rendered.starts_with("rendered lane:"), "{rendered}");
    assert!(rendered.contains("Claude Code"), "{rendered}");
    assert!(!rendered.contains("served live"), "{rendered}");
}

// ──────────────────────────────────────────────────── 3. render locally

/// A project whose manifest declares one server, one skill and one house rule —
/// enough for both lanes to have something in them.
fn project(dir: &Path, delivery_block: &str) -> PathBuf {
    let root = dir.join("proj");
    let manifest_dir = root.join(".agentstack");
    std::fs::create_dir_all(&manifest_dir).unwrap();
    std::fs::write(
        manifest_dir.join("agentstack.toml"),
        format!(
            r#"version = 1

[servers.search]
type = "http"
url = "https://example.invalid/mcp"

[targets]
default = ["claude-code"]
{delivery_block}"#
        ),
    )
    .unwrap();
    root
}

fn run(args: &[&str], cwd: &Path, home: &Path) -> String {
    let out = Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("NO_COLOR", "1")
        .output()
        .unwrap();
    format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    )
}

fn home_in(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    std::fs::create_dir_all(&home).unwrap();
    home
}

fn routing_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("not JSON ({e}):\n{text}"))
}

#[test]
fn render_locally_writes_files_where_the_lease_would_have_worked() {
    // ── per project ──────────────────────────────────────────────────────
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = home_in(tmp.path());

    // Automatic first, so the difference is the override and nothing else.
    let auto = project(&tmp.path().join("a"), "");
    let out = run(&["delivery", "--json"], &auto, &home);
    let body = routing_json(&out);
    let claude = &body["harnesses"][0];
    assert_eq!(claude["id"], MCP_HARNESS);
    assert_eq!(claude["render_locally"], false);
    assert_eq!(claude["override"], "none");
    let lane = |v: &Value, kind: &str| -> String {
        v["routes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["kind"] == kind)
            .unwrap_or_else(|| panic!("no route for {kind} in {v}"))["lane"]
            .as_str()
            .unwrap()
            .to_string()
    };
    assert_eq!(lane(claude, "servers"), "dynamic");
    assert_eq!(lane(claude, "skills"), "dynamic");

    // Now the same project with the project-wide override.
    let local = project(
        &tmp.path().join("b"),
        "\n[delivery]\nrender_locally = true\n",
    );
    let body = routing_json(&run(&["delivery", "--json"], &local, &home));
    let claude = &body["harnesses"][0];
    assert_eq!(claude["render_locally"], true);
    assert_eq!(claude["override"], "project");
    assert_eq!(lane(claude, "servers"), "rendered");
    assert_eq!(lane(claude, "skills"), "rendered");

    // And the files really appear: the escape hatch exists to produce them.
    let applied = run(&["apply", "--write", "--allow-unresolved"], &local, &home);
    assert!(
        local.join(".mcp.json").exists(),
        "render locally must put the server in a native file:\n{applied}"
    );

    // ── per harness ──────────────────────────────────────────────────────
    // One tool pinned to files inside an otherwise automatic project: the
    // override is per harness, so the answer must differ by harness.
    let per_harness = project(
        &tmp.path().join("c"),
        "\n[delivery.harness.claude-code]\nrender_locally = true\n",
    );
    let body = routing_json(&run(&["delivery", "--json"], &per_harness, &home));
    let claude = &body["harnesses"][0];
    assert_eq!(claude["override"], "harness");
    assert_eq!(lane(claude, "servers"), "rendered");

    // The other direction: a project-wide override that one harness opts out
    // of. The most specific answer wins, and it can point either way.
    let mixed = project(
        &tmp.path().join("d"),
        "\n[delivery]\nrender_locally = true\n\n[delivery.harness.claude-code]\nrender_locally = false\n",
    );
    let body = routing_json(&run(&["delivery", "--json"], &mixed, &home));
    let claude = &body["harnesses"][0];
    assert_eq!(claude["render_locally"], false, "the harness entry wins");
    assert_eq!(lane(claude, "servers"), "dynamic");

    // The command that sets it is the same shape the manifest carries.
    let set = run(&["delivery", "render-locally", "--write"], &auto, &home);
    assert!(set.contains("render locally recorded"), "{set}");
    let text = std::fs::read_to_string(auto.join(".agentstack/agentstack.toml")).unwrap();
    assert!(text.contains("[delivery]"), "{text}");
    assert!(text.contains("render_locally = true"), "{text}");
    // Clearing it removes the key rather than writing `false` — automatic is
    // the absence of an override, not a second stored value.
    let off = run(
        &["delivery", "render-locally", "--off", "--write"],
        &auto,
        &home,
    );
    assert!(off.contains("automatic again"), "{off}");
    let text = std::fs::read_to_string(auto.join(".agentstack/agentstack.toml")).unwrap();
    assert!(!text.contains("render_locally"), "{text}");
}

// ─────────────────────────────────────────────────────────── 4. the flip

#[test]
fn the_default_is_dynamic_for_skills_and_servers_on_an_mcp_capable_harness() {
    // At the routing function: no override, MCP-capable — dynamic.
    for kind in [Kind::Skill, Kind::Server] {
        let r = route(kind, true, false);
        assert_eq!(
            r.lane,
            Lane::Dynamic,
            "{kind:?} must default to the dynamic lane after the flip"
        );
        assert_eq!(r.reason, Reason::Routed);
    }

    // And through a real project with a manifest that says nothing about
    // delivery — the default has to be the routed one, not a stored setting.
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = home_in(tmp.path());
    let proj = project(tmp.path(), "");
    let text = std::fs::read_to_string(proj.join(".agentstack/agentstack.toml")).unwrap();
    assert!(
        !text.contains("delivery"),
        "the fixture must carry no override, or this proves nothing:\n{text}"
    );

    let body = routing_json(&run(&["delivery", "--json"], &proj, &home));
    assert_eq!(body["default"], "automatic");
    let routes = body["harnesses"][0]["routes"].as_array().unwrap();
    for kind in ["skills", "servers"] {
        let r = routes.iter().find(|r| r["kind"] == kind).unwrap();
        assert_eq!(r["lane"], "dynamic", "{kind} is not dynamic by default");
    }
    // Instructions and settings stay rendered whatever the default is: no
    // channel would carry them.
    for kind in ["instructions", "settings"] {
        let r = routes.iter().find(|r| r["kind"] == kind).unwrap();
        assert_eq!(
            r["lane"], "rendered",
            "{kind} must stay in the rendered lane"
        );
    }

    // The contract advertises the reading.
    assert!(agentstack::ui_contract::FEATURES.contains(&"delivery-routing-v1"));
}

// ─────────────────────────────────────────────────── 5. the honesty rules

#[test]
fn no_surface_claims_zero_files_or_calls_an_instruction_gateway_delivered() {
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = home_in(tmp.path());
    let proj = project(tmp.path(), "");

    // Real output from the surfaces that report delivery.
    let mut transcript = String::new();
    for args in [vec!["delivery"], vec!["init", "--dry-run"], vec!["status"]] {
        transcript.push_str(&run(&args, &proj, &home));
    }

    let lower = transcript.to_lowercase();
    // Rule 1: never a bare "0 files" / "no files" / "nothing on disk".
    for banned in ["0 files", "zero files", "no files", "nothing on disk"] {
        assert!(
            !lower.contains(banned),
            "a surface claimed '{banned}':\n{transcript}"
        );
    }
    // The sanctioned sentence is the one that appears instead, and it names
    // what stays behind.
    assert!(
        transcript.contains("0 project artifacts"),
        "the honest zero-artifacts sentence is missing:\n{transcript}"
    );
    assert!(
        transcript.contains("manifest and lock"),
        "the zero-artifacts sentence must name what stays:\n{transcript}"
    );

    // Rule 2: the rendered lane gets its own line, naming what is written.
    assert!(
        transcript.contains("rendered lane:"),
        "a surface reporting both lanes must carry a separate rendered-lane line:\n{transcript}"
    );

    // Rule 3: an instruction is never described as going live. Check every
    // clause that makes a live claim — the clause, not the whole line, because
    // a harness row legitimately reports both lanes side by side.
    for line in transcript.lines() {
        for clause in line.split(" · ") {
            if !clause.contains("served live") {
                continue;
            }
            for file_only in ["house rules", "settings", "hooks", "extensions"] {
                assert!(
                    !clause.contains(file_only),
                    "'{file_only}' described as served live: {clause}"
                );
            }
        }
    }
    // The word "gateway" never appears attached to an instruction anywhere.
    for line in transcript.lines() {
        let l = line.to_lowercase();
        if l.contains("gateway") {
            assert!(
                !l.contains("house rule") && !l.contains("instruction"),
                "an instruction was described through the gateway: {line}"
            );
        }
    }
}
