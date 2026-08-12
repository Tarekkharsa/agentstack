// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! **The manifest `init` writes must describe the project, not the machine and
//! not agentstack.**
//!
//! A real first run produced a manifest that got four things wrong at once, and
//! each one taught the reader something false about their own project:
//!
//! * It declared `agentstack` as one of its MCP servers, and built
//!   `[toolsets.default] servers = ["agentstack"]` around it. That entry is our
//!   own gateway bridge, found in a harness config because `gateway connect`
//!   put it there — so the project was asking the gateway to serve the gateway.
//! * It opened on `[servers]`, `[skills]`, `[instructions]` — three empty
//!   headings that were serialization artifacts, not documentation, and
//!   answered nothing a first-time reader is asking.
//! * It carried `[settings.claude-code] model = …`, read from the user's
//!   MACHINE-WIDE `~/.claude/settings.json`. In a project manifest that turns a
//!   personal preference into a repo file every teammate renders and commits.
//! * It pinned `[targets]` to all seven detected CLIs, which is precisely what
//!   an ABSENT `[targets]` already means — while freezing one laptop's
//!   inventory into a file that travels.
//!
//! Every test here has a control, because each fix is otherwise satisfiable by
//! emitting less: a manifest that never declares a toolset, never pins targets,
//! or never imports anything would pass the witnesses and be a worse product
//! than the bug.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

struct Run {
    text: String,
    code: i32,
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        if chars.next() != Some('[') {
            continue;
        }
        for f in chars.by_ref() {
            if ('@'..='~').contains(&f) {
                break;
            }
        }
    }
    out
}

fn run(args: &[&str], home: &Path, proj: &Path) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(proj)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        .env("NO_COLOR", "1")
        .stdin(Stdio::null())
        .output()
        .expect("the binary must run");
    Run {
        text: strip_ansi(&format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        )),
        code: out.status.code().expect("the process must exit normally"),
    }
}

/// The bridge exactly as `agentstack x gateway connect --write` registers it:
/// an absolute path to the binary, `mcp --auto-project`. The absolute path is
/// the point — recognizing it cannot depend on the command being the bare word.
const BRIDGE: &str = r#"{"mcpServers":{"agentstack":{"command":"/usr/local/bin/agentstack","args":["mcp","--auto-project"]}}}"#;

/// A fenced HOME and an empty project. `PATH` holds no agent CLI, so detection
/// is decided entirely by the config files each test writes.
fn machine(tmp: &Path, name: &str) -> (PathBuf, PathBuf) {
    let home = tmp.join(format!("{name}-home"));
    let proj = tmp.join(name);
    fs::create_dir_all(home.join(".claude")).unwrap();
    fs::create_dir_all(&proj).unwrap();
    (home, proj)
}

fn manifest_of(proj: &Path) -> String {
    fs::read_to_string(proj.join(".agentstack/agentstack.toml"))
        .expect("init must have written a manifest")
}

// --------------------------------------------------------------- the bridge

/// Our own registration is not the user's setup, and a manifest built around it
/// is a loop.
#[test]
fn the_gateway_bridge_is_never_imported_as_a_server() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = machine(tmp.path(), "bridge-only");
    fs::write(home.join(".claude.json"), BRIDGE).unwrap();

    let init = run(&["init", "--yes"], &home, &proj);
    assert_eq!(init.code, 0, "init runs:\n{}", init.text);
    let manifest = manifest_of(&proj);

    assert!(
        !manifest.contains("agentstack]") && !manifest.contains("\"agentstack\""),
        "the bridge must not be declared as a server — this project would be asking \
         the gateway to serve the gateway:\n{manifest}"
    );
    assert!(
        !manifest.contains("toolsets") && !manifest.contains("default_toolset"),
        "and with no real capability imported there is nothing for a toolset to name, \
         so none is synthesized:\n{manifest}"
    );
    assert!(
        !init.text.contains("Importing 1 MCP server"),
        "the review must not count it as an import either:\n{}",
        init.text
    );
}

