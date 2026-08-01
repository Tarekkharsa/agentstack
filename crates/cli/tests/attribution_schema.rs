// Integration test: unwraps/expects in free helper fns aren't seen as
// "in tests" by clippy's allow-unwrap-in-tests (only #[test] fns are),
// so opt the whole test file out of the workspace unwrap_used deny.
#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Phase 4 — attribution capture in the lock schema.
//!
//! `license` and `origin` join `[[skill]]`, so an upstream LICENSE/NOTICE
//! obligation is carried mechanically rather than by promise. This is the one
//! sanctioned schema change of the phase, and its discipline is what these
//! witnesses are for:
//!
//! 1. **Additive.** Every lock written before these fields still parses, and
//!    the schema version is deliberately NOT bumped.
//! 2. **Preserved.** A re-lock rebuilds entries from resolved state, which has
//!    no idea where content came from. If its `None` won, an ordinary
//!    `install` would erase the attribution — the precise "carried by promise"
//!    failure the fields exist to remove.
//! 3. **Re-gating is the mechanism, not a bug.** Lock bytes are consent-digest
//!    material by design. The first re-lock that records an attribution field
//!    changes the lock, which changes the digest, which reopens the review.
//!    That is asserted here rather than worked around, because the temptation
//!    to "fix" it by excluding these fields from the digest is exactly how
//!    content-binding gets holes.

use agentstack_core::lock::{Lock, LockedSkill, SkillLockSource, SUPPORTED_LOCK_VERSION};

fn entry(name: &str, license: Option<&str>, origin: Option<&str>) -> LockedSkill {
    LockedSkill {
        name: name.to_string(),
        source: SkillLockSource::Path,
        path: Some(format!("./skills/{name}")),
        git: None,
        rev: None,
        checksum: agentstack_core::digest::Sha256Hex::parse(&"a".repeat(64))
            .expect("a valid sha256"),
        license: license.map(str::to_string),
        origin: origin.map(str::to_string),
    }
}

/// A lock written before these fields existed still parses, with the new keys
/// simply absent. If this fails, every existing project breaks on upgrade.
#[test]
fn a_legacy_lock_without_attribution_still_loads() {
    let legacy = r#"
version = 2

[[skill]]
name = "summarize"
source = "path"
path = "./skills/summarize"
checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;
    let lock = Lock::parse(legacy, std::path::Path::new("agentstack.lock"))
        .expect("a pre-attribution lock must still parse");
    assert_eq!(lock.skills.len(), 1);
    assert_eq!(lock.skills[0].license, None, "absent, not invented");
    assert_eq!(lock.skills[0].origin, None);
}

/// And a v1 lock — the oldest shape — is unaffected too.
#[test]
fn a_v1_lock_is_unaffected_by_this_change() {
    let v1 = r#"
version = 1

[[skill]]
name = "summarize"
source = "path"
path = "./skills/summarize"
checksum = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
"#;
    let lock = Lock::parse(v1, std::path::Path::new("agentstack.lock"))
        .expect("a v1 lock must still parse");
    assert_eq!(lock.skills.len(), 1);
}

/// The version was NOT bumped, and that is a decision rather than an omission:
/// these are `serde(default)` optional fields, so no older binary
/// misinterprets them — it would rewrite them away, which changes the lock
/// bytes and forces a re-review rather than losing them silently.
#[test]
fn the_schema_version_was_deliberately_not_bumped() {
    assert_eq!(
        SUPPORTED_LOCK_VERSION, 2,
        "bumping this would refuse every lock in the wild over two optional \
         fields that parse fine without it"
    );
    let src = include_str!("../../core/src/lock.rs");
    assert!(
        src.contains("Phase 4 added `license` and `origin`"),
        "the version constant's doc comment must record WHY this change did \
         not bump it — an unexplained non-bump is indistinguishable from a \
         forgotten one"
    );
}

/// The preservation property, at the choke point. A re-lock hands `upsert` an
/// entry rebuilt from resolved state, with no attribution on it. That must not
/// erase what intake recorded.
#[test]
fn a_relock_cannot_launder_attribution_away() {
    let mut lock = Lock::default();
    lock.upsert(entry(
        "summarize",
        Some("Apache-2.0"),
        Some("eve.dev/r/summarize"),
    ));

    // What every ordinary re-lock produces: same content, no idea of origin.
    lock.upsert(entry("summarize", None, None));

    assert_eq!(
        lock.skills[0].license.as_deref(),
        Some("Apache-2.0"),
        "an ordinary re-lock must not erase a licence obligation"
    );
    assert_eq!(
        lock.skills[0].origin.as_deref(),
        Some("eve.dev/r/summarize"),
        "nor where the content came from"
    );
}

/// Preservation is not stickiness: re-importing from a source that now
/// declares something different updates it. Without this, a wrong licence
/// recorded once could never be corrected.
#[test]
fn a_new_declared_licence_does_replace_the_old_one() {
    let mut lock = Lock::default();
    lock.upsert(entry("summarize", Some("Apache-2.0"), Some("a")));
    lock.upsert(entry("summarize", Some("MIT"), Some("b")));
    assert_eq!(lock.skills[0].license.as_deref(), Some("MIT"));
    assert_eq!(lock.skills[0].origin.as_deref(), Some("b"));
}

/// Recording attribution changes the lock's bytes — and so re-gates consent.
///
/// Asserted deliberately. Lock bytes are consent-digest material by design, so
/// this is the content-binding mechanism doing its job: the description of what
/// the user approved has changed, and they get to see that. Anyone tempted to
/// suppress the re-gate by excluding these fields from the digest should read
/// invariant 4 first — that is how content binding acquires holes.
#[test]
fn recording_attribution_changes_the_lock_bytes_and_therefore_re_gates() {
    let mut bare = Lock::default();
    bare.upsert(entry("summarize", None, None));

    let mut attributed = Lock::default();
    attributed.upsert(entry("summarize", Some("Apache-2.0"), Some("eve.dev")));

    let a = toml::to_string(&bare).expect("serializable");
    let b = toml::to_string(&attributed).expect("serializable");
    assert_ne!(
        a, b,
        "if these matched, attribution would be invisible to the trust digest \
         — which would mean the lock no longer describes what was consented to"
    );
    assert!(
        b.contains("Apache-2.0") && b.contains("eve.dev"),
        "and the recorded values must actually be in the bytes:\n{b}"
    );
    // The bare form must not carry empty keys: `skip_serializing_if` keeps a
    // project that imported nothing byte-identical to before this change.
    assert!(
        !a.contains("license") && !a.contains("origin"),
        "a project with no attribution must serialize exactly as it did before, \
         or this change re-gates every project on earth for nothing:\n{a}"
    );
}
