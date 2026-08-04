// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `trust-card-diff-v1`: the consent card, structured.
//!
//! `trust --preview` now carries a `review` object — the same per-item facts
//! the terminal card prints, plus the change markers a re-review needs. It is
//! computed by the preview's OWN read-only walk, because the authoritative walk
//! resolves content (and materializes git worktrees) and must not move.
//!
//! Two walks computing "the same" identity strings is exactly the arrangement
//! that rots silently: the day one side's format changes, every re-review says
//! `changed` about content nobody touched, and the user learns to wave the diff
//! through. So the load-bearing witness here is **identity parity** — grant a
//! maximal fixture with the real binary, then preview it and assert every item
//! reads `unchanged`. It fails the moment the two walks disagree by one byte.
//!
//! The rest of the file pins the properties the design doc calls binding: the
//! diff is pin-to-pin and capped, every missing input degrades instead of
//! gating, a drifted library server stays redacted, and the whole thing writes
//! nothing.

use std::fs;
use std::path::Path;

fn run(bin: &str, args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = std::process::Command::new(bin)
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
        // No terminal: the non-interactive consent gate is the real path here,
        // which is why every grant below presents a consented digest.
        .stdin(std::process::Stdio::null())
        .output()
        .expect("spawn agentstack");
    (
        format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        ),
        out.status.success(),
    )
}

fn preview(bin: &str, home: &Path, proj: &Path) -> serde_json::Value {
    let (text, ok) = run(bin, &["trust", "--preview"], home, proj);
    assert!(ok, "preview failed:\n{text}");
    serde_json::from_str(&text).expect("preview is JSON")
}

/// Review the current surface and consent to exactly the digest it emitted —
/// the non-interactive path a panel drives.
fn grant(bin: &str, home: &Path, proj: &Path) -> serde_json::Value {
    let previewed = preview(bin, home, proj);
    let digest = previewed["surface_digest"].as_str().unwrap().to_string();
    let (text, ok) = run(
        bin,
        &["trust", "--yes", "--consented-digest", &digest],
        home,
        proj,
    );
    assert!(ok, "grant failed:\n{text}");
    previewed
}

fn lock(bin: &str, home: &Path, proj: &Path) {
    let (text, ok) = run(bin, &["lock", "--write"], home, proj);
    assert!(ok, "lock failed:\n{text}");
}

fn item<'a>(review: &'a serde_json::Value, kind: &str, name: &str) -> &'a serde_json::Value {
    review["items"]
        .as_array()
        .expect("review carries items")
        .iter()
        .find(|i| i["kind"] == kind && i["name"] == name)
        .unwrap_or_else(|| panic!("no {kind} item named {name} in {review}"))
}

/// A project declaring one of every kind the card discloses. Deliberately
/// maximal: parity is fixture-relative, so a kind absent here is a kind whose
/// two identity constructions are unwitnessed.
fn write_fixture(proj: &Path) {
    let a = proj.join(".agentstack");
    fs::create_dir_all(a.join("skills/summarize")).unwrap();
    fs::create_dir_all(a.join("instructions")).unwrap();
    fs::create_dir_all(a.join("workflows")).unwrap();
    fs::write(a.join("skills/summarize/SKILL.md"), "# Summarize\nbody\n").unwrap();
    fs::write(
        a.join("instructions/house-rules.md"),
        "Prefer boring code.\n",
    )
    .unwrap();
    fs::write(
        a.join("workflows/main.js"),
        "export const meta = { roles: ['worker'] };\nreturn 1;\n",
    )
    .unwrap();
    fs::write(a.join("ext.ts"), "export default {}\n").unwrap();
    fs::write(
        a.join("agentstack.toml"),
        r#"version = 1

[servers.filesystem]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "."]

[servers.docs]
type = "http"
url = "https://api.example.com/mcp/docs"

[servers.docs.headers]
Authorization = "Bearer ${DOCS_TOKEN}"

[skills.summarize]
path = "./skills/summarize"

[instructions.house-rules]
path = "./instructions/house-rules.md"

[workflows.pipeline]
path = "./workflows/main.js"
roles = ["worker"]

[extensions.addon]
path = "./ext.ts"
target = "pi"

[hooks.pre-commit]
event = "PreToolUse"
matcher = "Bash"
command = "./scripts/check.sh"
args = ["--strict"]

[settings.claude-code]
permissions = { allow = ["Bash(git status)"] }

[profiles.worker]
skills = ["summarize"]
servers = ["kibana"]

[policy.egress]
docs = ["api.example.com"]
"#,
    )
    .unwrap();
}

