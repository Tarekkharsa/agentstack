//! The user layer: the machine-level manifest's `[instructions]` merge in
//! beneath every project load — instructions ONLY (never servers/skills),
//! compiled at global scope only, and the project wins a name conflict.

use std::fs;
use std::sync::Mutex;

use agentstack::render::instructions::plan_instructions;
use agentstack::scope::Scope;

// These tests mutate the process-global HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn set_home(tmp: &std::path::Path) -> std::path::PathBuf {
    let home = tmp.join(".agentstack");
    fs::create_dir_all(home.join("instructions")).unwrap();
    std::env::set_var("HOME", tmp);
    std::env::set_var("AGENTSTACK_HOME", &home);
    home
}

fn unset_home() {
    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

#[test]
fn machine_instructions_merge_beneath_project_loads_global_scope_only() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = set_home(tmp.path());

    // The machine layer declares a fragment AND a personal server.
    fs::write(
        home.join("agentstack.toml"),
        "version = 1\n[instructions.style]\npath = \"./instructions/style.md\"\n\
         [servers.personal]\ntype = \"http\"\nurl = \"https://personal/mcp\"\n",
    )
    .unwrap();
    fs::write(home.join("instructions/style.md"), "Machine style.\n").unwrap();

    // A project with its own fragment.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(proj.join("instructions")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
         [instructions.house]\npath = \"./instructions/house.md\"\n",
    )
    .unwrap();
    fs::write(proj.join("instructions/house.md"), "Project rule.\n").unwrap();

    let ctx = agentstack::commands::load(Some(&proj)).unwrap();
    let m = &ctx.loaded.manifest;

    // Machine fragment merged in FIRST, flagged, path re-anchored absolute.
    let names: Vec<&String> = m.instructions.keys().collect();
    assert_eq!(names, ["style", "house"]);
    assert!(m.instructions["style"].from_user_layer);
    assert!(std::path::Path::new(&m.instructions["style"].path).is_absolute());
    assert!(!m.instructions["house"].from_user_layer);
    assert_eq!(
        ctx.loaded.user_path.as_deref(),
        Some(home.join("agentstack.toml").as_path())
    );

    // The personal server did NOT inherit.
    assert!(
        m.servers.is_empty(),
        "machine-layer servers must never auto-inject into a project"
    );

    // Global scope compiles both; project scope only the project's own.
    let reg = agentstack::adapter::Registry::load().unwrap();
    let desc = reg.get("claude-code").unwrap();
    let gp = plan_instructions(m, desc, Scope::Global, &ctx.dir).unwrap();
    assert_eq!(gp.fragments, ["style", "house"]);
    assert!(gp.proposed.contains("Machine style."));
    assert!(gp.proposed.contains("Project rule."));
    let pp = plan_instructions(m, desc, Scope::Project, &ctx.dir).unwrap();
    assert_eq!(pp.fragments, ["house"]);
    assert!(
        !pp.proposed.contains("Machine style."),
        "personal fragments must never compile into a repo's project-scope file"
    );

    unset_home();
}

#[test]
fn project_definition_wins_and_the_layer_never_merges_into_itself() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = set_home(tmp.path());

    fs::write(
        home.join("agentstack.toml"),
        "version = 1\n[instructions.style]\npath = \"./instructions/style.md\"\n",
    )
    .unwrap();
    fs::write(home.join("instructions/style.md"), "Machine style.\n").unwrap();

    // Project redefines the same fragment name → it fully owns it.
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n[instructions.style]\npath = \"./instructions/mine.md\"\n",
    )
    .unwrap();
    let ctx = agentstack::commands::load(Some(&proj)).unwrap();
    let style = &ctx.loaded.manifest.instructions["style"];
    assert!(!style.from_user_layer);
    assert_eq!(style.path, "./instructions/mine.md");
    assert!(
        ctx.loaded.user_path.is_none(),
        "nothing merged → no user layer recorded"
    );

    // Loading the machine layer itself stays single-layer and unflagged.
    let ctx = agentstack::commands::load(Some(tmp.path())).unwrap();
    assert!(ctx.loaded.user_path.is_none());
    assert!(!ctx.loaded.manifest.instructions["style"].from_user_layer);

    unset_home();
}

#[test]
fn future_version_machine_layer_is_skipped_not_fatal() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = set_home(tmp.path());

    // A machine layer written by a future agentstack: same no-op policy as a
    // broken personal file — it must neither be misread nor take every
    // project load down.
    fs::write(
        home.join("agentstack.toml"),
        "version = 99\n[instructions.style]\npath = \"./instructions/style.md\"\n",
    )
    .unwrap();
    fs::write(home.join("instructions/style.md"), "Machine style.\n").unwrap();

    let proj = tmp.path().join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(proj.join("agentstack.toml"), "version = 1\n").unwrap();

    let ctx = agentstack::commands::load(Some(&proj)).unwrap();
    assert!(ctx.loaded.manifest.instructions.is_empty());
    assert!(ctx.loaded.user_path.is_none());

    unset_home();
}
