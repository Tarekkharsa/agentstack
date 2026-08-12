// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Packaging (TODO.md item 7): one toolset and its pinned capabilities
//! composed into a self-run container image.
//!
//! `docs/design/packaging.md` is the contract, and each test here is one of
//! its load-bearing clauses. The decisive one is
//! `no_secret_value_can_reach_the_image_or_the_build_context`: packaging is
//! the first feature that turns a project's reviewed content into a
//! *distributable* artifact, so invariant 5 — secrets never serialize — stops
//! being a property of one machine's files and becomes a property of something
//! that can be handed to somebody else.
//!
//! The end-to-end build is Docker-gated much the way `sandbox_cli_e2e.rs` and
//! `sandbox_lockdown.rs` gate theirs — probe the daemon, print `SKIP:` and
//! early-return where there is none — with one difference: a skip is a PASS to
//! the test runner, so [`skip_or_fail`] refuses to skip when `CI` is set. See
//! its doc comment. Everything else here — the plan, the staged context, every
//! refusal — runs with no daemon at all, which is itself part of the contract.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use agentstack::cli::{ImageArgs, LockArgs};
use agentstack::commands::{image as image_cmd, lock as lock_cmd};

// These tests mutate the process-global HOME/AGENTSTACK_HOME; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// A ref name whose VALUE must never appear anywhere the build writes.
const SECRET_REF: &str = "PACKAGING_TOKEN";
/// The value that ref resolves to on this machine, planted in the environment
/// and in a `.env` the resolver chain reads. If any byte of it reaches the
/// build context, the witness fails.
const SECRET_VALUE: &str = "sk-live-do-not-bake-me-4f9a2c";

/// A temp machine with `HOME` and `AGENTSTACK_HOME` inside `tmp`, plus a
/// project holding one HTTP server behind a `${REF}`, one path skill, and one
/// toolset selecting both. Mirrors `package_layer.rs`'s setup.
fn machine(tmp: &Path) -> PathBuf {
    let home = tmp.join("home");
    fs::create_dir_all(&home).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));

    let proj = tmp.join("proj");
    fs::create_dir_all(proj.join(".agentstack")).unwrap();
    fs::create_dir_all(proj.join("skills/sql-review")).unwrap();
    fs::write(
        proj.join("skills/sql-review/SKILL.md"),
        "---\nname: sql-review\ndescription: review SQL\n---\nPrefer explicit joins.\n",
    )
    .unwrap();
    fs::write(
        proj.join(".agentstack/agentstack.toml"),
        format!(
            "version = 1\n\
             \n[servers.kibana]\n\
             type = \"http\"\n\
             url = \"https://kibana.example.com/mcp\"\n\
             headers = {{ Authorization = \"Bearer ${{{SECRET_REF}}}\" }}\n\
             \n[skills.sql-review]\n\
             path = \"../skills/sql-review\"\n\
             \n[profiles.backend]\n\
             servers = [\"kibana\"]\n\
             skills = [\"sql-review\"]\n\
             harness = \"claude-code\"\n"
        ),
    )
    .unwrap();
    proj
}

fn lock(proj: &Path) {
    lock_cmd::run(&LockArgs::default(), Some(proj)).expect("lock must pin the fixture");
}

/// The project's lockfile. It lives beside the manifest in `.agentstack/`, not
/// at the project root — the nested layout every command resolves to.
fn lock_of(proj: &Path) -> agentstack::lock::Lock {
    agentstack::lock::Lock::load(&proj.join(".agentstack")).unwrap()
}

/// Consent, through the one grant path `agentstack trust` uses.
fn trust(proj: &Path) {
    let digest = agentstack::trust::digest_for(proj).unwrap();
    agentstack::commands::trust::grant_with_answers(proj, true, Some(&digest), false, None)
        .expect("fixture must be trusted");
}

