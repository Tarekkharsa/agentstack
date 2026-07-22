//! Stage F resume — the journal model over a workflow run's recorded events
//! (design doc §6 step 3: the event stream **is** the resume journal; §12.4
//! Stage F: replay feeds completed step results into the drive loop for a
//! byte-identical script + args, and any divergence refuses).
//!
//! Everything here treats the journal as HOSTILE input (rule 7): once a human
//! can edit `events.jsonl`, a "recorded" step is a claim, not a fact. The two
//! verification anchors that keep replay honest without ever persisting
//! result or prompt text:
//!
//! - each engine-re-issued request must match the journaled `StepSpawned` on
//!   id, role, and `request_digest` (digest-vs-digest by construction —
//!   redaction keeps prompt text out of events and nothing else persists it);
//! - each replayed result is read from the child's run-dir `stdout` artifact
//!   and verified against the child's recorded `HeadlessOutput.sha256` on the
//!   RAW bytes, before the same `from_utf8_lossy` transform the live path
//!   applied — so replay equals live even for invalid UTF-8, and an edited
//!   artifact refuses per step, fail closed.
//!
//! A refused resume must leave the journal byte-untouched (so a corrected
//! re-attempt still reads the same journal): nothing in this module appends
//! an event, and the drive loop appends its `WorkflowResumed` marker only
//! after the whole journaled prefix has replayed cleanly.

use std::collections::HashMap;

use anyhow::{Context as AnyhowContext, Result};

use agentstack_workflow::SpawnRequest;

use crate::calllog::{RunEvent, RunLog};

/// Bound on the journal file before parse (rule 7: the journal is hostile
/// input; a doctored multi-gigabyte log must refuse, not exhaust memory).
const MAX_JOURNAL_BYTES: u64 = 64 * 1024 * 1024;

/// A journaled step's recorded terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum JournaledTerminal {
    Completed,
    Failed,
}

/// One journaled step, keyed by the engine's step id.
#[derive(Debug, Clone)]
struct JournaledStep {
    role: String,
    request_digest: String,
    child_run_id: String,
    /// The recorded terminal and its position in the journal. The position is
    /// the FEED ORDER: the live drive pushed results in the same iteration
    /// that appended these events, so journal terminal order IS the
    /// settlement order the script would have observed (Stage F task 4).
    terminal: Option<(JournaledTerminal, usize)>,
}

/// One consumed (aligned) journal entry handed back to the drive loop.
#[derive(Debug, Clone)]
pub(crate) struct TakenStep {
    pub step: u64,
    pub child_run_id: String,
    pub terminal: Option<(JournaledTerminal, usize)>,
}

/// The identity the original session recorded — Stage F compares against it,
/// never re-defines it (`wf_grant_digest` / `wf_args_digest` are the shipped
/// canonical forms).
#[derive(Debug, Clone)]
pub(crate) struct JournalIdentity {
    pub workflow: String,
    pub workflow_digest: String,
    pub grant_digest: String,
    pub args_digest: String,
    pub max_agents: u32,
    pub max_wall_seconds: u64,
}

/// The parsed, resumability-checked journal of one interrupted workflow run.
pub(crate) struct ReplayJournal {
    pub identity: JournalIdentity,
    steps: HashMap<u64, JournaledStep>,
}