/// A central-library server the fixture references by name — the one kind whose
/// definition lives outside the project, and therefore the one the preview
/// redacts when it drifts from its pin.
fn install_library_server(home: &Path, args: &str) {
    let lib = home.join(".agentstack/lib");
    fs::create_dir_all(lib.join("servers")).unwrap();
    fs::write(
        lib.join("servers/kibana.toml"),
        format!("type = \"stdio\"\ncommand = \"node\"\nargs = [\"{args}\"]\n"),
    )
    .unwrap();
    fs::write(
        lib.join("library.toml"),
        "version = 1\n\n[[server]]\nname = \"kibana\"\n",
    )
    .unwrap();
}

fn fixture(tmp: &Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let home = tmp.join("home");
    let proj = tmp.join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    install_library_server(&home, "kibana.js");
    write_fixture(&proj);
    (home, proj)
}

/// Every regular file under `root`, as (relative path, bytes).
fn tree(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let Ok(meta) = p.symlink_metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(p);
            } else if let Ok(bytes) = fs::read(&p) {
                out.push((
                    p.strip_prefix(root).unwrap().to_string_lossy().to_string(),
                    bytes,
                ));
            }
        }
    }
    out.sort();
    out
}

/// PARITY WITNESS. NEVER weaken this: it is the only thing standing between
/// "the preview recomputes the grant's identities" and "the preview invents its
/// own strings and calls every re-review a change".
///
/// Grant a maximal fixture, then preview it: the surface the grant just
/// recorded and the surface the preview just computed must agree on every item,
/// which they can only do if both walks build identical identity strings for
/// every kind.
#[test]
fn every_reviewed_item_reads_unchanged_right_after_the_grant_recorded_it() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);

    let v = preview(bin, &home, &proj);
    let features: Vec<&str> = v["features"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|f| f.as_str())
        .collect();
    assert!(
        features.contains(&"trust-card-diff-v1"),
        "the payload is not advertised: {features:?}"
    );

    let review = &v["review"];
    assert_eq!(review["re_review"], true);
    assert_eq!(review["prior_recorded"], true);
    assert_eq!(review["removed"].as_array().unwrap().len(), 0);

    let items = review["items"].as_array().unwrap();
    let drifted: Vec<String> = items
        .iter()
        .filter(|i| i["change"] != "unchanged")
        .map(|i| format!("{} {} → {}", i["kind"], i["name"], i["change"]))
        .collect();
    assert!(
        drifted.is_empty(),
        "the preview and the grant disagree about these items — one walk's \
         identity format moved: {drifted:?}"
    );

    // Fixture honesty: parity over an empty list would pass vacuously.
    let kinds: Vec<&str> = items.iter().filter_map(|i| i["kind"].as_str()).collect();
    for owed in [
        "server",
        "secrets",
        "extension",
        "workflow",
        "skill",
        "instruction",
        "hook",
        "settings",
        "policy",
    ] {
        assert!(kinds.contains(&owed), "the fixture declares no {owed}");
    }

    // The card's per-item facts, in the user's words.
    assert_eq!(
        item(review, "server", "filesystem")["runs"][0],
        "npx -y @modelcontextprotocol/server-filesystem ."
    );
    assert_eq!(
        item(review, "server", "docs")["contacts"][0],
        "https://api.example.com/mcp/docs"
    );
    assert_eq!(item(review, "secrets", "")["may_read"][0], "DOCS_TOKEN");
    // A hook runs a command at the user's permission, so it is disclosed as
    // something this project RUNS — the executable-surface bug in reverse.
    assert_eq!(
        item(review, "hook", "pre-commit")["runs"][0],
        "./scripts/check.sh --strict"
    );
    // Pinned kinds carry both pins; the rest carry none.
    let skill = item(review, "skill", "summarize");
    assert!(skill["pin"].is_string() && skill["prior_pin"].is_string());
    assert_eq!(skill["diff"]["status"], "unchanged");
    assert!(item(review, "hook", "pre-commit")["diff"].is_null());
}

