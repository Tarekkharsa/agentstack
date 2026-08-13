// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! A preference write must not wall the user off from the next command.
//!
//! The defect these tests pin: `agentstack more delivery render-locally --write`
//! records `[delivery] render_locally` and nothing else — a ROUTING choice over
//! capabilities the human already declared and already reviewed. But the
//! manifest bytes ARE the consent digest, so the write flipped a trusted
//! project to `Changed`, and the very next step of the documented journey
//! (`agentstack apply --write`, which the command itself prints) was refused by
//! a gate that this command's own bytes had tripped. Nothing new was
//! authorized, and a wall appeared anyway.
//!
//! `crate::trust_carry::TrustCarry` carries valid trust across that write. The
//! four tests below are the witnesses for its four properties: the carry
//! happens when trust was valid, and CANNOT happen otherwise — untrusted stays
//! untrusted, and a pending review stays pending.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use agentstack::cli::{ApplyArgs, DeliveryArgs, DeliveryCmd};
use agentstack::commands::{apply, delivery};
use agentstack::trust::{self, TrustState};

// These commands read and write the process-global HOME and the machine-wide
// trust store; serialize them.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn setup(home: &Path) {
    fs::create_dir_all(home).unwrap();
    std::env::set_var("HOME", home);
    std::env::set_var("AGENTSTACK_HOME", home.join(".agentstack"));
}

/// One project, one stdio server, one MCP-capable target. No `[delivery]`
/// table: the routing starts automatic, which is what makes the
/// `render-locally` write a real change rather than a no-op.
fn project(root: &Path) -> std::path::PathBuf {
    let proj = root.join("proj");
    fs::create_dir_all(&proj).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        "version = 1\n\
         [targets]\ndefault = [\"claude-code\"]\n\
         [servers.demo]\ntype = \"stdio\"\ncommand = \"/bin/echo\"\nargs = [\"hi\"]\n",
    )
    .unwrap();
    proj
}

fn render_locally_write() -> DeliveryArgs {
    DeliveryArgs {
        command: Some(DeliveryCmd::RenderLocally {
            harness: None,
            off: false,
            write: true,
        }),
        json: false,
    }
}

fn apply_write() -> ApplyArgs {
    ApplyArgs {
        verbose: false,
        targets: vec![],
        profile: None,
        dry_run: false,
        write: true,
        // The fixture asserts against the global ~/.claude.json; in a repo the
        // default scope would be project, so pin global explicitly.
        scope: Some(agentstack::scope::Scope::Global),
        allow_unresolved: false,
        prune_foreign: false,
        no_gitignore: true,
    }
}

/// The server definition reached a native config — i.e. the delivery gate let
/// the render through rather than refusing it.
fn servers_were_delivered(home: &Path) -> bool {
    fs::read_to_string(home.join(".claude.json"))
        .map(|text| text.contains("demo"))
        .unwrap_or(false)
}

/// (a) Trusted before → still trusted after, and the next command delivers.
///
/// This is the failing user journey from the example suite, in one test:
/// record the routing preference, then apply. Before the carry, the apply
/// refused with "changed since it was trusted" and wrote nothing.
#[test]
fn a_trusted_project_stays_trusted_and_the_next_command_delivers() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    let proj = project(tmp.path());
    trust::trust_unreviewed(&proj).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);

    delivery::run(&render_locally_write(), Some(&proj)).unwrap();

    // The write really happened — otherwise the rest of this test is vacuous.
    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    assert!(
        manifest.contains("render_locally"),
        "the preference was not recorded: {manifest}"
    );
    assert_eq!(
        trust::check(&proj),
        TrustState::Trusted,
        "a routing preference authorizes no new content — trust must be carried across its write"
    );

    apply::run(&apply_write(), Some(&proj)).unwrap();
    assert!(
        servers_were_delivered(&home),
        "the documented next step must not be refused by the previous step's own bytes"
    );
}

/// (b) Untrusted before → still untrusted, and the gate still refuses.
///
/// The property: `trust::repin` only ever UPDATES an existing entry. Were it
/// able to CREATE one, this project would read `Trusted` here and the render
/// below would land a server definition into a native config with no human
/// ever having reviewed it — a preference command minting trust. The unit
/// witness for the store-level rule is
/// `agentstack_trust::tests::repin_updates_existing_entry_only_and_preserves_surface`;
/// this is the end-to-end one, through the command that calls it.
#[test]
fn b_an_untrusted_project_is_never_granted_trust_by_the_preference_write() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    let proj = project(tmp.path());
    assert_eq!(trust::check(&proj), TrustState::Untrusted);

    delivery::run(&render_locally_write(), Some(&proj)).unwrap();

    assert_eq!(
        trust::check(&proj),
        TrustState::Untrusted,
        "re-pinning must never create trust — an untrusted project stays untrusted"
    );
    // And the refusal is not merely a reading: nothing is delivered.
    apply::run(&apply_write(), Some(&proj)).ok();
    assert!(
        !servers_were_delivered(&home),
        "the trust gate must still refuse an untrusted project after the preference write"
    );
}

/// (c) Changed before → still changed. Pending review stays pending.
///
/// The property: the capture is `Some` only when the project read `Trusted` at
/// that instant. A project already drifted by a human edit captures `None`, so
/// the carry is a no-op and the edit the human owes a review for is not
/// silently absorbed by an unrelated preference command.
#[test]
fn c_a_pending_review_stays_pending_across_the_preference_write() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    let proj = project(tmp.path());
    trust::trust_unreviewed(&proj).unwrap();

    // A human edit after trust: a SECOND server nobody has reviewed.
    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        format!("{manifest}[servers.smuggled]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\n"),
    )
    .unwrap();
    assert_eq!(trust::check(&proj), TrustState::Changed);

    delivery::run(&render_locally_write(), Some(&proj)).unwrap();

    assert_eq!(
        trust::check(&proj),
        TrustState::Changed,
        "a drifted project must stay drifted — the carry only ever covers a VALID prior trust"
    );
    apply::run(&apply_write(), Some(&proj)).ok();
    assert!(
        !servers_were_delivered(&home),
        "the unreviewed edit must not be delivered under a preference command's cover"
    );
}

/// (d) The re-pinned digest is the command's OWN bytes, not whatever is on disk
/// afterwards.
///
/// A hostile edit that lands between the pre-write capture and the write is
/// overwritten by this command (it rewrites the whole file from text it read
/// first), so what is on disk at the end is exactly what was digested. The
/// witness that the store holds the WRITTEN bytes and not a post-write re-read:
/// append to the manifest immediately after the command returns and the project
/// must read `Changed` at once. A post-write disk re-read would have blessed
/// whatever it found; a spliced pre-write snapshot cannot.
#[test]
fn d_the_carried_digest_covers_only_the_bytes_this_command_wrote() {
    let _g = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let tmp = assert_fs::TempDir::new().unwrap();
    let home = tmp.path().join("home");
    setup(&home);
    let proj = project(tmp.path());
    trust::trust_unreviewed(&proj).unwrap();

    delivery::run(&render_locally_write(), Some(&proj)).unwrap();
    assert_eq!(trust::check(&proj), TrustState::Trusted);

    let manifest = fs::read_to_string(proj.join("agentstack.toml")).unwrap();
    fs::write(
        proj.join("agentstack.toml"),
        format!("{manifest}[servers.smuggled]\ntype = \"stdio\"\ncommand = \"/bin/sh\"\n"),
    )
    .unwrap();
    assert_eq!(
        trust::check(&proj),
        TrustState::Changed,
        "the store must hold the digest of the bytes the command wrote, nothing wider"
    );
}