impl ReplayJournal {
    /// Load and gate a run's journal: bounded read, the resumability rule
    /// over ALL recorded terminal outcomes, and the last-wins step index.
    ///
    /// Last-wins per step id is load-bearing for composition: a previous
    /// resume that re-executed a step legitimately left two `StepSpawned`
    /// events for one id (the later, post-marker one supersedes), and a
    /// later `StepSpawned` resets the step's terminal — only terminals
    /// recorded AFTER a step's latest spawn belong to that execution.
    pub(crate) fn load(run_id: &str) -> Result<ReplayJournal> {
        anyhow::ensure!(
            safe_run_segment(run_id),
            "refusing resume: '{run_id}' is not a valid run id"
        );
        let path = crate::util::paths::agentstack_home()
            .join("runs")
            .join(run_id)
            .join("events.jsonl");
        let size = std::fs::metadata(&path)
            .map(|m| m.len())
            .with_context(|| format!("no recorded events for run '{run_id}'"))?;
        anyhow::ensure!(
            size <= MAX_JOURNAL_BYTES,
            "refusing resume: the journal for run '{run_id}' is {size} bytes (bound \
             {MAX_JOURNAL_BYTES}) — a journal this size is not a genuine workflow log (rule 7)"
        );

        let events = RunLog::read(run_id);
        anyhow::ensure!(
            !events.is_empty(),
            "no recorded events for run '{run_id}' — workflow run ids (w-…) are printed on the \
             run's admission banner"
        );

        let identity = events
            .iter()
            .find_map(|e| match e {
                RunEvent::WorkflowStarted {
                    workflow,
                    workflow_digest,
                    grant_digest,
                    args_digest,
                    max_agents,
                    max_wall_seconds,
                    ..
                } => Some(JournalIdentity {
                    workflow: workflow.clone(),
                    workflow_digest: workflow_digest.clone(),
                    grant_digest: grant_digest.clone(),
                    args_digest: args_digest.clone(),
                    max_agents: *max_agents,
                    max_wall_seconds: *max_wall_seconds,
                }),
                _ => None,
            })
            .with_context(|| {
                format!(
                    "run '{run_id}' is not a workflow run (no workflow_started event) — only \
                     workflow runs (w-…) are resumable"
                )
            })?;

        // The resumability rule (D2), decided against ALL recorded terminal
        // outcomes — a log can genuinely carry more than one (the Stage E
        // done/watchdog_kill boundary race, multi-session logs after a
        // resume), and `done` anywhere wins.
        let outcomes: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                RunEvent::WorkflowCompleted { outcome, .. } => Some(outcome.as_str()),
                _ => None,
            })
            .collect();
        resumability(run_id, &outcomes)?;

        // Last-wins step index (composition; see the method doc).
        let mut steps: HashMap<u64, JournaledStep> = HashMap::new();
        for (idx, event) in events.iter().enumerate() {
            match event {
                RunEvent::StepSpawned {
                    step,
                    role,
                    child_run_id,
                    request_digest,
                    ..
                } => {
                    steps.insert(
                        *step,
                        JournaledStep {
                            role: role.clone(),
                            request_digest: request_digest.clone(),
                            child_run_id: child_run_id.clone(),
                            terminal: None,
                        },
                    );
                }
                RunEvent::StepCompleted { step, .. } => {
                    if let Some(entry) = steps.get_mut(step) {
                        entry.terminal = Some((JournaledTerminal::Completed, idx));
                    }
                    // A terminal with no prior spawn is never a genuine shape
                    // (spawns are appended fail-closed before execution); it
                    // is ignored here and the batch shape check refuses the
                    // gap it leaves.
                }
                RunEvent::StepFailed { step, .. } => {
                    if let Some(entry) = steps.get_mut(step) {
                        entry.terminal = Some((JournaledTerminal::Failed, idx));
                    }
                }
                _ => {}
            }
        }

        Ok(ReplayJournal { identity, steps })
    }

    /// The identity gate (D4): every dimension compared against the CURRENT
    /// admitted values, each refusal naming its dimension. The digests are
    /// the shipped canonical forms, computed by the caller with the same
    /// functions the original session used.
    pub(crate) fn verify_identity(
        &self,
        name: &str,
        script_digest: &str,
        grant_digest: &str,
        args_digest: &str,
        effective_agents: u32,
        effective_wall: u64,
    ) -> Result<()> {
        anyhow::ensure!(
            self.identity.workflow == name,
            "refusing resume: workflow NAME diverged — the journal records '{}', this \
             invocation names '{name}'",
            self.identity.workflow
        );
        anyhow::ensure!(
            self.identity.workflow_digest == script_digest,
            "refusing resume: SCRIPT identity diverged — the pinned content digest no longer \
             matches the journaled workflow_digest (byte-identical script is the resume \
             precondition; replaying results into different code is not recovery)"
        );
        if self.identity.grant_digest != grant_digest {
            // The digest is authoritative; the recorded ceiling fields give
            // the human a hint at WHICH grant dimension moved.
            let hint = if self.identity.max_agents != effective_agents
                || self.identity.max_wall_seconds != effective_wall
            {
                format!(
                    "the effective ceilings moved (journaled max_agents={} \
                     max_wall_seconds={}, current {}/{})",
                    self.identity.max_agents,
                    self.identity.max_wall_seconds,
                    effective_agents,
                    effective_wall
                )
            } else {
                "the ceilings match, so the admitted ROLE SET moved".to_string()
            };
            anyhow::bail!(
                "refusing resume: effective GRANT diverged (ceilings or roles) — {hint}; \
                 machine policy, the manifest, or the declared roles changed since the \
                 original run, and resume never re-runs under a different grant"
            );
        }
        anyhow::ensure!(
            self.identity.args_digest == args_digest,
            "refusing resume: ARGS identity diverged — --args-json must be byte-identical to \
             the original invocation (passing no args is distinct from any args)"
        );
        Ok(())
    }

    /// Consume the journal entry for one engine-re-issued request, verifying
    /// per-step alignment (id, role, request digest). `Ok(None)` means the
    /// request is past the journal (unjournaled); a present-but-mismatched
    /// entry refuses — feeding results into a misaligned request is
    /// corruption, not recovery, and this is the honest detector for
    /// engine-nondeterminism bugs.
    pub(crate) fn take(
        &mut self,
        request: &SpawnRequest,
        current_request_digest: &str,
    ) -> Result<Option<TakenStep>> {
        let Some(entry) = self.steps.get(&request.id) else {
            return Ok(None);
        };
        anyhow::ensure!(
            entry.role == request.role && entry.request_digest == current_request_digest,
            "refusing resume: step #{} diverged from the journal (role or request digest \
             mismatch) — the engine's deterministic replay re-issued a different request than \
             the one recorded; the journal does not describe this script's execution",
            request.id
        );
        let entry = self.steps.remove(&request.id).expect("entry just found");
        Ok(Some(TakenStep {
            step: request.id,
            child_run_id: entry.child_run_id,
            terminal: entry.terminal,
        }))
    }

    /// Journaled steps not yet consumed by an aligned request.
    pub(crate) fn remaining(&self) -> usize {
        self.steps.len()
    }

    /// The refusal for journaled steps the engine never re-issued (leftover
    /// entries at a fully-live batch or at the run's end) — same misalignment
    /// class as `take`'s per-step check, from the other direction.
    pub(crate) fn refuse_leftover(&self, at: &str) -> anyhow::Error {
        let mut ids: Vec<u64> = self.steps.keys().copied().collect();
        ids.sort_unstable();
        let ids: Vec<String> = ids.iter().map(|i| format!("#{i}")).collect();
        anyhow::anyhow!(
            "refusing resume: {} journaled step(s) ({}) were never re-issued by the engine {at} \
             — the journal does not match this script's deterministic replay (a torn or \
             doctored journal, or an engine-nondeterminism bug)",
            self.steps.len(),
            ids.join(", ")
        )
    }
}