fn args(write: bool) -> ImageArgs {
    ImageArgs {
        toolset: Some("backend".into()),
        harness: None,
        tag: Some("agentstack-test/backend:witness".into()),
        from: None,
        json: false,
        write,
    }
}

/// Point the packaging backend at a docker client that cannot exist, so the
/// daemon-absent branch is exercised deterministically on a machine that
/// happens to have Docker installed.
fn without_docker<T>(f: impl FnOnce() -> T) -> T {
    std::env::set_var(
        agentstack_runtime::image::DOCKER_PROGRAM_ENV,
        "/nonexistent/agentstack-no-docker-here",
    );
    let out = f();
    std::env::remove_var(agentstack_runtime::image::DOCKER_PROGRAM_ENV);
    out
}

/// Every file under `root`, recursively.
fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

fn plan_of(proj: &Path, toolset: &str) -> agentstack::image::ImagePlan {
    let ctx = agentstack::commands::load(Some(proj)).unwrap();
    let libctx = ctx.library_ctx();
    agentstack::image::plan(
        &ctx.loaded.manifest,
        &ctx.dir,
        &libctx,
        &ctx.registry,
        toolset,
        "claude-code",
        None,
        None,
    )
    .expect("planning must not fail on a well-formed toolset")
}

// ── 1. the plan is honest and complete ─────────────────────────────────────

/// The dry run's job is to be a *complete* description of the artifact: a user
/// approving a build has to be able to see every byte of their content that
/// would enter it. A member the plan omits is a member nobody reviewed.
#[test]
fn a_plan_names_every_pinned_member_that_would_enter_the_image() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = machine(tmp.path());
    lock(&proj);

    let plan = plan_of(&proj, "backend");
    assert!(plan.buildable(), "blockers: {:?}", plan.blockers);

    // Every selected member, by name, with the digest its bytes are read by.
    let skill_pin = lock_of(&proj)
        .get("sql-review")
        .expect("the fixture skill is pinned")
        .checksum
        .hex()
        .to_string();

    let skill = plan
        .members
        .iter()
        .find(|m| m.name == "sql-review")
        .expect("the toolset's skill is named in the plan");
    assert_eq!(
        skill.digest, skill_pin,
        "the plan's digest must be the LOCK's pin — an image built from any other \
         digest is an image nobody reviewed"
    );
    assert!(
        skill.dest.starts_with(agentstack::image::IMAGE_HOME),
        "a skill lands where the harness reads it, not in a directory of our own \
         invention: {}",
        skill.dest
    );
    assert!(skill.compiled, "a skill is laid down, not merely carried");

    let server = plan
        .members
        .iter()
        .find(|m| m.name == "kibana")
        .expect("the toolset's server is named in the plan");
    assert!(
        !server.compiled,
        "a server definition is CARRIED, never compiled into native config — \
         rendering one resolves ${{REF}} and writes values to disk"
    );

    // Nothing extra, and nothing missing: exactly the toolset's two members.
    assert_eq!(
        plan.members.len(),
        2,
        "the plan is the whole composition and nothing else: {:?}",
        plan.members
            .iter()
            .map(|m| (m.kind.as_str(), m.name.as_str()))
            .collect::<Vec<_>>()
    );

    // And the secret is named as a requirement, never as a value.
    assert_eq!(plan.required_secrets, vec![SECRET_REF.to_string()]);
}

// ── 2. the decisive one: no secret value can be baked ──────────────────────

