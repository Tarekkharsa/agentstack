//! End-to-end tests for the vendor-packs rail (PLAN §9 — Linear pack MVP).
//!
//! These drive the real `add`/`remove` command flow against a temp manifest dir
//! (the embedded catalog + bundled assets are compiled in, so resolution and
//! asset extraction work regardless of cwd) and the library `search`/`sync`/
//! instruction-render paths. A pack is an install-time composition: after `add`
//! each member rides its normal manifest section, so the existing rails see it
//! for free.

use std::fs;
use std::path::Path;

use agentstack::adapter::Registry;
use agentstack::cli::{AddArgs, AddFromArgs, AddKind, RemoveArgs, UpgradeArgs};
use agentstack::manifest::Manifest;
use agentstack::plugin_recipes::{self, SyncOptions};
use agentstack::provider::{self, CandidateKind};
use agentstack::render::instructions::plan_instructions;
use agentstack::scope::Scope;

/// Seed a temp dir with an `agentstack.toml` and return the dir + manifest path.
fn seed(body: &str) -> (assert_fs::TempDir, std::path::PathBuf) {
    let tmp = assert_fs::TempDir::new().unwrap();
    let path = tmp.path().join("agentstack.toml");
    fs::write(&path, body).unwrap();
    (tmp, path)
}

/// Load the manifest fresh from `dir` (re-reads the file on disk).
fn load(dir: &Path) -> Manifest {
    agentstack::manifest::load_from_dir(dir).unwrap().manifest
}

fn add_args(id: &str, with_instructions: bool, write: bool) -> AddArgs {
    AddArgs {
        kind: AddKind::From(AddFromArgs {
            id: id.to_string(),
            profile: None,
            with_instructions,
            write,
        }),
    }
}

fn upgrade_args(name: &str, with_instructions: bool, yes: bool, write: bool) -> UpgradeArgs {
    UpgradeArgs {
        name: Some(name.to_string()),
        all: false,
        with_instructions,
        yes,
        write,
    }
}

#[test]
fn add_pack_writes_all_four_sections_with_instructions() {
    let (tmp, path) = seed("version = 1\n");
    let dir = tmp.path();

    agentstack::commands::add::run(&add_args("linear-pack", true, true), Some(dir)).unwrap();

    let m = load(dir);
    // 1. Server (vendor-named), secret lifted to a ${REF} (never a literal).
    let server = m.servers.get("linear-pack").expect("server written");
    assert_eq!(server.url.as_deref(), Some("https://mcp.linear.app/mcp"));
    assert_eq!(
        server.headers.get("Authorization").map(String::as_str),
        Some("Bearer ${LINEAR_PACK_TOKEN}")
    );

    // 2. Skill, extracted under the manifest dir.
    let skill = m.skills.get("linear_breakdown").expect("skill written");
    assert_eq!(skill.path.as_deref(), Some("./skills/linear/breakdown"));
    assert!(dir.join("skills/linear/breakdown/SKILL.md").exists());

    // 3. Instruction (opt-in). Flat name on disk, marker-stamped.
    let instr = m
        .instructions
        .get("linear_rules")
        .expect("instruction written");
    assert_eq!(instr.path, "./instructions/linear_rules.md");
    let instr_body = fs::read_to_string(dir.join("instructions/linear_rules.md")).unwrap();
    assert!(instr_body.starts_with("<!-- agentstack:vendor linear-pack (unofficial) -->"));

    // 4. Ledger with kind = "pack" recording every member.
    let ledger = m.plugins.get("linear-pack").expect("ledger written");
    assert_eq!(ledger.kind.as_deref(), Some("pack"));
    assert_eq!(ledger.source.as_deref(), Some("catalog:linear-pack"));
    assert_eq!(ledger.servers, vec!["linear-pack".to_string()]);
    assert_eq!(ledger.skills, vec!["linear_breakdown".to_string()]);
    assert_eq!(ledger.instructions, vec!["linear_rules".to_string()]);

    // No raw secret ever lands in the manifest text.
    let raw = fs::read_to_string(&path).unwrap();
    assert!(!raw.to_lowercase().contains("bearer lin"));
}

