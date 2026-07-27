//! The versioned envelope every UI-facing JSON read carries (UI control-plane
//! §"Versioned contracts"). External panels (t3code) decode `schema_version`
//! first: an unknown major means "disable and show the upgrade path", never
//! "guess". `features` names usable end-to-end contracts — a feature appears
//! only when its full read/action loop works in this binary, so a UI can gate
//! each affordance on the named contract instead of sniffing individual
//! fields.
//!
//! This is presentation-layer negotiation only. No enforcement decision may
//! read these fields: the CLI re-validates every precondition on every call
//! whether or not the caller negotiated.

/// Bumped only when an existing field changes meaning or shape. Adding fields
/// or features is backward-compatible and does NOT bump this.
pub const SCHEMA_VERSION: u64 = 1;

/// End-to-end contracts this binary serves. Names are stable identifiers for
/// external UIs; remove one only with a schema-version bump.
///
/// - `init-plan`: `init --plan` emits the detection plan with `plan_digest`.
/// - `apply-setup`: `init --yes --consented-plan <digest>` applies a reviewed
///   plan and refuses when the detected inputs drifted since the plan.
/// - `trust-preview`: `trust --preview` emits the full reviewed surface with
///   `surface_digest`.
/// - `trust-consent`: `trust --yes --consented-digest <digest>` grants bound
///   to the previewed bytes and refuses stale or missing digests.
/// - `status-v1`: `doctor --json` carries `state` + `next_action`.
/// - `profiles-v1`: `use --list --json` lists profiles with readiness.
/// - `diff-v1`: `diff --json` reports drift per target.
/// - `restore-last`: `restore --json` lists undoable writes; `restore --last
///   [--write]` previews/undoes the newest.
/// - `sessions-v1`: `use --list --json` carries per-profile `active` and the
///   top-level `session` object; `session start <profile>` activates
///   fail-closed (refuses untrusted or unpinned surfaces) and `session end`
///   reverts — including a session an interrupted UI left behind.
/// - `profiles-edit-v1`: `library-index` emits the central-library catalog
///   (skills + servers) for the browser; `add-skill-to-profile`,
///   `add-server-to-profile`, `create-profile`, and `use-profile` mutate the
///   toolset then re-lock + re-render, each bound to a `consent_digest` a prior
///   `--preview` returned (apply refuses on drift) and failing closed on an
///   unresolved `${REF}`. NOTE: `create-profile` no longer re-renders — see
///   `toolset-create-v2`, which is the name to gate on for that. This paragraph
///   still describes what a binary advertising ONLY `profiles-edit-v1` does,
///   which is what the name has to keep meaning.
/// - `diff-ownership-v1`: each `diff --json` target carries `managed` (the
///   names this render owns), `hand_edited` (the file no longer matches what we
///   last wrote), and `foreign_untracked` — foreign entries nobody ever declared
///   to agentstack, which `apply` preserves exactly like `kept` but which
///   `adopt` and `apply --prune-foreign` cannot act on. `kept` keeps its
///   `diff-v1` meaning (another MANIFEST's entries, adopt-eligible) precisely so
///   a panel on the older name never offers Adopt for something the CLI cannot
///   honor — the same reasoning as `library-remove-v1`. A UI that does not know
///   this name simply shows one fewer category, which is safe.
/// - `toolset-create-v2`: `create-profile` writes the manifest entry and
///   re-locks, and renders NOTHING — naming a toolset is no longer switching to
///   it (review finding H3). Activation is a separate verb (`use-profile`, or
///   `session start` for a reversible one). A separate name rather than a
///   revision of `profiles-edit-v1`, because a binary advertising that older
///   name legitimately re-renders on create: a panel that showed "created and
///   active" on the strength of `profiles-edit-v1` would be right about the old
///   binary and wrong about this one. `--allow-unresolved` is inert for this
///   verb now (nothing renders, so no `${REF}` resolves), exactly as it already
///   is for `remove-from-library`. The other three `profiles-edit-v1` verbs
///   (`add-skill-to-profile`, `add-server-to-profile`, `use-profile`) still
///   re-lock AND re-render, and are unchanged.
/// - `library-remove-v1`: `remove-from-library --kind skill|server --name <n>`
///   drops a capability from the MACHINE-WIDE central library, bound to a
///   `consent_digest` a prior `--preview` returned — here over the library index
///   bytes, since no manifest is involved. It is the only panel mutation that
///   edits machine state instead of the project (nothing re-locks, nothing
///   re-renders) and it is recoverable: the body and index row move to
///   `lib/.trash`, restorable with `lib trash --restore <id> --write`. A
///   separate name from `profiles-edit-v1` because a binary advertising that
///   contract legitimately predates removal — a panel offering a Remove button
///   on the older name would offer a button the CLI cannot honor.
/// - `workflow-observe-v1`: `workflow list --json` surfaces every declared
///   `[workflows.*]` entry with its per-entry trust + lock state (project-scoped
///   reads), and `workflow runs --json` lists recorded run history. Unlike the
///   other reads, `runs` reads the machine-global runs directory
///   (`agentstack_home()/runs`), not the project — run evidence is not
///   project-scoped. Both are read-only observation; running/resuming re-gates
///   independently.
/// - `workflow-serial-roles-v1`: each `workflow list --json` row carries
///   `serial_roles` — the subset of that workflow's roles whose harness takes
///   no per-child MCP config, so its children launch ONE AT A TIME whatever
///   the concurrency cap says. A separate name rather than a wider reading of
///   `workflow-observe-v1`, because a binary predating this field legitimately
///   advertises that contract without it: folding the field into the older
///   name would make it over-promise, and a UI reading `serial_roles` on the
///   strength of `workflow-observe-v1` would be sniffing a field — the exact
///   thing these names exist to replace.
/// - `doctor-advisories-v1`: `doctor --json` carries a top-level `advisories`
///   count, and section lines can carry `level: "advisory"` — findings that are
///   true and worth stating but are NOT something this project must repair, so
///   they are excluded from `warnings`, from `state`, and from `next_action`.
///   A UI that does not know the name renders advisories as `ok` (its level
///   match falls through), which is safe but silent: the CLI says "1 note" and
///   the panel says nothing. Gating on this name is what lets a panel show the
///   count instead of dropping it (review finding N4).
/// - `doctor-probe-v1`: `doctor --probe` actually STARTS each stdio server,
///   speaks the MCP `initialize` handshake, and stops it again; `doctor --json`
///   then carries a top-level `probe` object — `ran`, `skipped_reason`, and one
///   `servers` entry per stdio server with `status` (`ok` / `failed` /
///   `not_probeable`). A separate name rather than a wider reading of
///   `status-v1`, for two reasons. The first is the usual one: a binary
///   predating this emits `probe: null` and a UI reading the field would be
///   sniffing it. The second matters more — this is the ONLY doctor contract
///   with side effects, so a panel must not offer a "check my servers actually
///   run" button unless the CLI behind it really can spawn, bound, and reap
///   those processes. `ran: false` with a `skipped_reason` is a first-class
///   answer, not an error: the CLI refuses to spawn anything for a project that
///   is not trusted at its current bytes, and a UI that treats that as a
///   failure would push the user to retry instead of to the trust review.
pub const FEATURES: &[&str] = &[
    "init-plan",
    "apply-setup",
    "trust-preview",
    "trust-consent",
    "status-v1",
    "profiles-v1",
    "diff-v1",
    "restore-last",
    "sessions-v1",
    "profiles-edit-v1",
    "diff-ownership-v1",
    "toolset-create-v2",
    "library-remove-v1",
    "workflow-observe-v1",
    "workflow-serial-roles-v1",
    "doctor-advisories-v1",
    "doctor-probe-v1",
];

