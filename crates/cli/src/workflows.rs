//! Governed-workflow admission (D7 W1): the single choke point everything
//! workflow-shaped must pass through before a name is even invocable.
//!
//! [`normalized_workflows`] is the seam the W3 engine will call — and,
//! deliberately, NOTHING calls it today (W1 ships core + trust only; there is
//! no `workflow run`, no engine, no spawning). Wiring it to a command without
//! the engine's own witnesses is exactly the premature surface W1 exists to
//! avoid.
//!
//! The admission order is fixed and security-relevant:
//!
//! 1. **Trust gate FIRST** (rule 3): an untrusted bundle's workflows never
//!    validate, never resolve, never normalize — the names are not invocable
//!    and no source byte is interpreted.
//! 2. **Static validation** — the same findings `validate.rs` reports
//!    (name/source/roles/bounds), refused here so a half-declared workflow
//!    can never be admitted even if a caller skipped `doctor`.
//! 3. **Strict lock verification** — every workflow's current source digest,
//!    sorted role set, and rev must MATCH its pin (rule 4). Drift, an
//!    unpinned entry, an unresolvable source, and an offline-unverifiable
//!    git source all refuse: admission needs verified bytes, not absent
//!    evidence.
//! 4. **Ceiling intersection** — effective limits are
//!    min(manifest request, machine `[policy.workflows]` cap), the
//!    `MachineLimits` discipline: requests can only reduce, never increase
//!    (rule 2).

use std::path::Path;

use anyhow::Result;

use crate::lock::Lock;
use crate::manifest::{Manifest, WorkflowPolicy};
use crate::store::Store;

/// Built-in request defaults when a `[workflows.X]` entry declares no ceiling
/// of its own. Deliberately conservative (the design doc's example workflow,
/// not the validation maxima); W3 may tune them, machine policy may cap them.
pub const DEFAULT_MAX_AGENTS: u32 = 25;
pub const DEFAULT_MAX_WALL_SECONDS: u64 = 1800;

/// One workflow that passed the full admission chain: verified pinned source
/// plus the EFFECTIVE (already machine-capped) ceilings. This is the only
/// workflow shape the W3 engine may ever consume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedWorkflow {
    pub name: String,
    /// Sorted-unique role profiles the script's `agent()` calls may name — a
    /// closed set, verified against the lock pin.
    pub roles: Vec<String>,
    /// Effective spawn ceiling: min(manifest request, machine cap).
    pub max_agents: u32,
    /// Effective wall-clock ceiling in seconds: min(request, machine cap).
    pub max_wall_seconds: u64,
    /// The verified strict integrity-root digest of the source.
    pub checksum: String,
    /// The exact digest anchor — a later read of the script must use this
    /// same `(anchor, declared)` pair so the bytes read are the bytes pinned.
    pub anchor: std::path::PathBuf,
    /// The declared path under `anchor` the digest walked.
    pub declared: String,
}