#[test]
fn instructions_are_only_written_with_the_opt_in_flag() {
    let (tmp, _path) = seed("version = 1\n");
    let dir = tmp.path();

    // Without --with-instructions: server + skill land, the instruction does not.
    agentstack::commands::add::run(&add_args("linear-pack", false, true), Some(dir)).unwrap();

    let m = load(dir);
    assert!(m.servers.contains_key("linear-pack"));
    assert!(m.skills.contains_key("linear_breakdown"));
    assert!(
        !m.instructions.contains_key("linear_rules"),
        "house rules must be opt-in"
    );
    assert!(!dir.join("instructions/linear_rules.md").exists());

    // The ledger records no instruction either.
    let ledger = m.plugins.get("linear-pack").unwrap();
    assert!(ledger.instructions.is_empty());
}

#[test]
fn pack_install_is_refused_atomically_on_policy_violation() {
    // forbid one of the pack's members → the whole install must bail with NOTHING
    // written: no server, no skill, no ledger, no extracted asset on disk.
    let (tmp, path) = seed(
        r#"
        version = 1
        [policy]
        forbid = ["linear_breakdown"]
        "#,
    );
    let dir = tmp.path();
    let before = fs::read_to_string(&path).unwrap();

    let err = agentstack::commands::add::run(&add_args("linear-pack", true, true), Some(dir))
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("linear_breakdown"), "names the member: {msg}");
    assert!(msg.contains("forbid"), "names the rule: {msg}");

    // Atomic: the manifest is byte-identical and nothing was extracted.
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    let m = load(dir);
    assert!(m.servers.is_empty());
    assert!(m.skills.is_empty());
    assert!(m.plugins.is_empty());
    assert!(!dir.join("skills/linear/breakdown").exists());
    assert!(!dir.join("instructions/linear_rules.md").exists());
}

#[test]
fn pack_members_surface_in_doctor_machinery() {
    // After install, the pack's server secret + skill are visible through the very
    // same accessors doctor uses (referenced_secrets / skills) — no special-casing.
    let (tmp, _path) = seed("version = 1\n");
    let dir = tmp.path();
    agentstack::commands::add::run(&add_args("linear-pack", false, true), Some(dir)).unwrap();

    let m = load(dir);
    assert!(m
        .referenced_secrets()
        .contains(&"LINEAR_PACK_TOKEN".to_string()));
    assert!(m.skills.contains_key("linear_breakdown"));
}

#[test]
fn search_surfaces_pack_and_standalone_skill_with_correct_kinds() {
    // The catalog provider emits the pack with its composition and the standalone
    // skill with its source — these feed `search`'s [pack]/[skill] badges.
    let pack = provider::search_all("linear", 25)
        .into_iter()
        .find(|c| c.name == "linear-pack")
        .expect("pack surfaced");
    match &pack.kind {
        CandidateKind::Pack(spec) => {
            assert!(spec.server.is_some());
            assert_eq!(spec.skills.len(), 1);
            assert_eq!(spec.instructions.len(), 1);
        }
        other => panic!("expected a pack, got {other:?}"),
    }
    // Aggregated trust: the http+Authorization server needs a secret, runs no code.
    let t = pack.trust();
    assert!(t.needs_secret);
    assert!(!t.runs_code);

    let skill = provider::search_all("triage", 25)
        .into_iter()
        .find(|c| c.name == "pr-triage")
        .expect("standalone skill surfaced");
    assert!(matches!(skill.kind, CandidateKind::Skill(_)));
}

#[test]
fn pack_instruction_carries_provenance_into_rendered_region() {
    // Install with house rules, then render the Claude instruction region: the
    // vendor provenance marker + heading must survive the merge so origin is
    // visible in the daily-driver agent's CLAUDE.md.
    let (tmp, _path) = seed("version = 1\n");
    let dir = tmp.path();
    agentstack::commands::add::run(&add_args("linear-pack", true, true), Some(dir)).unwrap();

    let m = load(dir);
    let reg = Registry::load().unwrap();
    let claude = reg.get("claude-code").unwrap();
    let plan = plan_instructions(&m, claude, Scope::Global, dir).unwrap();

    assert!(plan.fragments.contains(&"linear_rules".to_string()));
    assert!(plan
        .proposed
        .contains("<!-- agentstack:vendor linear-pack (unofficial) -->"));
    assert!(plan.proposed.contains("# vendor: linear-pack (unofficial)"));
}

