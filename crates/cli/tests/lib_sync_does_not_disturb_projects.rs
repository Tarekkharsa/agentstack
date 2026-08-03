// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! W3 — a central library that moves ahead disturbs no project.
//!
//! `docs/design/automatic-delivery.md` §"The reproducibility rule" and
//! §"Update model" rule 1 promise two things a user can check:
//!
//! 1. `lib sync` **changes nothing in any project** — no manifest, no lock, no
//!    trust state, no rendered file;
//! 2. a project **keeps serving its pinned bytes** afterwards, because runtime
//!    reads the content-addressed store by digest and never the live library.
//!
//! The scope of (2) is fixed by
//! `docs/design/pinned-serving-and-library-drift.md`: it is the LIBRARY-sourced,
//! pinned, store-verified case and nothing else. The last test here is that
//! fence — an inline skill whose bytes changed is the project's own content
//! drifting, and still refuses.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::{json, Value};

use agentstack::cli::{LibArgs, LibKind, LibSyncArgs, LockArgs, UseArgs};
use agentstack::commands::lib::{self, add_skill, LibSource};
use agentstack::commands::{lock as lock_cmd, use_profile};
use agentstack::trust::{self, TrustState};

// HOME / AGENTSTACK_HOME are process-global (and inherited by the spawned MCP
// child); serialize every test in this binary.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// The pinned body's marker, and the one the library moves to. Distinct
/// strings so "which bytes were served" is answerable by a substring check.
const PINNED_BODY: &str = "zzpinnedbody";
const NEWER_BODY: &str = "zznewerlibrarybody";

// ---------------------------------------------------------------- fixture --

/// A machine with a central library holding one skill, and a project that
/// selects it through a toolset, pins it, is trusted, and has been activated
/// (so there are rendered artifacts to compare).
struct Machine {
    home: PathBuf,
    ashome: PathBuf,
    lib_home: PathBuf,
    proj: PathBuf,
}

fn git_identity() {
    std::env::set_var("GIT_AUTHOR_NAME", "t");
    std::env::set_var("GIT_AUTHOR_EMAIL", "t@e.st");
    std::env::set_var("GIT_COMMITTER_NAME", "t");
    std::env::set_var("GIT_COMMITTER_EMAIL", "t@e.st");
}

fn git(args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .unwrap();
    assert!(out.status.success(), "git {args:?}: {out:?}");
}

fn use_args() -> UseArgs {
    UseArgs {
        profile: Some("p".into()),
        targets: vec!["claude-code".into()],
        scope: None,
        write: true,
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
        list: false,
        json: false,
        quiet: false,
    }
}

fn sync_args(init: bool, remote: Option<&str>) -> LibArgs {
    LibArgs {
        kind: LibKind::Sync(LibSyncArgs {
            init,
            remote: remote.map(str::to_string),
            status: false,
            message: None,
            allow_secrets: false,
            source: None,
        }),
    }
}

/// `skill_toml` is the manifest's skill declaration: empty for the
/// library-sourced case (the name resolves through `library.toml`), or an
/// inline `[skills.helper]` block for the project-local case.
fn machine(tmp: &Path, skill_toml: &str) -> Machine {
    let home = tmp.join("home");
    let ashome = home.join(".agentstack");
    fs::create_dir_all(&ashome).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", &ashome);

    // The library skill, added through the real `lib add` path so the library
    // index and the on-disk copy agree.
    let lib_home = ashome.join("lib");
    let src = tmp.join("src");
    fs::create_dir_all(&src).unwrap();
    fs::write(
        src.join("SKILL.md"),
        format!("---\nname: helper\ndescription: Helps.\n---\n{PINNED_BODY}\n"),
    )
    .unwrap();
    add_skill(
        &lib_home,
        "helper",
        LibSource::Path(&src),
        false,
        true,
        false,
    )
    .unwrap();

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join("instructions")).unwrap();
    fs::write(proj.join("instructions/house.md"), "Be kind.\n").unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        format!(
            "version = 1\n[targets]\ndefault = [\"claude-code\"]\n\
             [profiles.p]\nskills = [\"helper\"]\n\
             [instructions.house]\npath = \"./instructions/house.md\"\n{skill_toml}"
        ),
    )
    .unwrap();

    // Pin, trust, activate: the ordinary shape of a project that has said yes
    // once. `use --write` renders the native config and materializes the skill.
    lock_cmd::run(&LockArgs::default(), Some(&proj)).unwrap();
    trust::trust_unreviewed(&proj).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);
    use_profile::run(&use_args(), Some(&proj)).unwrap();

    Machine {
        home,
        ashome,
        lib_home,
        proj,
    }
}