/// The control: a REAL server still imports, still lands in the library, and
/// still gets the toolset — with the bridge sitting right next to it in the
/// same config file.
///
/// Without this, the witness above is satisfied by importing no servers at all.
#[test]
fn a_real_server_beside_the_bridge_still_imports_and_still_gets_a_toolset() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = machine(tmp.path(), "real-server");
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{
             "agentstack":{"command":"/usr/local/bin/agentstack","args":["mcp","--auto-project"]},
             "tldraw":{"command":"npx","args":["-y","tldraw-mcp"]}
           }}"#,
    )
    .unwrap();

    let init = run(&["init", "--yes"], &home, &proj);
    assert_eq!(init.code, 0, "init runs:\n{}", init.text);
    let manifest = manifest_of(&proj);

    assert!(
        manifest.contains("tldraw"),
        "the user's own server must still be imported:\n{manifest}"
    );
    assert!(
        manifest.contains("[toolsets.default]") && manifest.contains("default_toolset"),
        "and a toolset that has something to name is still written:\n{manifest}"
    );
    assert!(
        !manifest.contains("\"agentstack\""),
        "while the bridge beside it is still left out:\n{manifest}"
    );
}

/// The exclusion is stated on request, and only on request: it is ours, so
/// there is no decision for the user to make, but a reader auditing the import
/// against their own config must be able to account for every entry in it.
#[test]
fn verbose_names_the_bridge_it_passed_over_and_a_plain_run_does_not() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = machine(tmp.path(), "verbose");
    fs::write(home.join(".claude.json"), BRIDGE).unwrap();

    let quiet = run(&["init", "--yes", "--dry-run"], &home, &proj);
    let loud = run(&["init", "--yes", "--dry-run", "--verbose"], &home, &proj);

    assert!(
        loud.text.contains("gateway bridge"),
        "--verbose must name what it passed over, and why:\n{}",
        loud.text
    );
    assert!(
        !quiet.text.contains("gateway bridge"),
        "and an ordinary run must not — this is not a decision anyone has to make:\n{}",
        quiet.text
    );
}

// ------------------------------------------------------- the empty headings

/// Empty tables are serialization, not documentation. What a first reader wants
/// at that moment is how to put something in the file.
#[test]
fn an_empty_manifest_teaches_instead_of_listing_empty_tables() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = machine(tmp.path(), "empty-tables");
    fs::write(home.join(".claude.json"), BRIDGE).unwrap();

    let init = run(&["init", "--yes"], &home, &proj);
    assert_eq!(init.code, 0, "init runs:\n{}", init.text);
    let manifest = manifest_of(&proj);

    for heading in ["[servers]", "[skills]", "[instructions]", "skills = []"] {
        assert!(
            !manifest.contains(heading),
            "`{heading}` says nothing and answers nothing:\n{manifest}"
        );
    }
    assert!(
        manifest.contains("agentstack add from") && manifest.contains("agentstack lib link"),
        "the file must instead name the commands that put something in it:\n{manifest}"
    );
    // Still a manifest, and still one this tool can read back.
    assert!(
        manifest.contains("version = 1"),
        "the schema version is not optional:\n{manifest}"
    );
    let status = run(&["status"], &home, &proj);
    assert_eq!(
        status.code, 0,
        "and the manifest it wrote must load:\n{}",
        status.text
    );
}

// ------------------------------------------------------------- the settings

/// A model choice read from `~/.claude/settings.json` is a personal, machine-wide
/// preference. Declaring it in the PROJECT manifest would render it into a repo
/// file every teammate commits.
#[test]
fn imported_native_settings_land_in_the_machine_layer_not_the_repo() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = machine(tmp.path(), "settings");
    fs::write(home.join(".claude.json"), BRIDGE).unwrap();
    fs::write(
        home.join(".claude/settings.json"),
        r#"{"model":"opus","theme":"dark"}"#,
    )
    .unwrap();

    let init = run(&["init", "--yes"], &home, &proj);
    assert_eq!(init.code, 0, "init runs:\n{}", init.text);

    let manifest = manifest_of(&proj);
    assert!(
        !manifest.contains("[settings"),
        "a personal preference must not become a project declaration:\n{manifest}"
    );

    let machine_manifest = fs::read_to_string(home.join(".agentstack/agentstack.toml"))
        .expect("the import must have declared the settings in the machine layer");
    assert!(
        machine_manifest.contains("[settings.claude-code]") && machine_manifest.contains("opus"),
        "they belong to the layer that travels with the person:\n{machine_manifest}"
    );
    assert!(
        init.text.contains("machine layer"),
        "and the review must say where they went — a value that moves without a \
         word is the same surprise as one in the wrong place:\n{}",
        init.text
    );
    // The whole import is still one undoable transaction, both halves of it.
    let undo = run(&["x", "restore", "--last", "--write"], &home, &proj);
    assert_eq!(undo.code, 0, "the undo runs:\n{}", undo.text);
    assert!(
        !home.join(".agentstack/agentstack.toml").exists(),
        "undoing the import takes the machine-layer half back too:\n{}",
        undo.text
    );
}