#[test]
fn remove_pack_fully_reverses_install_and_spares_user_files() {
    let (tmp, path) = seed("version = 1\n");
    let dir = tmp.path();
    agentstack::commands::add::run(&add_args("linear-pack", true, true), Some(dir)).unwrap();

    // A user-authored instruction file (no vendor marker) must be left untouched.
    let user_instr = dir.join("instructions/mine.md");
    fs::create_dir_all(user_instr.parent().unwrap()).unwrap();
    fs::write(&user_instr, "# my own rules\n").unwrap();

    // Sanity: everything is present before removal.
    let pack_instr = dir.join("instructions/linear_rules.md");
    assert!(pack_instr.exists());

    let remove = RemoveArgs {
        name: "linear-pack".into(),
        write: true,
    };
    agentstack::commands::remove::run(&remove, Some(dir)).unwrap();

    let m = load(dir);
    assert!(m.servers.is_empty(), "server removed");
    assert!(m.skills.is_empty(), "skill removed");
    assert!(m.instructions.is_empty(), "instruction removed");
    assert!(m.plugins.is_empty(), "ledger dropped");

    // The pack-written instruction file is deleted; the user's is spared.
    assert!(!pack_instr.exists(), "pack instruction file deleted");
    assert!(user_instr.exists(), "user-authored file untouched");
    assert_eq!(fs::read_to_string(&user_instr).unwrap(), "# my own rules\n");

    // The remaining manifest still parses.
    let _: Manifest = toml::from_str(&fs::read_to_string(&path).unwrap())
        .expect("manifest still parses after removal");
}

#[test]
fn pack_ledger_is_invisible_to_plugins_sync() {
    // A pack ledger is an install record, not a publishable plugin: `sync` must
    // not render it as a native plugin package nor report it as a recipe.
    let (tmp, _path) = seed(
        r#"
        version = 1

        [servers.linear-pack]
        type = "http"
        url = "https://mcp.linear.app/mcp"
        headers = { Authorization = "Bearer ${LINEAR_PACK_TOKEN}" }

        [skills.linear_breakdown]
        path = "./skills/linear/breakdown"

        [plugins.linear-pack]
        kind = "pack"
        version = "0.1.0"
        description = "Linear pack"
        servers = ["linear-pack"]
        skills = ["linear_breakdown"]
        "#,
    );
    let dir = tmp.path();
    let m = load(dir);
    let reg = Registry::load().unwrap();

    // statuses() (what `plugins list/status` build on) omits the pack ledger.
    let statuses = plugin_recipes::statuses(&m, &reg, dir);
    assert!(
        statuses.iter().all(|s| s.name != "linear-pack"),
        "pack ledger must not appear as a recipe status"
    );

    // sync() renders nothing for it and reports no recipes / no package dir.
    let report = plugin_recipes::sync(
        &m,
        &reg,
        dir,
        &SyncOptions {
            targets: vec![],
            write: true,
        },
    )
    .unwrap();
    assert!(report.recipes.is_empty(), "no recipe rendered for the pack");
    // No native plugin package directory is rendered for the pack ledger (sync may
    // still touch empty marketplace manifests, but never a per-pack package).
    assert!(
        report
            .changed
            .iter()
            .all(|p| !p.ends_with("plugins/agentstack/linear-pack")),
        "pack ledger must not render a package: {:?}",
        report.changed
    );
    assert!(
        !dir.join("plugins/agentstack/linear-pack").exists(),
        "no native package dir created for a pack ledger"
    );
}

#[test]
fn standalone_bundled_skill_extracts_its_asset() {
    // A `kind: skill` catalog entry is an embedded asset: `add from` must both
    // write the manifest entry AND extract the bundled SKILL.md, with the path
    // pointing at the extracted copy (not a dangling catalog-relative path).
    let (tmp, _path) = seed("version = 1\n");
    let dir = tmp.path();

    agentstack::commands::add::run(&add_args("pr-triage", false, true), Some(dir)).unwrap();

    let m = load(dir);
    let skill = m.skills.get("pr-triage").expect("skill written");
    assert_eq!(skill.path.as_deref(), Some("./skills/pr-triage"));
    assert!(
        dir.join("skills/pr-triage/SKILL.md").exists(),
        "bundled asset extracted under the manifest dir"
    );
}