/// A library-sourced project (no inline `[skills.helper]` block — the name
/// resolves through the central library).
fn library_machine(tmp: &Path) -> Machine {
    machine(tmp, "")
}

fn cleanup() {
    std::env::remove_var("AGENTSTACK_HOME");
    std::env::remove_var("HOME");
}

/// The artifact `use --write` left for `name` in the target's skills dir —
/// what a harness actually opens. Project scope for a repo manifest, with the
/// global dir as the fallback so the helper does not silently pass by looking
/// in the wrong place.
fn rendered_skill(m: &Machine, name: &str) -> PathBuf {
    let project = m.proj.join(".claude/skills").join(name);
    if project.symlink_metadata().is_ok() {
        return project;
    }
    let global = m.home.join(".claude/skills").join(name);
    assert!(
        global.symlink_metadata().is_ok(),
        "the fixture must have rendered '{name}' somewhere"
    );
    global
}

/// The digest this project pinned for `name`.
fn pinned_digest(proj: &Path, name: &str) -> String {
    agentstack::lock::Lock::load(proj)
        .unwrap()
        .get(name)
        .expect("the fixture must pin the skill")
        .checksum
        .hex()
        .to_string()
}

// ------------------------------------------------------- byte-level state --

/// One filesystem entry, compared by what it IS rather than by what it
/// resolves to. A symlink records its TARGET: "nothing changed" must not be
/// satisfiable by repointing a link at different bytes.
#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Dir,
    Link(PathBuf),
    File(Vec<u8>),
}

fn snapshot(roots: &[PathBuf]) -> BTreeMap<PathBuf, Entry> {
    let mut out = BTreeMap::new();
    for root in roots {
        collect(root, &mut out);
    }
    out
}

fn collect(path: &Path, out: &mut BTreeMap<PathBuf, Entry>) {
    let Ok(meta) = path.symlink_metadata() else {
        return;
    };
    if meta.file_type().is_symlink() {
        out.insert(
            path.to_path_buf(),
            Entry::Link(fs::read_link(path).unwrap()),
        );
        return;
    }
    if meta.is_dir() {
        out.insert(path.to_path_buf(), Entry::Dir);
        let Ok(rd) = fs::read_dir(path) else { return };
        for e in rd.flatten() {
            collect(&e.path(), out);
        }
        return;
    }
    out.insert(
        path.to_path_buf(),
        Entry::File(fs::read(path).unwrap_or_default()),
    );
}

// --------------------------------------------------------- the MCP session --

/// One live `agentstack mcp` process, driven request by request. Copied from
/// `yes_on_lease_path.rs` rather than shared — test binaries cannot import each
/// other. Eager mode only: this file's subject is a project that already said
/// yes, not the consent door.
struct McpSession {
    child: std::process::Child,
    stdin: std::process::ChildStdin,
    stdout: std::io::BufReader<std::process::ChildStdout>,
}

impl McpSession {
    fn open(proj: &Path) -> McpSession {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"));
        cmd.args(["mcp", "--manifest-dir"]).arg(proj);
        let mut child = cmd
            .current_dir(proj)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut session = McpSession {
            child,
            stdin,
            stdout,
        };
        session.request(1, "initialize", json!({}));
        session
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Value {
        use std::io::{BufRead, Write};
        let frame = json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params });
        writeln!(self.stdin, "{frame}").unwrap();
        self.stdin.flush().unwrap();
        let mut line = String::new();
        self.stdout.read_line(&mut line).unwrap();
        let v: Value = serde_json::from_str(&line)
            .unwrap_or_else(|e| panic!("not a JSON-RPC frame ({e}): {line:?}"));
        assert_eq!(v["id"], json!(id), "unexpected response: {v}");
        v
    }

    fn load(&mut self, name: &str) -> Value {
        self.request(
            2,
            "tools/call",
            json!({
                "name": "agentstack_load",
                "arguments": { "name": name, "reason": "witness the served bytes" }
            }),
        )
    }

    fn close(mut self) {
        drop(self.stdin);
        let _ = self.child.wait();
    }
}

