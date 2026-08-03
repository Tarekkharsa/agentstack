//! Finding: the crate boundary.
//!
//! The crate doc claims this domain "has **no** internal dependency edges: Boa
//! can never reach `trust`, `policy`, `core`, `adapters`, `recorder`, or any
//! enforcement path." That was true by convention and enforced by nothing — a
//! single line in a `Cargo.toml` could have dissolved it silently, which is
//! exactly the failure mode a witness exists to catch.
//!
//! **What this proves:** `boa_engine` is confined to this crate, and this
//! crate depends on no other agentstack crate, so no Boa-reachable code path
//! can *call* an enforcement API.
//!
//! **What it does NOT prove**, and `POSTURE_LABEL` says so at length: this is a
//! **compile-time reach** boundary, not a **runtime memory** boundary. The
//! crate still links into the `agentstack` process, whose address space also
//! holds the `CommitmentKey` and secrets resolved in flight. A Boa
//! memory-safety bug stays a whole-process concern; only the recorded
//! QuickJS-in-wasmtime fallback would add runtime isolation.

use std::path::{Path, PathBuf};

/// The workspace `crates/` directory, found from this test's own manifest.
fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/workflow has a parent")
        .to_path_buf()
}

/// The `[dependencies]`-ish region of a Cargo manifest: everything from the
/// first `dependencies` table to EOF. Deliberately crude — this is a lint over
/// text we control, and a crude reading that errs toward *more* matches is the
/// safe direction for a boundary check.
fn dependency_text(manifest: &Path) -> String {
    let text = std::fs::read_to_string(manifest)
        .unwrap_or_else(|e| panic!("read {}: {e}", manifest.display()));
    match text.find("dependencies]") {
        Some(start) => text[start..].to_string(),
        None => String::new(),
    }
}

fn crate_manifests() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(crates_dir()).expect("read crates/") {
        let entry = entry.expect("dir entry");
        let manifest = entry.path().join("Cargo.toml");
        if manifest.is_file() {
            let name = entry.file_name().to_string_lossy().into_owned();
            out.push((name, manifest));
        }
    }
    assert!(
        out.len() > 5,
        "expected the full workspace crate set, found {}",
        out.len()
    );
    out
}

#[test]
fn the_workflow_crate_has_no_authority_reach() {
    let manifests = crate_manifests();

    // 1. Boa lives HERE — asserted positively, so the test cannot pass by
    //    accidentally looking at the wrong directory or an empty set.
    let workflow_deps = dependency_text(&crates_dir().join("workflow/Cargo.toml"));
    assert!(
        workflow_deps.contains("boa_engine"),
        "crates/workflow must be the crate that owns the Boa dependency"
    );

    // 2. Boa lives ONLY here. The approved dependency is approved *isolated*;
    //    a second crate taking it would put an interpreter next to an
    //    enforcement path without anyone re-reviewing that.
    for (name, manifest) in &manifests {
        if name == "workflow" {
            continue;
        }
        let deps = dependency_text(manifest);
        for line in deps.lines() {
            let line = line.trim();
            if line.starts_with('#') {
                continue;
            }
            assert!(
                !line.starts_with("boa"),
                "crate {name:?} takes a Boa dependency ({line:?}); boa_engine is approved only \
                 for crates/workflow, isolated from every enforcement path"
            );
        }
    }

    // 3. This crate reaches no agentstack crate. This is the actual "no
    //    authority reach" claim: Boa's code cannot CALL trust, policy, core,
    //    adapters, recorder, executor, egress, runtime, or the CLI.
    for line in workflow_deps.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        assert!(
            !line.starts_with("agentstack"),
            "crates/workflow took an internal dependency ({line:?}); the domain must stay \
             self-contained — hostile script text in, brokered spawn requests out"
        );
    }

    // 4. The no-unsafe floor the whole posture rests on (invariant 1).
    let lib = std::fs::read_to_string(crates_dir().join("workflow/src/lib.rs")).expect("read lib");
    assert!(
        lib.contains("#![forbid(unsafe_code)]"),
        "crates/workflow must keep #![forbid(unsafe_code)]"
    );
}