#[test]
fn upgrade_noop_reports_already_current_and_leaves_manifest_identical() {
    // With the embedded catalog, re-resolving an installed pack yields identical
    // content: upgrade must be a clean no-op (byte-identical manifest, exit Ok).
    let (tmp, path) = seed("version = 1\n");
    let dir = tmp.path();
    agentstack::commands::add::run(&add_args("linear-pack", true, true), Some(dir)).unwrap();

    let before = fs::read_to_string(&path).unwrap();
    agentstack::commands::upgrade::run(&upgrade_args("linear-pack", false, false, true), Some(dir))
        .unwrap();
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        before,
        "no-op upgrade must not touch the manifest"
    );
}

#[test]
fn upgrade_gates_instruction_body_change_until_accepted() {
    // House rules steer the daily-driver agent: an instruction-body change must be
    // refused (nothing written) until the user passes --with-instructions/--yes.
    let (tmp, _path) = seed("version = 1\n");
    let dir = tmp.path();
    agentstack::commands::add::run(&add_args("linear-pack", true, true), Some(dir)).unwrap();

    let instr = dir.join("instructions/linear_rules.md");
    let tampered = format!("{}\nTAMPERED\n", fs::read_to_string(&instr).unwrap());
    fs::write(&instr, &tampered).unwrap();

    // Without acceptance: the steering change is gated; the file is left as-is.
    agentstack::commands::upgrade::run(&upgrade_args("linear-pack", false, false, true), Some(dir))
        .unwrap();
    assert_eq!(
        fs::read_to_string(&instr).unwrap(),
        tampered,
        "gated upgrade must not overwrite the instruction file"
    );

    // With --with-instructions: the vendor body is re-stamped, dropping the edit.
    agentstack::commands::upgrade::run(&upgrade_args("linear-pack", true, false, true), Some(dir))
        .unwrap();
    let restored = fs::read_to_string(&instr).unwrap();
    assert!(restored.starts_with("<!-- agentstack:vendor linear-pack (unofficial) -->"));
    assert!(!restored.contains("TAMPERED"), "accepted upgrade re-stamps the body");
}

#[test]
fn upgrade_repins_lock_for_pack_skills() {
    // Drift a skill on disk (delete its extracted dir): upgrade re-extracts it and
    // re-pins the lockfile with a checksum row for the skill.
    let (tmp, _path) = seed("version = 1\n");
    let dir = tmp.path();
    agentstack::commands::add::run(&add_args("linear-pack", false, true), Some(dir)).unwrap();

    fs::remove_dir_all(dir.join("skills/linear/breakdown")).unwrap();

    agentstack::commands::upgrade::run(&upgrade_args("linear-pack", false, false, true), Some(dir))
        .unwrap();

    assert!(
        dir.join("skills/linear/breakdown/SKILL.md").exists(),
        "drifted skill re-extracted"
    );
    let lock = fs::read_to_string(dir.join("agentstack.lock")).expect("lockfile written");
    assert!(lock.contains("linear_breakdown"), "skill re-pinned in lock");
    assert!(lock.contains("checksum"), "lock row carries a checksum");
}

#[test]
fn upgrade_rejects_non_pack_and_missing_ledger() {
    // A plain server recipe (not kind = "pack") and an unknown name both bail.
    let (tmp, _path) = seed(
        r#"
        version = 1
        [plugins.play]
        version = "1"
        description = "not a pack"
        "#,
    );
    let dir = tmp.path();

    let err = agentstack::commands::upgrade::run(&upgrade_args("play", false, false, true), Some(dir))
        .unwrap_err();
    assert!(err.to_string().contains("not an installed vendor pack"));

    let err = agentstack::commands::upgrade::run(&upgrade_args("ghost", false, false, true), Some(dir))
        .unwrap_err();
    assert!(err.to_string().contains("not an installed vendor pack"));
}