/// The text of a `tools/call` result, whatever its outcome.
fn call_text(response: &Value) -> &str {
    response["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
}

/// One load, start to finish: open a session, load `name`, close it. Returns
/// the raw tool result.
fn load_once(proj: &Path, name: &str) -> Value {
    let mut mcp = McpSession::open(proj);
    let out = mcp.load(name);
    mcp.close();
    out
}

/// The successful load's payload, parsed. Panics (with the refusal text) if the
/// load was refused — a refusal here is the failure being witnessed against.
fn served(response: &Value) -> Value {
    assert_ne!(
        response["result"]["isError"],
        json!(true),
        "the load was refused: {}",
        call_text(response)
    );
    serde_json::from_str(call_text(response)).expect("a load result is a JSON payload")
}

// ------------------------------------------------------------ the library --

/// Give the library a git remote and push it, returning the remote URL.
fn publish(m: &Machine, tmp: &Path) -> String {
    let bare = tmp.join("remote.git");
    git(&["init", "-q", "--bare", &bare.to_string_lossy()]);
    let url = format!("file://{}", bare.display());
    lib::run(&sync_args(true, Some(&url)), None).unwrap();
    lib::run(&sync_args(false, None), None).unwrap();
    assert!(m.lib_home.join(".git").is_dir());
    url
}

/// A second machine changes the skill and pushes — the "someone else moved the
/// library ahead" half of a sync.
fn push_new_bytes(url: &str, tmp: &Path) {
    let work = tmp.join("machine2");
    git(&["clone", "-q", url, &work.to_string_lossy()]);
    fs::write(
        work.join("skills/helper/SKILL.md"),
        format!("---\nname: helper\ndescription: Helps.\n---\n{NEWER_BODY}\n"),
    )
    .unwrap();
    let w = work.to_str().unwrap();
    git(&["-C", w, "add", "-A"]);
    git(&["-C", w, "commit", "-qm", "newer helper"]);
    git(&["-C", w, "push", "-q", "origin", "HEAD"]);
}

// ---------------------------------------------------------------- witnesses --

/// Acceptance, first half: a `lib sync` that pulls changed bytes changes **no
/// active bytes, no lease, no trust state, and no rendered file** in any
/// project.
///
/// The comparison is deliberately structural — every path under the project and
/// under the rendered global config, with symlinks compared by target — because
/// the interesting failure is not "a file was rewritten" but "a link now points
/// somewhere else", which a presence check would wave through.
#[test]
fn a_sync_that_pulls_changed_bytes_leaves_every_project_byte_identical() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();
    let tmp = assert_fs::TempDir::new().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    let m = library_machine(&tmp);

    let roots = vec![m.proj.clone(), m.home.join(".claude")];
    let before = snapshot(&roots);
    assert!(
        before.len() > 3,
        "the fixture must have rendered something to compare: {before:#?}"
    );
    let trust_before = (trust::check(&m.proj), trust::digest_for(&m.proj));
    assert_eq!(trust_before.0, TrustState::Trusted);

    let url = publish(&m, &tmp);
    push_new_bytes(&url, &tmp);
    lib::run(&sync_args(false, None), None).unwrap();

    // Precondition: the sync really did move the library's live bytes.
    let live = fs::read_to_string(m.lib_home.join("skills/helper/SKILL.md")).unwrap();
    assert!(live.contains(NEWER_BODY), "the sync pulled nothing: {live}");

    assert_eq!(
        snapshot(&roots),
        before,
        "a library sync rewrote something in the project or its rendered output"
    );
    assert_eq!(
        (trust::check(&m.proj), trust::digest_for(&m.proj)),
        trust_before,
        "a library sync must not touch trust state or the consented digest"
    );
    cleanup();
}

/// Acceptance, second half — and the decisive witness for
/// `docs/design/pinned-serving-and-library-drift.md`: after the sync changed a
/// pinned library skill's bytes, the load path serves the PINNED body, names
/// that a newer version exists, and never mentions the library's new bytes.
///
/// Before that decision this refused: the loader re-resolved the live library,
/// saw checksum drift, and blocked — which made rule 1 ("`lib sync` … never
/// interrupts") false for every project that had loaded a library skill.
#[test]
fn a_project_keeps_serving_its_pinned_bytes_after_a_sync() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();
    let tmp = assert_fs::TempDir::new().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    let m = library_machine(&tmp);

    let url = publish(&m, &tmp);
    push_new_bytes(&url, &tmp);
    lib::run(&sync_args(false, None), None).unwrap();

    let out = served(&load_once(&m.proj, "helper"));
    let body = out["instructions"].as_str().unwrap_or_default();
    assert!(
        body.contains(PINNED_BODY),
        "the pinned bytes were not what got served: {body}"
    );
    assert!(
        !body.contains(NEWER_BODY),
        "the library's newer bytes reached agent context: {body}"
    );

    // The user still learns the library moved — as an offer, not an alarm.
    let note = out["note"].as_str().unwrap_or_default();
    assert!(
        note.contains("newer version") && note.contains("helper"),
        "the load must say a newer version is available: {out}"
    );
    assert!(
        note.contains("agentstack lock"),
        "the offer must name the one command that takes it: {note}"
    );
    assert!(
        out["warning"].is_null(),
        "keep-pinned is the resting state — this is not a warning: {out}"
    );
    cleanup();
}