/// Wrap a response body in the envelope. The two envelope keys are injected
/// into the body object so existing consumers keep their field paths; a
/// non-object body would be a programming error and panics in debug builds.
pub fn envelope(body: serde_json::Value) -> serde_json::Value {
    let mut map = match body {
        serde_json::Value::Object(map) => map,
        other => {
            debug_assert!(false, "envelope() needs a JSON object, got {other}");
            let mut map = serde_json::Map::new();
            map.insert("body".into(), other);
            map
        }
    };
    map.insert("schema_version".into(), SCHEMA_VERSION.into());
    map.insert(
        "features".into(),
        serde_json::Value::Array(FEATURES.iter().map(|f| (*f).into()).collect()),
    );
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_injects_version_and_features_without_touching_body() {
        let out = envelope(serde_json::json!({"a": 1}));
        assert_eq!(out["schema_version"], SCHEMA_VERSION);
        assert_eq!(out["a"], 1);
        let features: Vec<&str> = out["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(features, FEATURES);
        // Every name ever shipped must still be SERVED. That — not position —
        // is the property external UIs depend on: they gate an affordance with
        // `features.includes("<name>")`, so dropping or renaming one silently
        // disables a working button, while inserting a new name anywhere is
        // harmless. An earlier version of this test pinned the order instead
        // and failed the moment a contract was added mid-list, which tested
        // the test rather than the contract. Removing a name is a
        // schema-version bump, so it must break here first.
        for shipped in [
            "init-plan",
            "apply-setup",
            "trust-preview",
            "trust-consent",
            "status-v1",
            "profiles-v1",
            "diff-v1",
            "restore-last",
            "sessions-v1",
            "profiles-edit-v1",
            "workflow-observe-v1",
            "workflow-serial-roles-v1",
            "doctor-advisories-v1",
        ] {
            assert!(
                features.contains(&shipped),
                "FEATURES dropped '{shipped}' — a UI gating on that name loses a working \
                 affordance, so removing one needs a schema-version bump"
            );
        }
        // No duplicates: a repeated name means two contracts think they own it.
        let mut sorted = features.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "FEATURES contains a duplicate name");
    }
}