/// D2 — the resumability rule against the shipped terminal-outcome
/// vocabulary. `done` anywhere wins (the multi-terminal read); a recorded
/// deterministic failure or engine defect refuses; only the two
/// interruption outcomes (and the no-terminal case) resume. Unknown
/// vocabulary refuses — fail closed on a journal this code does not
/// understand.
fn resumability(run_id: &str, outcomes: &[&str]) -> Result<()> {
    if outcomes.contains(&"done") {
        anyhow::bail!(
            "refusing resume: run '{run_id}' completed (outcome 'done' is recorded) — there is \
             nothing to resume"
        );
    }
    if let Some(failed) = outcomes.iter().find(|o| o.starts_with("failed:")) {
        anyhow::bail!(
            "refusing resume: run '{run_id}' recorded '{failed}' — a deterministic failure; \
             replaying the journal would reproduce it identically (§3.1: replay never \
             recomputes). Fix the cause and re-run fresh"
        );
    }
    if outcomes.contains(&"engine_invariant_breach") {
        anyhow::bail!(
            "refusing resume: run '{run_id}' recorded 'engine_invariant_breach' — an engine \
             defect, not a recoverable interruption; please report it"
        );
    }
    if let Some(unknown) = outcomes
        .iter()
        .find(|o| **o != "wall_deadline" && **o != "watchdog_kill")
    {
        anyhow::bail!(
            "refusing resume: run '{run_id}' recorded the unrecognized outcome '{unknown}' — \
             failing closed on journal vocabulary this build does not understand"
        );
    }
    // Only wall_deadline / watchdog_kill terminals (or none at all): the run
    // was interrupted, not concluded — resumable with a fresh wall clock.
    Ok(())
}

