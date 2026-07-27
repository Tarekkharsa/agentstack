//! Bounding what workflow results cost in memory (scaling plan Phase 2b):
//! the opt-in result handle and the machine-owned resident-result ceiling.
//!
//! # The problem
//!
//! Every `agent()` result becomes a live `JsString` in the Boa heap, and the
//! posture label admits the honest consequence: *"No JS heap cap exists
//! in-process."* At width 450 that is not an attack, it is ordinary use —
//! hundreds of children each returning tens of kilobytes, all resident at
//! once, plus every intermediate array the script builds from them.
//!
//! # Two mechanisms, deliberately separate
//!
//! - **`agent(prompt, { result: 'handle' })`** — opt-in, per call. The promise
//!   resolves with `{ digest, bytes, preview }` instead of the full text. A
//!   script that fans out over large outputs asks for handles and keeps its
//!   heap flat by construction.
//! - **A machine-owned resident ceiling** — `[policy.workflows]
//!   max_resident_result_bytes`. When the total of all results fed to the
//!   engine would exceed it, the RUN fails closed in-band, naming
//!   `result: 'handle'` as the remedy.
//!
//! The ceiling deliberately does **not** silently switch a result to a handle
//! at the threshold. That would change a value's *type* mid-run based on how
//! chatty earlier children happened to be — a stage expecting a string would
//! get an object, non-deterministically, and the failure would surface as a
//! confusing script error far from its cause. Exceeding a machine ceiling is
//! a run-level condition, so it is reported like one (the `wall_deadline`
//! shape), not papered over per step.
//!
//! # Why there is no second content store
//!
//! The plan called for `~/.agentstack/artifacts/<sha256>`. Building it now
//! would write a second copy of bytes that are **already** persisted and
//! already digest-recorded: `runs/<child>/stdout` is written by the locked
//! child path as the exact input to the recorded `HeadlessOutput.sha256`, and
//! Stage F resume already verifies results against it. A handle therefore
//! carries the digest of that existing artifact rather than duplicating it.
//!
//! Handles do not outlive their workflow run (the script holds them; the run
//! ends; they are gone), so no cross-session addressing is needed. A real
//! machine-local CAS earns its keep when Phase 6 needs a cross-machine
//! transfer unit — with evidence, at that point, rather than a duplicate
//! write per child today.

use serde_json::{json, Value};

/// Bytes of each result kept inline in a handle. Enough to recognise and log
/// what came back, far too little to matter for the heap.
///
/// The consequence worth stating plainly: a handle costs roughly
/// `preview + digest + keys` ≈ 620 bytes, so **for results smaller than that a
/// handle is LARGER than the text it replaces**. Handles are for the wide
/// stages returning kilobytes each, where the saving is one to two orders of
/// magnitude; asking for them on short results is harmless but pointless.
const HANDLE_PREVIEW_BYTES: usize = 512;

/// Default resident ceiling when `[policy.workflows]
/// max_resident_result_bytes` is absent. 8 MiB is generous for real
/// orchestration (hundreds of ordinary text results) while still bounding the
/// growth the posture label calls out.
pub(crate) const DEFAULT_MAX_RESIDENT_RESULT_BYTES: u64 = 8 * 1024 * 1024;

/// Whether this call asked for its result as a handle.
///
/// Strict: only the exact string `"handle"`. A typo must not silently produce
/// full text the author believed was going to be a handle.
pub(crate) fn wants_handle(opts: &Value) -> bool {
    opts.get("result").and_then(|v| v.as_str()) == Some("handle")
}

/// `{ digest, bytes, preview }` for one child's raw stdout.
///
/// `digest` is the sha256 of the same bytes the locked child persisted as
/// `runs/<child>/stdout` and recorded as `HeadlessOutput.sha256`, so it is
/// equal to the recorded identity by construction rather than by a second
/// bookkeeping path.
///
/// Not frozen, and deliberately so: the CLI never reads a handle back (there
/// is no `read()` builtin in v1), so a script mutating its own copy affects
/// nothing. Claiming immutability we do not enforce would be exactly the kind
/// of unearned assurance the enforcement docs exist to avoid.
pub(crate) fn handle_value(raw: &str) -> Value {
    json!({
        "digest": format!("sha256:{}", agentstack_core::digest::sha256_hex(raw.as_bytes())),
        "bytes": raw.len(),
        "preview": truncate_on_char_boundary(raw, HANDLE_PREVIEW_BYTES),
    })
}

