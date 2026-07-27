//! The child-placement seam (scaling plan Phase 5).
//!
//! One implementation ships: [`LocalDispatcher`], which is exactly the shipped
//! behaviour. The seam exists now, with nothing behind it but the local path,
//! so that the *shape of the request* is designed before there is a network to
//! carry it — a wire format invented alongside its first remote consumer is
//! how authority-bearing fields get added by accident.
//!
//! # The load-bearing rule
//!
//! [`TaskDescriptor`] carries **digests and names, never authority**. There is
//! deliberately no field for argv, policy, secrets, filesystem paths, model
//! selection, or a harness binary — a placement backend receives a reference to
//! something already admitted and frozen, and it cannot widen, redirect, or
//! re-parameterise it. Adding such a field would create the second dispatch
//! path rule 6 forbids, so the absence is the contract, not an oversight.
//!
//! A future remote implementation must additionally re-verify locally (trust,
//! lock, policy intersection against its OWN machine ceiling), resolve its own
//! `${REF}` secrets, write its own evidence, and remain its own enforcement
//! boundary. None of that is built here; it is recorded in
//! `docs/design/workflow-scaling.md` §4 Phase 6 behind an explicit trigger.

use std::collections::HashMap;
use std::path::PathBuf;

use agentstack_workflow::SpawnRequest;

use super::workflow::{ChildStep, RoleBinding};

/// What a placement backend is told about one child.
///
/// Everything here is either an identity (which run, which role) or a value
/// the drive loop already froze. Notably absent, and listed so a reviewer can
/// check the absence at a glance: argv, policy, secrets, workspace paths,
/// model or harness selection, and any ceiling the backend could reinterpret.
///
/// `step` and `role` are unread by [`LocalDispatcher`], which takes them from
/// the `SpawnRequest` it already holds. They are kept because they are how a
/// backend *addresses* a task — a remote one has no `SpawnRequest`, only this
/// descriptor plus a prompt reference — and because designing them in now is
/// the entire reason the seam exists before the network does.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct TaskDescriptor {
    /// The engine's request id — correlates the eventual `StepResult`.
    pub(crate) step: u64,
    /// The role name. A backend resolves it through the SAME manifest and
    /// profile it would have to verify anyway; it is not a capability.
    pub(crate) role: String,
    /// The child run id the drive loop already announced in a fail-closed
    /// `StepSpawned` event, so evidence exists before placement.
    pub(crate) child_run_id: String,
    /// Whether this child needs the project to itself (park/swap fallback).
    pub(crate) serial: bool,
}

/// Where a child actually runs.
///
/// Kept deliberately small: placement, and nothing else. Admission, freezing,
/// evidence, and result handling all stay in the drive loop, so a second
/// implementation cannot quietly acquire any of them.
pub(crate) trait Dispatcher: Send + Sync {
    /// Run one already-admitted child to completion and report it.
    ///
    /// Returning a [`ChildStep`] rather than a handle keeps v1 synchronous per
    /// worker, which is what the persistent pool already provides. A remote
    /// backend that needs poll/cancel semantics will widen this to a handle
    /// pair at that point — with its own review — rather than the shape being
    /// guessed now.
    fn run(&self, task: &TaskDescriptor, request: &SpawnRequest) -> ChildStep;
}

/// The reference implementation and the permanent fallback: run the child on
/// this machine through `run --locked`, exactly as the drive loop always has.
pub(crate) struct LocalDispatcher {
    pub(crate) manifest_dir: Option<PathBuf>,
    pub(crate) bindings: std::sync::Arc<HashMap<String, RoleBinding>>,
    pub(crate) pids: crate::runs::ChildPids,
}

impl Dispatcher for LocalDispatcher {
    fn run(&self, task: &TaskDescriptor, request: &SpawnRequest) -> ChildStep {
        super::workflow::run_child(
            self.manifest_dir.as_deref(),
            &self.bindings,
            request,
            &self.pids,
            task.serial,
            &task.child_run_id,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The absence of authority-bearing fields IS the contract, so it gets a
    /// witness rather than only a comment. A `TaskDescriptor` that grew an
    /// argv, a path, or a policy would be the second dispatch path rule 6
    /// forbids, and the failure mode is silent — a remote backend would simply
    /// start honouring it.
    #[test]
    fn a_task_descriptor_carries_no_authority() {
        let task = TaskDescriptor {
            step: 7,
            role: "reader".into(),
            child_run_id: "r-abc".into(),
            serial: false,
        };
        // Debug is the cheapest total view of the struct's contents; if a
        // field is ever added, it shows up here.
        let rendered = format!("{task:?}");
        for forbidden in [
            "argv", "command", "policy", "secret", "token", "path", "harness", "model", "grant",
            "ceiling",
        ] {
            assert!(
                !rendered.to_lowercase().contains(forbidden),
                "TaskDescriptor gained an authority-bearing field ({forbidden}): {rendered}"
            );
        }
        assert_eq!(task.step, 7);
    }
}