/// Invariant 5, at the one seam where breaking it would travel: an image is
/// handed to other people.
///
/// The setup is deliberately hostile to the claim — the ref is resolvable
/// three ways on this machine (process env, a project `.env`, and a `.env` the
/// chain reads by default), so a build that resolved anything at all would
/// find a value to bake. The assertion then walks **every byte the build
/// writes** and refuses to find that value anywhere, while requiring the
/// placeholder and the ref's NAME to be present — because "no secret" would
/// also be satisfied by a build that quietly dropped the server.
#[test]
fn no_secret_value_can_reach_the_image_or_the_build_context() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = machine(tmp.path());

    // Make the ref resolvable every way the chain knows about.
    std::env::set_var(SECRET_REF, SECRET_VALUE);
    fs::write(
        proj.join(".agentstack/.env"),
        format!("{SECRET_REF}={SECRET_VALUE}\n"),
    )
    .unwrap();
    fs::write(proj.join(".env"), format!("{SECRET_REF}={SECRET_VALUE}\n")).unwrap();

    lock(&proj);
    trust(&proj);

    let before = project_snapshot(&proj);
    let context = agentstack::image::context_dir_for("backend");

    // Docker is absent, so the build stages the context in full and then
    // refuses — which is exactly the state this witness needs to inspect.
    let err = without_docker(|| image_cmd::run(&args(true), Some(&proj)))
        .expect_err("no daemon means the build cannot complete");
    let message = format!("{err:#}");
    assert!(
        message.contains("docker"),
        "the refusal must name Docker: {message}"
    );

    std::env::remove_var(SECRET_REF);

    // THE assertion: not one byte of the resolved value, anywhere.
    let files = walk(&context);
    assert!(!files.is_empty(), "the build context must have been staged");
    for f in &files {
        let bytes = fs::read(f).unwrap();
        let text = String::from_utf8_lossy(&bytes);
        assert!(
            !text.contains(SECRET_VALUE),
            "a resolved secret value reached {}",
            f.display()
        );
        // The value's distinctive tail, in case something re-encoded it.
        assert!(
            !text.contains("do-not-bake-me"),
            "a fragment of the resolved secret reached {}",
            f.display()
        );
    }

    // The placeholder survived, and the NAME is what the image will require —
    // so the absence above is honesty, not omission.
    let server_json = fs::read_to_string(context.join("agentstack/servers/kibana.json")).unwrap();
    assert!(
        server_json.contains(&format!("${{{SECRET_REF}}}")),
        "the server definition must carry its placeholder verbatim: {server_json}"
    );
    let required = fs::read_to_string(context.join("agentstack/required-secrets")).unwrap();
    assert_eq!(required.trim(), SECRET_REF);

    // The guard that makes "required at run time" real, and the one property
    // that keeps it safe: a file-derived name is an ARGUMENT to `printenv`,
    // never a fragment of a program (invariant 7).
    let entry = fs::read_to_string(context.join("agentstack/entrypoint.sh")).unwrap();
    assert!(entry.contains("printenv \"$ref\""), "{entry}");
    assert!(!entry.contains("eval"), "{entry}");
    assert!(entry.contains("exit 78"), "{entry}");

    // And the manifest that travelled with it is the commit-safe one.
    let staged_manifest =
        fs::read_to_string(context.join("agentstack/manifest/agentstack.toml")).unwrap();
    assert!(staged_manifest.contains(&format!("${{{SECRET_REF}}}")));
    assert!(!staged_manifest.contains(SECRET_VALUE));

    // Finally: the project itself is untouched. A build writes outside the
    // project or it is not a dry-run-by-default command in any useful sense.
    assert_eq!(
        before,
        project_snapshot(&proj),
        "building an image must not write into the project"
    );
}

/// Every project file with its bytes — the comparison a "wrote nothing here"
/// claim needs.
fn project_snapshot(proj: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    walk(proj)
        .into_iter()
        .map(|p| {
            let bytes = fs::read(&p).unwrap_or_default();
            (p, bytes)
        })
        .collect()
}

// ── 3. anything unaccounted-for refuses ────────────────────────────────────