/// Read one replayed child's result: the run-dir `stdout` artifact, verified
/// against the child's recorded `HeadlessOutput.sha256` on the RAW bytes
/// BEFORE any transform, then the same single transform the live path
/// applied (`from_utf8_lossy`). A truncated capture replays as the same
/// truncated string the live run saw — the digest covers the captured
/// bytes, identically and honestly.
pub(crate) fn read_verified_result(step: u64, child_run_id: &str) -> Result<String> {
    anyhow::ensure!(
        safe_run_segment(child_run_id),
        "refusing resume: step #{step} names an invalid child run id in the journal"
    );
    let recorded = RunLog::read(child_run_id)
        .into_iter()
        .rev()
        .find_map(|e| match e {
            RunEvent::HeadlessOutput { sha256, .. } => Some(sha256),
            _ => None,
        })
        .with_context(|| {
            format!(
                "refusing resume: step #{step}'s child run {child_run_id} recorded no output \
                 evidence (HeadlessOutput) — its result cannot be verified, so it cannot be \
                 replayed"
            )
        })?;

    let path = crate::util::paths::agentstack_home()
        .join("runs")
        .join(child_run_id)
        .join(super::locked::CHILD_STDOUT_FILE);
    let size = std::fs::metadata(&path).map(|m| m.len()).with_context(|| {
        format!(
            "refusing resume: step #{step}'s result artifact is missing \
             (runs/{child_run_id}/{}) — the recorded result cannot be re-fed",
            super::locked::CHILD_STDOUT_FILE
        )
    })?;
    anyhow::ensure!(
        size <= crate::runs::MAX_PROMPT_OUTPUT_BYTES as u64,
        "refusing resume: step #{step}'s result artifact exceeds the capture cap — a genuine \
         capture never does, so the artifact was tampered with"
    );
    let bytes =
        std::fs::read(&path).with_context(|| format!("reading step #{step}'s result artifact"))?;
    let digest = agentstack_core::digest::sha256_hex(&bytes);
    anyhow::ensure!(
        digest == recorded,
        "refusing resume: step #{step}'s result artifact does not match the recorded output \
         digest (child run {child_run_id}) — an edited artifact, the wrong file, or \
         truncation drift; the replay feed is tamper-evident and fails closed"
    );
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// The recorder's run-id segment rule, mirrored (the recorder keeps its
/// predicate private; this defensive copy exists because journal-supplied
/// ids are hostile input used to build paths here).
fn safe_run_segment(run_id: &str) -> bool {
    !run_id.is_empty()
        && run_id.len() <= 128
        && run_id != "."
        && run_id != ".."
        && run_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D2 in one table: every shipped terminal outcome (and the boundary-race
    /// multi-terminal combinations) maps to the decided resumability.
    #[test]
    fn resumability_rule_matches_the_decided_table() {
        let ok = |outs: &[&str]| resumability("w-t", outs).is_ok();
        assert!(ok(&[]), "no terminal (crash / kill -9 / torn tail)");
        assert!(ok(&["wall_deadline"]));
        assert!(ok(&["watchdog_kill"]));
        assert!(ok(&["wall_deadline", "watchdog_kill"]));

        assert!(!ok(&["done"]));
        assert!(!ok(&["done", "watchdog_kill"]), "done anywhere wins");
        assert!(!ok(&["watchdog_kill", "done"]), "order-independent");
        assert!(!ok(&["failed:runtime_error"]), "deterministic failure");
        assert!(!ok(&["failed:agents_exhausted"]));
        assert!(!ok(&["engine_invariant_breach"]));
        assert!(!ok(&["something_new"]), "unknown vocabulary fails closed");
    }

    #[test]
    fn hostile_run_segments_are_refused() {
        for bad in ["", ".", "..", "../evil", "a/b", "x\0y"] {
            assert!(!safe_run_segment(bad), "must reject {bad:?}");
        }
        assert!(safe_run_segment("w-0123abcd"));
    }
}