fn truncate_on_char_boundary(s: &str, cap: usize) -> &str {
    if s.len() <= cap {
        return s;
    }
    let mut end = cap;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// The running total of result bytes handed to the engine, against the
/// machine-owned ceiling.
///
/// Overflow is **sticky and set at the charge site**, the same pattern the
/// engine uses for `exhausted` and `compile_denied`: the drive loop polls it
/// after collecting, so a breach can never be lost between steps, and no
/// script-controlled string is involved in deciding it.
pub(crate) struct ResidentBudget {
    cap: u64,
    used: u64,
    overflowed: bool,
}

impl ResidentBudget {
    pub(crate) fn new(cap: u64) -> Self {
        Self {
            cap,
            used: 0,
            overflowed: false,
        }
    }

    pub(crate) fn cap(&self) -> u64 {
        self.cap
    }

    pub(crate) fn used(&self) -> u64 {
        self.used
    }

    /// Charge one result. Measured on the JSON serialization, which is what
    /// actually crosses into the interpreter — a structured result costs what
    /// its rendered form costs, not what its `as_str()` would have been.
    ///
    /// Saturating: the total is evidence, not arithmetic to trust with
    /// overflow. Charging continues after a breach so the reported figure is
    /// the real total rather than the first offending prefix.
    pub(crate) fn charge(&mut self, value: &Value) {
        let bytes = match value {
            // A plain string result crosses as its own bytes; serializing it
            // would add quotes and escapes that are not really resident.
            Value::String(s) => s.len() as u64,
            other => serde_json::to_string(other).map(|s| s.len()).unwrap_or(0) as u64,
        };
        self.used = self.used.saturating_add(bytes);
        if self.used > self.cap {
            self.overflowed = true;
        }
    }

    pub(crate) fn overflowed(&self) -> bool {
        self.overflowed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_exact_opt_in_string_requests_a_handle() {
        assert!(wants_handle(&json!({ "result": "handle" })));
        // A typo must not silently yield full text the author expected to be
        // a handle — and nothing else means "handle" either.
        for miss in [
            json!({ "result": "handles" }),
            json!({ "result": "Handle" }),
            json!({ "result": true }),
            json!({ "role": "r" }),
        ] {
            assert!(!wants_handle(&miss), "{miss:?}");
        }
    }

    #[test]
    fn a_handle_carries_the_digest_of_the_persisted_artifact() {
        let raw = "hello world";
        let handle = handle_value(raw);
        assert_eq!(
            handle["digest"],
            format!(
                "sha256:{}",
                agentstack_core::digest::sha256_hex(raw.as_bytes())
            ),
            "the handle digest must equal the recorded HeadlessOutput identity"
        );
        assert_eq!(handle["bytes"], 11);
        assert_eq!(handle["preview"], "hello world");
    }

    #[test]
    fn a_handle_stays_small_however_large_the_result() {
        let raw = "x".repeat(4 * 1024 * 1024);
        let handle = handle_value(&raw);
        assert_eq!(handle["bytes"], 4 * 1024 * 1024);
        assert_eq!(
            handle["preview"].as_str().unwrap().len(),
            HANDLE_PREVIEW_BYTES,
            "the preview is what keeps the heap flat"
        );
        // The whole point: a handle costs kilobytes, not megabytes.
        let rendered = serde_json::to_string(&handle).unwrap();
        assert!(
            rendered.len() < 2 * 1024,
            "handle rendered {}",
            rendered.len()
        );
    }

    #[test]
    fn multibyte_previews_cut_on_a_char_boundary() {
        // A naive byte slice here panics; the cut must land cleanly.
        let raw = "é".repeat(HANDLE_PREVIEW_BYTES);
        let handle = handle_value(&raw);
        let preview = handle["preview"].as_str().unwrap();
        assert!(preview.len() <= HANDLE_PREVIEW_BYTES);
        assert!(raw.starts_with(preview));
    }

    #[test]
    fn the_budget_trips_only_past_the_cap_and_stays_tripped() {
        let mut budget = ResidentBudget::new(100);
        budget.charge(&json!("x".repeat(60)));
        assert!(!budget.overflowed(), "under the cap");
        budget.charge(&json!("x".repeat(40)));
        assert!(!budget.overflowed(), "exactly at the cap is not over it");
        budget.charge(&json!("x"));
        assert!(budget.overflowed());
        assert_eq!(budget.used(), 101);

        // Sticky, and the total keeps accumulating so the reported figure is
        // the real one rather than the first offending prefix.
        budget.charge(&json!("x".repeat(10)));
        assert!(budget.overflowed());
        assert_eq!(budget.used(), 111);
    }

    #[test]
    fn a_structured_result_is_charged_what_it_renders_to() {
        // Phase 2a results are objects; charging `as_str()` (nothing) would
        // have made the ceiling blind to exactly the results it must bound.
        let mut budget = ResidentBudget::new(1_000_000);
        let value = json!({ "findings": ["a", "b"], "n": 2 });
        budget.charge(&value);
        assert_eq!(
            budget.used(),
            serde_json::to_string(&value).unwrap().len() as u64
        );
        assert!(budget.used() > 0);
    }
}