/// Two shapes of "we cannot account for these bytes", both refusing before a
/// context directory exists: a member with no lock entry at all, and a member
/// whose pinned deposit the content store cannot verify.
#[test]
fn an_unpinned_or_unverifiable_member_fails_the_build_closed() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = machine(tmp.path());

    // (a) Never locked: the skill is selected and pinned nowhere.
    let plan = plan_of(&proj, "backend");
    assert!(!plan.buildable());
    let blocker = plan
        .blockers
        .iter()
        .find(|b| b.name == "sql-review")
        .expect("the unpinned skill is named");
    assert!(
        blocker.reason.contains("agentstack lock"),
        "a refusal names the safe next step: {}",
        blocker.reason
    );
    let err = image_cmd::run(&args(false), Some(&proj)).expect_err("an unpinned plan refuses");
    assert!(format!("{err:#}").contains("refusing to build"));
    assert!(
        !agentstack::image::context_dir_for("backend").exists(),
        "a refused plan must not have staged anything"
    );

    // (b) Locked, then the store deposit is corrupted. The pin exists and the
    // bytes it names cannot be produced — a signal, never a gap to fill from
    // whatever happens to be on disk.
    lock(&proj);
    trust(&proj);
    assert!(
        plan_of(&proj, "backend").buildable(),
        "sanity: locked builds"
    );

    let hex = lock_of(&proj)
        .get("sql-review")
        .expect("the fixture skill is pinned")
        .checksum
        .hex()
        .to_string();
    let deposit = agentstack::store::Store::default_store()
        .root()
        .join("content")
        .join(&hex);
    fs::write(deposit.join("SKILL.md"), "tampered\n").unwrap();

    let plan = plan_of(&proj, "backend");
    assert!(
        !plan.buildable(),
        "a deposit that no longer hashes to its own name must block"
    );
    assert!(
        plan.blockers.iter().any(|b| b.name == "sql-review"),
        "the blocker names the member: {:?}",
        plan.blockers
    );
    let err = without_docker(|| image_cmd::run(&args(true), Some(&proj)))
        .expect_err("--write refuses too");
    assert!(format!("{err:#}").contains("refusing to build"));
    assert!(
        !agentstack::image::context_dir_for("backend").exists(),
        "the refusal happens BEFORE anything is staged"
    );
}

// ── 4. the posture label is one of the shipped ones ────────────────────────

/// The artifact must not invent a fourth posture word, and must not claim the
/// one it has not earned. Asserted against the shipped `Posture` /
/// `GrantPosture` vocabulary, not against a string typed here.
#[test]
fn the_artifact_carries_an_honest_posture_label() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = machine(tmp.path());
    lock(&proj);
    trust(&proj);

    let plan = plan_of(&proj, "backend");
    let posture = plan.posture();

    // It is a SHIPPED posture: both vocabularies round-trip the same slug.
    assert_eq!(
        agentstack::commands::sandbox::Posture::from_slug(posture.slug()),
        Some(posture),
        "the artifact's slug must be one `Posture::from_slug` already knows"
    );
    assert!(
        agentstack::grant::GrantPosture::from_slug(posture.slug()).is_some(),
        "the grant vocabulary must know it too — one posture word, not two"
    );

    // It is the SANDBOX one, printed in its shipped spelling, and it is not
    // the lockdown one: topological confinement is established by the runner's
    // internal network and sidecar, neither of which an image contains.
    assert_eq!(
        posture,
        agentstack::commands::sandbox::Posture::Sandbox,
        "an image is prepared for the --sandbox contract, no stronger"
    );
    let label = posture.to_string();
    assert!(label.contains("DIRECT ROUTE OPEN"), "{label}");
    assert!(
        !label.contains("ENFORCED"),
        "ENFORCED is reserved for lockdown and must never appear on an artifact \
         that establishes no route at all: {label}"
    );

    // The payload states the same thing, and states the limit in the same
    // breath — the label and its caveat must never travel apart.
    let json = plan.to_json();
    let p = &json["image"]["posture"];
    assert_eq!(p["slug"], posture.slug());
    assert_eq!(p["label"], label);
    assert_eq!(
        p["established_by"], "run",
        "posture is a property of the run; an image is inert bytes"
    );
    let caveat = p["caveat"].as_str().unwrap();
    assert!(caveat.contains("docker run"), "{caveat}");
    assert!(caveat.contains("no egress proxy"), "{caveat}");

    // Staged, the same label reaches the artifact itself as a machine slug.
    without_docker(|| image_cmd::run(&args(true), Some(&proj))).expect_err("no daemon");
    let context = agentstack::image::context_dir_for("backend");
    let dockerfile = fs::read_to_string(context.join("Dockerfile")).unwrap();
    assert!(
        dockerfile.contains(&format!(r#"org.agentstack.posture="{}""#, posture.slug())),
        "{dockerfile}"
    );
    let descriptor: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(context.join("agentstack/image.json")).unwrap())
            .unwrap();
    assert_eq!(descriptor["image"]["posture"]["label"], label);
}