#[test]
fn upgrade_refuses_when_resolved_member_now_forbidden() {
    // Install a pack, then forbid one of its members. Re-resolving on upgrade must
    // re-gate against [policy] and bail atomically before writing anything.
    let (tmp, path) = seed("version = 1\n");
    let dir = tmp.path();
    agentstack::commands::add::run(&add_args("linear-pack", false, true), Some(dir)).unwrap();

    let with_policy = format!(
        "{}\n[policy]\nforbid = [\"linear_breakdown\"]\n",
        fs::read_to_string(&path).unwrap()
    );
    fs::write(&path, &with_policy).unwrap();
    let before = fs::read_to_string(&path).unwrap();

    let err = agentstack::commands::upgrade::run(&upgrade_args("linear-pack", false, false, true), Some(dir))
        .unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("linear_breakdown"), "names the member: {msg}");
    assert!(msg.contains("forbid"), "names the rule: {msg}");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        before,
        "refused upgrade leaves the manifest untouched"
    );
}

#[test]
fn new_vendor_packs_surface_in_search_with_correct_composition() {
    // Cloudflare (stdio → runs code) and PostHog (http+Authorization → needs
    // secret) both surface as packs with their full composition.
    let cf = provider::search_all("cloudflare", 25)
        .into_iter()
        .find(|c| c.name == "cloudflare-pack")
        .expect("cloudflare pack surfaced");
    match &cf.kind {
        CandidateKind::Pack(spec) => {
            assert!(spec.server.is_some());
            assert_eq!(spec.skills.len(), 2);
            assert_eq!(spec.instructions.len(), 1);
        }
        other => panic!("expected a pack, got {other:?}"),
    }
    assert!(cf.trust().runs_code, "stdio (npx) server runs code");

    let ph = provider::search_all("posthog", 25)
        .into_iter()
        .find(|c| c.name == "posthog-pack")
        .expect("posthog pack surfaced");
    match &ph.kind {
        CandidateKind::Pack(spec) => {
            assert!(spec.server.is_some());
            assert_eq!(spec.skills.len(), 2);
            assert_eq!(spec.instructions.len(), 1);
        }
        other => panic!("expected a pack, got {other:?}"),
    }
    assert!(ph.trust().needs_secret, "http+Authorization needs a secret");
}

#[test]
fn new_vendor_packs_install_and_remove_cleanly() {
    // The new packs ride the exact same rail: install writes all members + ledger
    // and extracts assets; remove fully reverses it.
    let (tmp, _path) = seed("version = 1\n");
    let dir = tmp.path();

    agentstack::commands::add::run(&add_args("cloudflare-pack", true, true), Some(dir)).unwrap();
    let m = load(dir);
    assert!(m.servers.contains_key("cloudflare-pack"));
    assert!(m.skills.contains_key("cloudflare_ship_worker"));
    assert!(m.skills.contains_key("cloudflare_wrangler_cheatsheet"));
    assert!(dir.join("skills/cloudflare/ship-worker/SKILL.md").exists());
    assert!(dir.join("instructions/cloudflare_rules.md").exists());

    let remove = RemoveArgs {
        name: "cloudflare-pack".into(),
        write: true,
    };
    agentstack::commands::remove::run(&remove, Some(dir)).unwrap();
    let m = load(dir);
    assert!(m.servers.is_empty() && m.skills.is_empty() && m.plugins.is_empty());
    assert!(!dir.join("instructions/cloudflare_rules.md").exists());
}

#[test]
fn pack_install_refuses_to_clobber_an_existing_on_disk_dir() {
    // The collision check sees disk, not just manifest keys: a pre-existing
    // (user-authored) skill dir at our destination blocks the install atomically.
    let (tmp, path) = seed("version = 1\n");
    let dir = tmp.path();

    let user_skill = dir.join("skills/linear/breakdown");
    fs::create_dir_all(&user_skill).unwrap();
    fs::write(user_skill.join("SKILL.md"), "# my own\n").unwrap();

    let before = fs::read_to_string(&path).unwrap();
    let err = agentstack::commands::add::run(&add_args("linear-pack", true, true), Some(dir))
        .expect_err("install must refuse to overwrite the existing dir");
    assert!(err.to_string().contains("already exists"));

    // Atomic: nothing written to the manifest, and the user's file is untouched.
    assert_eq!(fs::read_to_string(&path).unwrap(), before);
    assert_eq!(
        fs::read_to_string(user_skill.join("SKILL.md")).unwrap(),
        "# my own\n"
    );
}