/// The re-gate story: the bytes moved, so the DIFF says so — while `change`
/// keeps reading `unchanged`, because a skill's identity is where its body
/// comes from and that did not move. The two answer different questions and
/// both are printed, which is the divergence `ui_contract.rs` records.
#[test]
fn a_moved_pin_reports_the_lines_that_moved() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);

    fs::write(
        proj.join(".agentstack/skills/summarize/SKILL.md"),
        "# Summarize\nbody changed here\n",
    )
    .unwrap();
    lock(bin, &home, &proj);

    let v = preview(bin, &home, &proj);
    let skill = item(&v["review"], "skill", "summarize");
    assert_ne!(
        skill["pin"], skill["prior_pin"],
        "the lock moved but the card shows one pin"
    );
    assert_eq!(
        skill["change"], "unchanged",
        "the pin is deliberately not part of the diff key"
    );
    let diff = &skill["diff"];
    assert_eq!(diff["status"], "changed");
    assert_eq!(diff["capped"], false);
    assert_eq!(diff["headline"], "changed 2 lines");
    assert_eq!(diff["files"][0]["path"], "SKILL.md");
    assert_eq!(diff["files"][0]["change"], "modified");
    assert_eq!(diff["files"][0]["added"], 1);
    assert_eq!(diff["files"][0]["removed"], 1);
    let lines = diff["files"][0]["lines"].as_array().unwrap();
    let joined = lines
        .iter()
        .filter_map(|l| l.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(joined.contains("+ body changed here"), "{joined}");
    assert!(joined.contains("- body"), "{joined}");
}

/// The cap: a rewrite too large to read inline names the files and the exact
/// counts and drops the lines. A panel that rendered 400 lines into a consent
/// dialog would be the same flood the terminal refuses.
#[test]
fn an_oversized_rewrite_keeps_the_counts_and_drops_the_lines() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);

    let rewritten: String = (0..200).map(|i| format!("line {i}\n")).collect();
    fs::write(
        proj.join(".agentstack/skills/summarize/SKILL.md"),
        rewritten,
    )
    .unwrap();
    lock(bin, &home, &proj);

    let v = preview(bin, &home, &proj);
    let diff = &item(&v["review"], "skill", "summarize")["diff"];
    assert_eq!(diff["status"], "changed");
    assert_eq!(diff["capped"], true);
    assert!(
        diff["files"][0]["lines"].is_null(),
        "an oversized diff kept its lines"
    );
    assert_eq!(diff["files"][0]["added"], 200);
    assert!(diff["headline"].is_string(), "the scale is still named");
}

/// Degrade (a): a project nobody ever trusted. Everything is `added`, nothing
/// claims a prior, and no diff pretends to know what changed — and the command
/// still succeeds, because none of this may gate.
#[test]
fn a_never_trusted_project_reads_as_all_added_and_invents_no_diff() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);

    let review = preview(bin, &home, &proj)["review"].clone();
    assert_eq!(review["re_review"], false);
    assert_eq!(review["prior_recorded"], false);
    assert_eq!(review["removed"].as_array().unwrap().len(), 0);
    for i in review["items"].as_array().unwrap() {
        assert_eq!(i["change"], "added", "{i}");
        assert!(i["prior_pin"].is_null(), "{i}");
        let status = i["diff"]["status"].as_str();
        assert!(
            i["diff"].is_null() || status == Some("no_snapshot"),
            "an unreviewed item claimed to know what changed: {i}"
        );
    }
    // No recognition index on a fresh machine: `null`, not a fabricated zero.
    assert!(item(&review, "skill", "summarize")["recognized_other_projects"].is_null());
}