// ── 5. no daemon, no pretending ────────────────────────────────────────────

/// Planning and validation must work with no Docker at all, and the build must
/// say plainly which of the two distinct things is missing — a client, or a
/// daemon — plus the exact command that finishes the job. "Docker is
/// unavailable" would be true and useless.
#[test]
fn the_build_degrades_honestly_without_a_docker_daemon() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = machine(tmp.path());
    lock(&proj);
    trust(&proj);

    // The plan itself never asks about Docker.
    without_docker(|| {
        image_cmd::run(&args(false), Some(&proj)).expect("a plan needs no daemon");
        let mut json = args(false);
        json.json = true;
        image_cmd::run(&json, Some(&proj)).expect("--json needs no daemon either");
    });
    assert!(
        !agentstack::image::context_dir_for("backend").exists(),
        "a plan writes nothing"
    );

    // The build stages everything, then refuses with the two facts a user
    // needs: what is missing, and what to run.
    let err = without_docker(|| image_cmd::run(&args(true), Some(&proj)))
        .expect_err("the build cannot complete without a daemon");
    let message = format!("{err:#}");
    assert!(
        message.contains("no docker client"),
        "the message distinguishes a missing CLIENT from a stopped daemon: {message}"
    );
    assert!(
        message.contains("install Docker"),
        "and names how to get it: {message}"
    );
    let context = agentstack::image::context_dir_for("backend");
    assert!(
        message.contains(&context.display().to_string()),
        "and names the staged context so the work is not lost: {message}"
    );
    assert!(
        message.contains("build --tag"),
        "and hands over the exact command: {message}"
    );

    // The staged context is genuinely complete — the offer to finish it by
    // hand has to be real.
    for rel in [
        "Dockerfile",
        "agentstack/image.json",
        "agentstack/entrypoint.sh",
        "agentstack/required-secrets",
        "agentstack/manifest/agentstack.toml",
        "agentstack/manifest/agentstack.lock",
        "agentstack/servers/kibana.json",
        "agentstack/home/.claude/skills/sql-review/SKILL.md",
    ] {
        assert!(
            context.join(rel).exists(),
            "the staged context is missing {rel}"
        );
    }
}

// ── Docker-gated end to end ────────────────────────────────────────────────