/// The serving change itself, without git in the picture: with the store
/// holding the pinned snapshot, the live library directory is mutated out from
/// under the project and the served body is still the pinned one.
///
/// Second half, same subject from the other side: when the STORE snapshot is
/// the thing that cannot be trusted, the load refuses and names `agentstack
/// lock` — it never falls back to reading the live directory, which is the
/// fallback that would quietly undo the whole rule.
#[test]
fn a_pinned_skill_is_served_from_the_content_store_not_the_live_library() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    let m = library_machine(&tmp);

    // The store received the pinned bytes at lock time.
    let digest = pinned_digest(&m.proj, "helper");
    let snapshot_dir = m.ashome.join("store/content").join(&digest);
    assert!(
        snapshot_dir.join("SKILL.md").is_file(),
        "the pinning act must have deposited the bytes it pinned"
    );

    // The live library moves — by nothing but an editor.
    fs::write(
        m.lib_home.join("skills/helper/SKILL.md"),
        format!("---\nname: helper\ndescription: Helps.\n---\n{NEWER_BODY}\n"),
    )
    .unwrap();

    let out = served(&load_once(&m.proj, "helper"));
    let body = out["instructions"].as_str().unwrap_or_default();
    assert!(body.contains(PINNED_BODY), "{body}");
    assert!(!body.contains(NEWER_BODY), "{body}");

    // Now the other direction: restore the live bytes so they match the pin
    // again (the load would otherwise be refused for drift), and tamper with
    // the STORE copy instead.
    fs::write(
        m.lib_home.join("skills/helper/SKILL.md"),
        format!("---\nname: helper\ndescription: Helps.\n---\n{PINNED_BODY}\n"),
    )
    .unwrap();
    fs::write(snapshot_dir.join("SKILL.md"), "tampered\n").unwrap();

    let refused = load_once(&m.proj, "helper");
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "a store snapshot that fails verification must refuse: {refused}"
    );
    let text = call_text(&refused);
    assert!(text.contains("helper"), "the refusal must name it: {text}");
    assert!(text.contains("content store"), "{text}");
    assert!(
        text.contains("agentstack lock"),
        "a refusal names the one command that fixes it: {text}"
    );
    assert!(
        !text.contains(PINNED_BODY) && !text.contains("tampered"),
        "the refusal served bytes instead of refusing: {text}"
    );
    cleanup();
}

/// The RENDERED lane's half of the same rule, and the decisive one: reading
/// **through** the artifact `use --write` left on disk still yields the pinned
/// bytes after the library moved ahead.
///
/// This is a different question from the served one above, and it used to have
/// a different answer. `render::skills` symlinks the artifact at its source
/// dir; when that source was the live library directory, a `lib sync` changed
/// the bytes a harness reads through the link while the link's TARGET STRING,
/// the lock, and the trust digest all stayed put — unreviewed content in agent
/// context with nothing re-gating it (invariant 4). The structural assertion is
/// the one that says why: the delivered artifact must not resolve back into the
/// live library at all.
#[test]
fn a_rendered_skill_still_reads_its_pinned_bytes_after_a_sync() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    git_identity();
    let tmp = assert_fs::TempDir::new().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    let m = library_machine(&tmp);

    // Precondition: the artifact exists and reads as the pinned body today.
    let rendered = rendered_skill(&m, "helper");
    let before = fs::read_to_string(rendered.join("SKILL.md")).unwrap();
    assert!(
        before.contains(PINNED_BODY),
        "the fixture did not render the pinned body: {before}"
    );

    let url = publish(&m, &tmp);
    push_new_bytes(&url, &tmp);
    lib::run(&sync_args(false, None), None).unwrap();

    // Precondition: the sync really did move the library's live bytes.
    let live = fs::read_to_string(m.lib_home.join("skills/helper/SKILL.md")).unwrap();
    assert!(live.contains(NEWER_BODY), "the sync pulled nothing: {live}");

    let after = fs::read_to_string(rendered.join("SKILL.md")).unwrap();
    assert!(
        after.contains(PINNED_BODY),
        "the rendered artifact no longer reads the pinned bytes: {after}"
    );
    assert!(
        !after.contains(NEWER_BODY),
        "the library's newer bytes reach agent context through the rendered \
         artifact — the symlink target never changed, so nothing re-gated: {after}"
    );

    // Why it holds, structurally: the artifact resolves into the immutable,
    // content-addressed snapshot for the digest this project pinned, and never
    // into the directory `lib sync` rewrites.
    let target = fs::canonicalize(&rendered).unwrap();
    assert!(
        !target.starts_with(&m.lib_home),
        "the rendered artifact still points into the live library: {}",
        target.display()
    );
    assert!(
        target.starts_with(
            m.ashome
                .join("store/content")
                .join(pinned_digest(&m.proj, "helper"))
        ),
        "the rendered artifact must resolve to the pinned snapshot, got {}",
        target.display()
    );
    cleanup();
}