/// Degrades (b) and (c): the approved bytes are gone, or no longer hash to
/// their own name. Both answer `no_snapshot` — the honest "I cannot show you
/// what changed" — and the tampered case NEVER renders the bytes it found.
#[test]
fn a_deleted_or_tampered_snapshot_degrades_to_no_snapshot() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    let granted = grant(bin, &home, &proj);
    let prior_pin = item(&granted["review"], "skill", "summarize")["pin"]
        .as_str()
        .unwrap()
        .to_string();

    fs::write(
        proj.join(".agentstack/skills/summarize/SKILL.md"),
        "# Summarize\nsecond version\n",
    )
    .unwrap();
    lock(bin, &home, &proj);

    let snapshot = home.join(".agentstack/store/content").join(&prior_pin);
    assert!(snapshot.is_dir(), "the fixture never deposited its bytes");

    // (c) tampered: the snapshot is still there and no longer hashes to its
    // name. The bytes planted here must not reach the payload.
    fs::write(snapshot.join("SKILL.md"), "# EVIL\nplanted\n").unwrap();
    let v = preview(bin, &home, &proj);
    let skill = item(&v["review"], "skill", "summarize");
    assert_eq!(skill["diff"]["status"], "no_snapshot");
    assert!(
        !v.to_string().contains("planted"),
        "tampered store content reached the consent payload"
    );
    // The pins are still shown — honest about what it does and does not know.
    assert_eq!(skill["prior_pin"], prior_pin.as_str());
    assert!(skill["pin"].is_string());

    // (b) absent: same answer, no error.
    fs::remove_dir_all(&snapshot).unwrap();
    let skill = item(&preview(bin, &home, &proj)["review"], "skill", "summarize").clone();
    assert_eq!(skill["diff"]["status"], "no_snapshot");
    assert_eq!(skill["prior_pin"], prior_pin.as_str());
}

/// The inherited redaction: a library server whose live definition no longer
/// matches its pin is named as changed and NOT quoted. Saying that something
/// moved discloses nothing; emitting bytes the consent digest does not cover
/// would let a UI bind consent to them.
#[test]
fn a_drifted_library_server_stays_redacted_and_still_reads_changed() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);

    install_library_server(&home, "evil.js");
    let v = preview(bin, &home, &proj);
    let kibana = item(&v["review"], "server", "kibana");
    assert_eq!(kibana["change"], "changed");
    assert!(
        kibana["identity"]
            .as_str()
            .unwrap()
            .contains("does not match the lockfile pin"),
        "{kibana}"
    );
    assert_eq!(kibana["runs"].as_array().unwrap().len(), 0);
    assert_eq!(kibana["contacts"].as_array().unwrap().len(), 0);
    assert!(
        !v.to_string().contains("evil.js"),
        "the drifted definition leaked into the payload: {v}"
    );
}

/// Declare a git-sourced LIBRARY skill that was never cloned, reference it from
/// the fixture's profile so the review walks it, and pin it by hand — `lock`
/// itself would try to clone, which is precisely the situation being witnessed:
/// the pin exists, the checkout does not.
fn install_uncached_git_library_skill(home: &Path, proj: &Path, name: &str) {
    let lib = home.join(".agentstack/lib");
    let text = fs::read_to_string(lib.join("library.toml")).unwrap();
    fs::write(
        lib.join("library.toml"),
        format!(
            "{text}\n[[skill]]\nname = \"{name}\"\nsource = \"git\"\n\
             git = \"https://example.invalid/{name}.git\"\n\
             rev = \"{REMOTE_REV}\"\n"
        ),
    )
    .unwrap();
    let manifest = proj.join(".agentstack/agentstack.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        text.replace(
            "skills = [\"summarize\"]",
            &format!("skills = [\"summarize\", \"{name}\"]"),
        ),
    )
    .unwrap();
    let lockfile = proj.join(".agentstack/agentstack.lock");
    let text = fs::read_to_string(&lockfile).unwrap();
    fs::write(
        &lockfile,
        format!(
            "{text}\n[[skill]]\nname = \"{name}\"\nsource = \"git\"\n\
             git = \"https://example.invalid/{name}.git\"\nrev = \"{REMOTE_REV}\"\n\
             checksum = \"{}\"\n",
            "a".repeat(64)
        ),
    )
    .unwrap();
}