fn docker_up() -> bool {
    Command::new("docker")
        .arg("info")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Report a Docker-gated witness that could not run.
///
/// A skip that returns `Ok` is counted as a PASS by the test runner, so on a
/// developer's machine this reads as "the image is fine" when the image was
/// never built. That is tolerable locally — it keeps the rest of the file
/// runnable without a daemon — and NOT tolerable in CI, where this job exists
/// precisely because the runner ships Docker. A daemon outage there would
/// silently turn this witness green and nobody would learn the image had
/// stopped being proven.
///
/// So: skip loudly off CI, fail on CI.
fn skip_or_fail(reason: &str) {
    assert!(
        std::env::var_os("CI").is_none(),
        "REFUSING to report a skipped Docker witness as a pass on CI: {reason}. \
         This job runs on a runner that ships Docker; if the daemon is missing, \
         the image below was never built and this test proved nothing."
    );
    eprintln!("SKIP: {reason}");
}

fn pull(image: &str) -> bool {
    Command::new("docker")
        .args(["pull", image])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// The real thing, against a real daemon — Docker-gated the way the sandbox
/// e2e tests are (probe, print `SKIP:`, early-return).
///
/// `alpine:3` stands in for a runner image: it has the `/bin/sh` the
/// entrypoint needs and nothing else, which is what makes the two claims under
/// test isolable — the artifact's labels, and the guard refusing to start
/// without the secret it names.
///
///   cargo test -p agentstack --test packaging -- --nocapture --ignored
#[test]
fn a_built_image_carries_its_labels_and_refuses_to_start_without_its_secret() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    if !docker_up() {
        skip_or_fail("no Docker daemon");
        return;
    }
    if !pull("alpine:3") {
        skip_or_fail("cannot pull alpine:3");
        return;
    }

    let tmp = assert_fs::TempDir::new().unwrap();
    let proj = machine(tmp.path());
    lock(&proj);
    trust(&proj);

    let tag = "agentstack-test/packaging:witness";
    let mut a = args(true);
    a.tag = Some(tag.to_string());
    a.from = Some("alpine:3".to_string());
    // `alpine:3` has no `claude`, so the image's CMD would fail — the point
    // here is the GUARD, which runs before the harness is ever exec'd.
    image_cmd::run(&a, Some(&proj)).expect("the build must succeed with a live daemon");

    let labels = Command::new("docker")
        .args(["inspect", "--format", "{{json .Config.Labels}}", tag])
        .output()
        .unwrap();
    let labels = String::from_utf8_lossy(&labels.stdout);
    assert!(
        labels.contains("\"org.agentstack.toolset\":\"backend\""),
        "{labels}"
    );
    assert!(
        labels.contains("\"org.agentstack.posture\":\"sandbox\""),
        "{labels}"
    );

    // No secret in the environment: the entrypoint refuses, names the ref, and
    // never starts the harness.
    let refused = Command::new("docker")
        .args(["run", "--rm", tag, "/bin/sh", "-c", "echo HARNESS_RAN"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&refused.stderr);
    let stdout = String::from_utf8_lossy(&refused.stdout);
    assert_eq!(refused.status.code(), Some(78), "stderr: {stderr}");
    assert!(stderr.contains(SECRET_REF), "{stderr}");
    assert!(!stdout.contains("HARNESS_RAN"), "{stdout}");

    // With the secret present, the guard steps out of the way.
    let allowed = Command::new("docker")
        .args([
            "run",
            "--rm",
            "-e",
            &format!("{SECRET_REF}={SECRET_VALUE}"),
            tag,
            "/bin/sh",
            "-c",
            "echo HARNESS_RAN",
        ])
        .output()
        .unwrap();
    assert!(
        String::from_utf8_lossy(&allowed.stdout).contains("HARNESS_RAN"),
        "stderr: {}",
        String::from_utf8_lossy(&allowed.stderr)
    );

    // And the image itself holds the placeholder, never the value — the same
    // claim as the staged-context witness, asserted on the built layer.
    let inside = Command::new("docker")
        .args([
            "run",
            "--rm",
            "--entrypoint",
            "/bin/sh",
            tag,
            "-c",
            "cat /agentstack/servers/kibana.json /agentstack/required-secrets",
        ])
        .output()
        .unwrap();
    let inside = String::from_utf8_lossy(&inside.stdout);
    assert!(inside.contains(&format!("${{{SECRET_REF}}}")), "{inside}");
    assert!(!inside.contains(SECRET_VALUE), "{inside}");

    let _ = Command::new("docker").args(["rmi", "-f", tag]).status();
}