/// Fail closed on the rendered lane. When the store cannot produce verified
/// bytes for the pin, `use --write` refuses the skill by name and points at
/// `agentstack lock` — it never quietly renders the live directory instead,
/// which is the fallback that would restore the whole hole.
#[test]
fn materializing_a_pinned_skill_whose_store_copy_is_tampered_refuses() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    let m = library_machine(&tmp);

    // Start from no rendered artifact, so what the refusal leaves behind is
    // unambiguous rather than inherited from the fixture's activation.
    let rendered = rendered_skill(&m, "helper");
    fs::remove_file(&rendered).unwrap();

    // The live library still matches the pin — the drift gate has nothing to
    // say, which is what isolates this to the store read. The STORE copy is
    // what cannot be trusted.
    let digest = pinned_digest(&m.proj, "helper");
    let snapshot_dir = m.ashome.join("store/content").join(&digest);
    fs::write(snapshot_dir.join("SKILL.md"), "tampered\n").unwrap();

    let err = use_profile::run(&use_args(), Some(&m.proj))
        .expect_err("a store snapshot that fails verification must refuse the activation");
    let text = format!("{err:#}");
    assert!(text.contains("helper"), "the refusal must name it: {text}");
    assert!(text.contains("content store"), "{text}");
    assert!(
        text.contains("agentstack lock"),
        "a refusal names the one command that fixes it: {text}"
    );

    // Nothing was rendered — and above all nothing pointing at the live
    // library, which is the silent fallback this refusal exists to prevent.
    if let Ok(target) = fs::canonicalize(&rendered) {
        panic!(
            "a refused activation left an artifact at {}: {}",
            rendered.display(),
            target.display()
        );
    }
    cleanup();
}

/// The scope fence. An INLINE skill's bytes are the project's own content, so
/// their changing is project drift — not an update available — and it must
/// still refuse exactly as before. If this ever passes a body into agent
/// context, the library exemption has widened past library sources.
#[test]
fn an_inline_skill_that_drifts_still_blocks() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let tmp = tmp.path().canonicalize().unwrap();
    // The project-local body has to exist before the fixture pins and
    // activates, so it is seeded into the project directory first.
    fs::create_dir_all(tmp.join("proj/skills/helper")).unwrap();
    fs::write(
        tmp.join("proj/skills/helper/SKILL.md"),
        format!("---\nname: helper\ndescription: Helps.\n---\n{PINNED_BODY}\n"),
    )
    .unwrap();
    // The same name, declared inline: an inline block wins over the library
    // index, so this is the project-local skill in every respect.
    let m = machine(&tmp, "[skills.helper]\npath = \"./skills/helper\"\n");
    served(&load_once(&m.proj, "helper"));

    // Now the project's own content changes.
    fs::write(
        m.proj.join("skills/helper/SKILL.md"),
        format!("---\nname: helper\ndescription: Helps.\n---\n{NEWER_BODY}\n"),
    )
    .unwrap();

    let refused = load_once(&m.proj, "helper");
    assert_eq!(
        refused["result"]["isError"],
        json!(true),
        "inline drift must still refuse: {refused}"
    );
    let text = call_text(&refused);
    assert!(text.contains("drifted"), "{text}");
    assert!(text.contains("helper"), "{text}");
    assert!(
        !text.contains(NEWER_BODY) && !text.contains(PINNED_BODY),
        "a refused load must serve nothing: {text}"
    );
    cleanup();
}