const REMOTE_REV: &str = "1111111111111111111111111111111111111111";

/// IDENTITY IS DECLARED, NOT RESOLVED. A git-sourced library skill with no
/// local checkout cannot be resolved offline — and the resolver's failure must
/// not become the item's identity. It once did: the grant walk recorded `?` for
/// anything the resolver could not reach, while the preview named the declared
/// origin, so the very next preview after a successful grant called a freshly
/// consented project `changed` — and offline it could never converge.
#[test]
fn an_unresolvable_library_skill_keeps_its_declared_identity() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    install_uncached_git_library_skill(&home, &proj, "remote-lib");
    grant(bin, &home, &proj);

    let review = preview(bin, &home, &proj)["review"].clone();
    let skill = item(&review, "skill", "remote-lib");
    assert_eq!(
        skill["identity"], "library",
        "the resolver's offline verdict leaked into the diff identity: {skill}"
    );
    assert_eq!(
        skill["change"], "unchanged",
        "a freshly granted, merely-uncached skill reads as drifted: {skill}"
    );

    // TAMPER: identity must still catch a real source flip. Declaring the same
    // name inline shadows the library entry, and that IS a change of where the
    // body comes from.
    let inline = proj.join(".agentstack/skills/remote-lib");
    fs::create_dir_all(&inline).unwrap();
    fs::write(inline.join("SKILL.md"), "# Remote\nlocal impostor\n").unwrap();
    let manifest = proj.join(".agentstack/agentstack.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!("{text}\n[skills.remote-lib]\npath = \"./skills/remote-lib\"\n"),
    )
    .unwrap();

    let flipped = item(&preview(bin, &home, &proj)["review"], "skill", "remote-lib").clone();
    assert_eq!(flipped["identity"], "inline");
    assert_eq!(
        flipped["change"], "changed",
        "a library→inline source flip went unnoticed: {flipped}"
    );
}

/// READ-ONLY WITNESS: `trust --preview` is consumed by a panel and must never
/// write, fetch, or spawn. The fixture declares a git-sourced skill that was
/// never fetched — the case that would drag in a clone and a worktree if the
/// preview resolved content the way the grant walk does.
#[test]
fn preview_writes_nothing_and_never_materializes_a_worktree() {
    let bin = env!("CARGO_BIN_EXE_agentstack");
    let tmp = tempfile::tempdir().unwrap();
    let (home, proj) = fixture(tmp.path());
    lock(bin, &home, &proj);
    grant(bin, &home, &proj);

    let manifest = proj.join(".agentstack/agentstack.toml");
    let text = fs::read_to_string(&manifest).unwrap();
    fs::write(
        &manifest,
        format!("{text}\n[skills.remote]\ngit = \"https://example.invalid/skills.git\"\n"),
    )
    .unwrap();

    let before_home = tree(&home);
    let before_proj = tree(&proj);
    let v = preview(bin, &home, &proj);
    assert!(
        item(&v["review"], "skill", "remote")["diff"]["status"] == "no_snapshot",
        "the un-fetched skill should degrade, not resolve"
    );
    assert_eq!(
        before_home,
        tree(&home),
        "preview wrote under the agentstack home"
    );
    assert_eq!(before_proj, tree(&proj), "preview wrote into the project");
    assert!(
        !home.join(".agentstack/store/co").exists(),
        "preview materialized a git worktree"
    );
}