/// The control: a machine layer that already speaks for a CLI is never
/// rewritten by a project's import — the same rule the library-collision path
/// follows for shared state.
#[test]
fn an_existing_machine_setting_is_kept_not_overwritten() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = machine(tmp.path(), "settings-kept");
    fs::write(home.join(".claude.json"), BRIDGE).unwrap();
    fs::write(home.join(".claude/settings.json"), r#"{"model":"opus"}"#).unwrap();
    fs::create_dir_all(home.join(".agentstack")).unwrap();
    fs::write(
        home.join(".agentstack/agentstack.toml"),
        "version = 1\n\n[settings.claude-code]\nmodel = \"sonnet\"\n",
    )
    .unwrap();

    let init = run(&["init", "--yes"], &home, &proj);
    assert_eq!(init.code, 0, "init runs:\n{}", init.text);

    let machine_manifest = fs::read_to_string(home.join(".agentstack/agentstack.toml")).unwrap();
    assert!(
        machine_manifest.contains("sonnet") && !machine_manifest.contains("opus"),
        "the user's own machine-layer answer wins; an import never rewrites it:\n{machine_manifest}"
    );
    assert!(
        init.text.contains("already declares settings"),
        "and the review must say so rather than implying it imported them:\n{}",
        init.text
    );
}

// -------------------------------------------------------------- the targets

/// `[targets]` is a NARROWING. One that restates the detection adds no meaning
/// and pins one laptop's inventory into a file that travels.
#[test]
fn targets_are_omitted_when_they_only_restate_the_detection() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = machine(tmp.path(), "targets-all");
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"tldraw":{"command":"npx","args":["-y","tldraw-mcp"]}}}"#,
    )
    .unwrap();

    let init = run(&["init", "--yes"], &home, &proj);
    assert_eq!(init.code, 0, "init runs:\n{}", init.text);
    let manifest = manifest_of(&proj);

    assert!(
        !manifest.contains("[targets]"),
        "every detected CLI contributed, so this list is exactly what an absent \
         [targets] already resolves to:\n{manifest}"
    );
    // Absent is not "none": the render still reaches the tool.
    let apply = run(&["apply"], &home, &proj);
    assert!(
        apply.text.contains("Claude Code"),
        "an omitted [targets] must still resolve to the detected CLIs:\n{}",
        apply.text
    );
}

/// The control: a real narrowing is still written, because there it carries
/// information nothing else does.
///
/// Codex has a config file (so it is DETECTED) that declares nothing agentstack
/// understands (so it does not CONTRIBUTE). The import targets what contributed
/// — and that fact cannot be recovered from the manifest any other way.
#[test]
fn a_real_narrowing_is_still_pinned() {
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = machine(tmp.path(), "targets-narrow");
    fs::write(
        home.join(".claude.json"),
        r#"{"mcpServers":{"tldraw":{"command":"npx","args":["-y","tldraw-mcp"]}}}"#,
    )
    .unwrap();
    fs::create_dir_all(home.join(".codex")).unwrap();
    fs::write(
        home.join(".codex/config.toml"),
        "# a config that exists and declares nothing we understand\n",
    )
    .unwrap();

    let init = run(&["init", "--yes"], &home, &proj);
    assert_eq!(init.code, 0, "init runs:\n{}", init.text);
    let manifest = manifest_of(&proj);

    assert!(
        manifest.contains("[targets]") && manifest.contains("claude-code"),
        "the import narrowed to the CLI that contributed, and that is not \
         recoverable from detection:\n{manifest}"
    );
    assert!(
        !manifest.contains("\"codex\""),
        "and the detected-but-silent CLI is deliberately not targeted:\n{manifest}"
    );
}
