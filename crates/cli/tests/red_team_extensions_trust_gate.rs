// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! RED TEAM — the trust gate on `[extensions.*]`, and the one layer it must
//! NOT close over.
//!
//! A native extension is code the harness loads inside its own process at full
//! user permission, outside every policy ceiling, so `render::extensions`
//! refuses to write a byte of it for a project that is untrusted or drifted.
//! That is the rule, and the first test here is its negative control: an
//! untrusted repository declaring an extension renders nothing.
//!
//! The second test guards the other direction. The machine's own manifest
//! (`$AGENTSTACK_HOME/agentstack.toml`) is the user's personal layer, not a
//! project's content. It is deliberately undiscoverable as a project
//! (`manifest::discover_project_base`), so no `agentstack trust` invocation can
//! ever reach it — its project root resolves to `$HOME`, which nothing can put
//! in the trust store. Gating it would refuse machine-level extensions forever
//! while naming a command that cannot help: a gate nobody can satisfy is a
//! broken feature, not a stronger one. So the gate exempts it, exactly as
//! `render::hooks::trust_refusal` exempts machine-level hooks, and the two
//! halves are witnessed together here — the exemption plus the untrusted
//! project that still renders zero bytes, which is what proves the gate was
//! narrowed rather than removed.

use std::fs;
use std::path::{Path, PathBuf};

/// The extension body. Its presence in a harness extension directory is the
/// proof that extension bytes were delivered.
const EXT_BODY: &str = "export default (pi) => {} // checkpoint\n";

/// The manifest declaring one pi extension from a source dir beside it. It also
/// carries an instructions fragment — not decoration: an extensions-only apply
/// reports "nothing was delivered" (a global extension artifact is not counted
/// as rendered content) and exits nonzero for reasons that have nothing to do
/// with this gate, which would blur what the exit code below is asserting.
const EXT_MANIFEST: &str = "version = 1\n\
                            [instructions.house]\n\
                            path = \"./instructions/house.md\"\n\
                            [extensions.checkpoint]\n\
                            path = \"./extensions/checkpoint\"\n\
                            target = \"pi\"\n";

fn run(args: &[&str], home: &Path, cwd: &Path) -> (String, bool) {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_agentstack"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("HOME", home)
        .env("AGENTSTACK_HOME", home.join(".agentstack"))
        .env("PATH", "/usr/bin:/bin")
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

/// Write the extension source and the manifest that declares it into `dir`.
fn declare_extension(dir: &Path) {
    fs::create_dir_all(dir.join("extensions/checkpoint")).unwrap();
    fs::write(dir.join("extensions/checkpoint/index.ts"), EXT_BODY).unwrap();
    fs::create_dir_all(dir.join("instructions")).unwrap();
    fs::write(dir.join("instructions/house.md"), "# house rules\n").unwrap();
    fs::write(dir.join("agentstack.toml"), EXT_MANIFEST).unwrap();
}

/// Pin the declared extension. Pinning is not consent: it is what keeps the
/// refusal below firing on "not trusted" rather than "not pinned", which would
/// prove nothing about the trust gate.
fn pin(home: &Path, cwd: &Path, manifest_dir: &Path) {
    let dir = manifest_dir.display().to_string();
    let (text, ok) = run(&["--manifest-dir", &dir, "lock", "--write"], home, cwd);
    assert!(ok, "lock failed:\n{text}");
}

/// pi's global extensions directory (`~/.pi/agent/extensions`) — where a
/// global-scope render lands.
fn global_ext_dir(home: &Path) -> PathBuf {
    home.join(".pi/agent/extensions")
}

// ------------------------------------------------- the gate (negative control)

/// A project nobody has trusted renders ZERO extension bytes. Without this,
/// the exemption below could be an accidental "the gate never fires".
#[test]
fn an_untrusted_project_renders_no_extension_bytes() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let proj = tmp.path().join("proj");
    fs::create_dir_all(&home).unwrap();
    fs::create_dir_all(&proj).unwrap();
    declare_extension(&proj);
    pin(&home, &proj, &proj);

    let (text, ok) = run(&["apply", "--write", "--scope", "project"], &home, &proj);
    assert!(
        !ok,
        "apply --write exited 0 on an untrusted project — a script cannot tell \
         this from success:\n{text}"
    );
    assert!(
        text.contains("refusing to render native extensions"),
        "the refusal must name what it refused:\n{text}"
    );
    assert!(
        text.contains("agentstack trust"),
        "the refusal must name the command that answers it:\n{text}"
    );

    let dir = proj.join(".pi/extensions");
    let empty = !dir.exists() || fs::read_dir(&dir).unwrap().next().is_none();
    assert!(
        empty,
        "an untrusted project's extension bytes reached {}",
        dir.display()
    );
}

// ------------------------------------------------------- the other direction

/// The machine's own manifest is the personal layer, not a project: nothing can
/// trust it, so nothing may gate it on trust. Its extensions must render.
#[test]
fn the_machine_manifests_own_extensions_are_not_gated_on_a_project_it_cannot_trust() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let machine = home.join(".agentstack");
    fs::create_dir_all(&machine).unwrap();
    declare_extension(&machine);
    pin(&home, tmp.path(), &machine);

    let dir = machine.display().to_string();
    let (text, ok) = run(
        &["--manifest-dir", &dir, "apply", "--write"],
        &home,
        tmp.path(),
    );
    assert!(ok, "the machine manifest's own apply failed:\n{text}");

    let artifact = global_ext_dir(&home).join("checkpoint/index.ts");
    assert_eq!(
        fs::read_to_string(&artifact).unwrap_or_default(),
        EXT_BODY,
        "the machine layer's own extension was gated on a project that cannot \
         exist — expected bytes at {}:\n{text}",
        artifact.display()
    );
}