/// Admit every declared workflow or refuse with a named reason — the W3
/// admission choke point (see the module docs for the fixed order). Returns
/// the full normalized set; any single failure refuses the whole call, so a
/// caller can never iterate a half-admitted surface.
///
/// `base` is the project root the trust grant is anchored at;
/// `manifest_dir` is where declared `path` sources anchor (they differ when
/// the manifest lives in `.agentstack/`). `machine` is the MACHINE layer's
/// `[policy.workflows]` — callers must never pass the project manifest's own
/// table here (a repo cannot cap itself into legitimacy, and passing its
/// table as the cap would let it widen the machine's — rule 2).
pub fn normalized_workflows(
    base: &Path,
    manifest: &Manifest,
    manifest_dir: &Path,
    store: &Store,
    lock: &Lock,
    machine: &WorkflowPolicy,
) -> Result<Vec<NormalizedWorkflow>> {
    // 1. Trust gate FIRST — before any validation touches (or error text
    // echoes) the hostile declarations. Untrusted means inert: no name from
    // this manifest is invocable, full stop.
    match crate::trust::check(base) {
        crate::trust::TrustState::Trusted => {}
        crate::trust::TrustState::Changed => anyhow::bail!(
            "refusing to normalize workflows: {} is trusted but its manifest or lock changed since review — re-review and re-grant with `agentstack trust .`",
            base.display()
        ),
        crate::trust::TrustState::Untrusted => anyhow::bail!(
            "refusing to normalize workflows: {} is not trusted — nothing from an untrusted bundle normalizes or is invocable; review and grant with `agentstack trust .`",
            base.display()
        ),
    }

    // 2. Static validation: the workflow findings are all errors, and the
    // choke point refuses on them even though `lock`/`doctor` report them
    // earlier — defense in depth, not a new rule set.
    let workflow_issues: Vec<String> = crate::manifest::validate::validate(manifest)
        .into_iter()
        .filter(|i| {
            use crate::manifest::validate::IssueKind::*;
            matches!(
                i.kind,
                InvalidWorkflowName
                    | InvalidWorkflowSource
                    | UnknownWorkflowRole
                    | InvalidWorkflowBounds
            )
        })
        .map(|i| i.message)
        .collect();
    if !workflow_issues.is_empty() {
        anyhow::bail!(
            "refusing to normalize workflows: {} validation error(s):\n  {}",
            workflow_issues.len(),
            workflow_issues.join("\n  ")
        );
    }

    // 3 + 4. Per workflow: strict lock verification, then ceiling
    // intersection. Read-only — never fetch here; an offline-unverifiable
    // git source refuses (admission needs verified bytes).
    let mut out = Vec::new();
    for (name, wf) in &manifest.workflows {
        let resolved = crate::resolve::resolve_workflow_entry(
            name,
            wf,
            manifest_dir,
            store,
            crate::resolve::ResolveMode::NoFetch,
        )
        .map_err(|e| anyhow::anyhow!("refusing to normalize workflow '{name}': {e:#}"))?;
        let status = crate::resolve::classify_workflow(
            name,
            &resolved.checksum,
            &resolved.roles,
            resolved.rev.as_deref(),
            lock,
        );
        match status {
            crate::resolve::WorkflowLockStatus::Matches => {}
            other => anyhow::bail!(
                "refusing to normalize workflow '{name}': lock verification failed ({other:?}) — run `agentstack lock`, review, and re-trust"
            ),
        }
        // Effective ceilings: the manifest requests (or the built-in default
        // stands in), the machine cap clamps. min() — a request can only
        // reduce the cap, never raise it (rule 2).
        let requested_agents = wf.max_agents.unwrap_or(DEFAULT_MAX_AGENTS);
        let max_agents = match machine.max_agents {
            Some(cap) => requested_agents.min(cap),
            None => requested_agents,
        };
        let requested_wall = wf.max_wall_seconds.unwrap_or(DEFAULT_MAX_WALL_SECONDS);
        let max_wall_seconds = match machine.max_wall_seconds {
            Some(cap) => requested_wall.min(cap),
            None => requested_wall,
        };
        out.push(NormalizedWorkflow {
            name: name.clone(),
            roles: resolved.roles,
            max_agents,
            max_wall_seconds,
            checksum: resolved.checksum,
            anchor: resolved.anchor,
            declared: resolved.declared,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// A project with one declared+pinned workflow (roles: reader), returning
    /// (tempdir, manifest, store). The manifest lives at the project root, so
    /// base == manifest_dir.
    fn pinned_project(
        proj: &assert_fs::TempDir,
        manifest_toml: &str,
    ) -> (Manifest, crate::store::Store) {
        proj.child("workflows/audit/main.js")
            .write_str("export const meta = { name: 'audit' } // v1")
            .unwrap();
        proj.child("agentstack.toml")
            .write_str(manifest_toml)
            .unwrap();
        let manifest: Manifest = toml::from_str(manifest_toml).unwrap();
        let store = crate::store::Store::with_root(proj.child("store").path().to_path_buf());
        crate::commands::lock::record_workflow_pins(proj.path(), &manifest, &store).unwrap();
        (manifest, store)
    }

    const MANIFEST: &str = r#"
        version = 1
        [profiles.reader]
        [workflows.audit]
        path = "./workflows/audit"
        roles = ["reader"]
        max_agents = 100
        "#;

    /// D7 W1 witnesses, all behind one env-scoped home:
    /// - untrusted bundle → nothing normalizes, the refusal names the gate;
    /// - trusted + pinned → normalizes, and a manifest `max_agents` ABOVE the
    ///   machine `[policy.workflows]` cap is CLAMPED to the cap (rule 2:
    ///   requests reduce, never increase), while the wall clock falls back to
    ///   the built-in default untouched by an absent cap;
    /// - a roles widening with unchanged bytes is refused as RolesDrift.
    #[test]
    fn trust_gate_ceiling_clamp_and_roles_drift() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let proj = assert_fs::TempDir::new().unwrap();
        let (manifest, store) = pinned_project(&proj, MANIFEST);
        let lock = Lock::load(proj.path()).unwrap();
        let machine = WorkflowPolicy {
            max_agents: Some(10),
            max_wall_seconds: None,
            max_concurrent: None,
        };

        // Untrusted → refuses before anything resolves; no name invocable.
        let err =
            normalized_workflows(proj.path(), &manifest, proj.path(), &store, &lock, &machine)
                .unwrap_err()
                .to_string();
        assert!(err.contains("not trusted"), "{err}");

        // Trust the project (the low-level grant; the human review path is
        // exercised by the trust-command tests).
        crate::trust::trust(proj.path()).unwrap();

        let admitted =
            normalized_workflows(proj.path(), &manifest, proj.path(), &store, &lock, &machine)
                .unwrap();
        assert_eq!(admitted.len(), 1);
        let wf = &admitted[0];
        assert_eq!(wf.roles, vec!["reader".to_string()]);
        assert_eq!(
            wf.max_agents, 10,
            "manifest requested 100, machine caps at 10 — clamped, never raised"
        );
        assert_eq!(
            wf.max_wall_seconds, DEFAULT_MAX_WALL_SECONDS,
            "no request and no machine cap → the built-in default"
        );

        // Roles widening with unchanged bytes: same source, one more role in
        // the manifest → RolesDrift refusal at admission.
        let widened: Manifest = toml::from_str(
            r#"
            version = 1
            [profiles.reader]
            [profiles.writer]
            [workflows.audit]
            path = "./workflows/audit"
            roles = ["reader", "writer"]
            "#,
        )
        .unwrap();
        let err = normalized_workflows(proj.path(), &widened, proj.path(), &store, &lock, &machine)
            .unwrap_err()
            .to_string();
        assert!(err.contains("RolesDrift"), "{err}");

        std::env::remove_var("AGENTSTACK_HOME");
    }

    /// A role naming no declared profile is refused at the choke point too —
    /// the same `UnknownWorkflowRole` finding validate.rs reports, proven
    /// where admission lives.
    #[test]
    fn undeclared_role_refuses_at_admission() {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        let proj = assert_fs::TempDir::new().unwrap();
        let toml_text = r#"
            version = 1
            [workflows.audit]
            path = "./workflows/audit"
            roles = ["ghost"]
            "#;
        let (manifest, store) = pinned_project(&proj, toml_text);
        let lock = Lock::load(proj.path()).unwrap();
        crate::trust::trust(proj.path()).unwrap();

        let err = normalized_workflows(
            proj.path(),
            &manifest,
            proj.path(),
            &store,
            &lock,
            &WorkflowPolicy::default(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("ghost"), "{err}");
        assert!(err.contains("profiles"), "{err}");

        std::env::remove_var("AGENTSTACK_HOME");
    }
}
