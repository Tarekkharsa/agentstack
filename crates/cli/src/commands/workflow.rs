//! `agentstack workflow run <name>` — the Stage C composition (design doc
//! §12.4): workflow-level admission, then the drive loop over the
//! `agentstack-workflow` engine, each `SpawnRequest` becoming a locked child
//! run through the existing `run --locked` seams with per-child MCP config
//! injection.
//!
//! Order is security-relevant and fixed:
//!
//! 1. **Admission before any parse.** The W1 choke point
//!    ([`crate::workflows::normalized_workflows`]: trust gate FIRST, static
//!    validation, strict lock verification, machine-capped ceilings) admits
//!    the whole declared set; then the named workflow's pinned bytes are read
//!    under a digest sandwich. The script text never reaches the engine — not
//!    even its parser — before admission passes.
//! 2. **Roles are resolved to profiles against the MANIFEST.** The manifest
//!    roles are the authority; the script's `meta.roles` is the
//!    script-internal consistency set (R2), cross-checked ⊆ the manifest set
//!    after engine construction and refused otherwise.
//! 3. **The engine is constructed inside `catch_unwind` at this CLI edge** —
//!    the crate contains its own parse panics (AL5), but the CLI must not
//!    rely on that: belt and suspenders in both directions.
//! 4. **The out-of-thread watchdog is armed before the first `step()`.**
//!    Stage C is where scripts first execute, so the liveness backstop exists
//!    from day one: a SEPARATE thread force-exits the PROCESS on wall-clock
//!    overrun (a cooperative check cannot fire on a drive thread stuck inside
//!    Boa). Stage E: the dying watchdog appends its terminal outcome
//!    best-effort before the exit — the one exception to fail-closed
//!    recording.
//! 5. **The workflow log is the join table (Stage E).** Every spawn is
//!    recorded fail-closed BEFORE its child launches; child grant digests,
//!    postures, and outcomes live in each child's own log and are JOINED by
//!    `workflow report`, never duplicated into workflow events.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context as AnyhowContext, Result};
use owo_colors::OwoColorize;

use agentstack_workflow::{
    extract_meta, Grant, Progress, RuntimeLimits, SpawnRequest, StepOutcome, StepOutput,
    StepResult, WorkflowErrorKind, WorkflowRun,
};

use crate::calllog::{RunEvent, RunLog};
use crate::cli::{RunArgs, WorkflowReportArgs, WorkflowRunArgs};
use crate::commands::locked::{run_locked_child, supports_injection, ts};
use crate::commands::workflow_replay::{read_verified_result, JournaledTerminal, ReplayJournal};
use crate::text::sanitize_line;
use crate::workflows::NormalizedWorkflow;

/// Default cap on concurrently running children when the machine's
/// `[policy.workflows] max_concurrent` is absent. Conservative and
/// single-digit by design: children are full harness CLI processes (each ~1
/// CPU during startup plus a model call), the canonical map fan-out (§3.1)
/// and the acceptance fixture are 3-wide, and 4 covers that shape with one
/// slot of headroom while bounding host load and API-rate pressure.
/// Engine-owned: never script-negotiated (Stage D negotiates the OTHER
/// ceilings, not this one).
const DEFAULT_MAX_CONCURRENT: u32 = 4;

/// Bound on `--args-json` before it is parsed (rule 7: invoker args are
/// untrusted input). Depth is bounded twice behind this: serde_json's own
/// recursion limit at parse, and the engine's `MAX_JSON_DEPTH` boundary at
/// install.
const MAX_ARGS_JSON_BYTES: usize = 256 * 1024;

/// Bound on the pinned script read (rule 7 — the bytes are trusted-by-digest,
/// but the read is still bounded like every other file ingestion).
const MAX_SCRIPT_BYTES: u64 = 1024 * 1024;

/// The watchdog's process exit code on wall-clock overrun — the `timeout(1)`
/// convention, so CI wrappers recognize it.
const WATCHDOG_EXIT_CODE: i32 = 124;

/// Grace the watchdog is armed ABOVE the effective wall ceiling: room for an
/// in-flight batch to reach the next cooperative checkpoint (the CLI is
/// blocked joining the batch while children run, so the clean in-band refusal
/// can only fire at batch boundaries); a batch still running past the grace
/// fail-closes via the watchdog at exit 124. Honest note: the fixed grace is
/// proportionally huge for a tiny ceiling (a 1s `meta.maxWallSeconds` → 31s
/// hard kill) — acceptable, since a genuinely stalled run isn't escalating,
/// just burning CPU, and both paths fail closed.
const WATCHDOG_GRACE_SECS: u64 = 30;

/// D3 (Stage E): the script-authored `label` is byte-bounded at append time
/// (char-boundary truncation, visible ellipsis) and stored as data in the
/// JSON event; `sanitize_line` runs at REPORT render — the same
/// bound-at-source / sanitize-at-terminal split Stage C established for
/// progress lines.
const MAX_LABEL_BYTES: usize = 120;

/// Rule-7 bounds on the taint detector (D2, §11 ruling 3). Results shorter
/// than the floor never mark (trivial strings like "ok" would mark
/// everything); the needle and the scanned prompt are capped so hostile
/// sizes cannot blow up the scan (`str::contains` is linear two-way search);
/// and the RETAINED source set is bounded twice — each source keeps only its
/// needle prefix (detection never uses more), and at most
/// [`TAINT_MAX_SOURCES`] sources are kept, so the evidence bookkeeping can
/// never grow past `TAINT_MAX_SOURCES × TAINT_NEEDLE_BYTES` (8 MiB) or make
/// a large-but-legitimate run's memory/CPU depend on unbounded child output
/// — Stage E stays evidence-only, never a new liveness hazard.
/// FALSE NEGATIVES are accepted and stated: transformed, mid-sliced,
/// sub-floor, or beyond-the-source-cap embeddings go unmarked — this is a
/// reviewability aid, not DLP.
const TAINT_MIN_BYTES: usize = 64;
const TAINT_NEEDLE_BYTES: usize = 64 * 1024;
const TAINT_SCAN_BYTES: usize = 256 * 1024;
const TAINT_MAX_SOURCES: usize = 128;

/// The workflow-level evidence channel (Stage E) — the same material-append
/// discipline as the locked run's `Evidence`: a failure to record is itself
/// a reason not to proceed ("nothing trusted runs unobserved"). The ONE
/// exception is the watchdog's already-dying path, which appends
/// best-effort and exits 124 regardless (see `spawn_watchdog`).
struct WorkflowEvidence {
    log: RunLog,
    run_id: String,
    started: Instant,
}

impl WorkflowEvidence {
    fn material(&self, ev: &RunEvent) -> Result<()> {
        self.log.append_checked(ev).with_context(|| {
            format!(
                "refusing to proceed: workflow evidence for run {} could not be recorded",
                self.run_id
            )
        })
    }

    /// Append the terminal `WorkflowCompleted` (checked). Every drive exit
    /// path lands here except the watchdog's best-effort one.
    fn terminal(&self, outcome: &str, exhausted: bool) -> Result<()> {
        self.material(&RunEvent::WorkflowCompleted {
            ts: ts(),
            outcome: outcome.to_string(),
            exhausted,
            duration_ms: self.started.elapsed().as_millis() as u64,
        })
    }

    /// Stage F: the resume marker — appended AFTER the whole journaled
    /// prefix replayed cleanly and BEFORE the resumed session's first live
    /// event, checked (recording failure on resume fails closed before any
    /// live spawn — the Stage E gate). Everything after the LAST marker in
    /// a log is the newest session's live tail.
    fn resumed(&self, replayed_steps: u64) -> Result<()> {
        self.material(&RunEvent::WorkflowResumed {
            ts: ts(),
            replayed_steps,
        })
    }

    /// Record the terminal outcome for a run failing with `why`. If the
    /// recording ALSO fails, surface both — the run's failure stays primary.
    fn fail(&self, outcome: &str, exhausted: bool, why: anyhow::Error) -> anyhow::Error {
        match self.terminal(outcome, exhausted) {
            Ok(()) => why,
            Err(rec) => why.context(format!(
                "ALSO: this outcome's evidence could not be recorded ({rec:#})"
            )),
        }
    }
}

/// Stable recorded-outcome slug for each engine error kind — an exhaustive
/// match, so a future kind cannot ship without naming its outcome string.
fn kind_slug(kind: WorkflowErrorKind) -> &'static str {
    use WorkflowErrorKind as K;
    match kind {
        K::InvalidScript => "invalid_script",
        K::MetaViolation => "meta_violation",
        K::UndeclaredRole => "undeclared_role",
        K::RoleNotAdmitted => "role_not_admitted",
        K::IterationLimit => "iteration_limit",
        K::CompileDenied => "compile_denied",
        K::Panicked => "panicked",
        K::RuntimeError => "runtime_error",
        K::Internal => "internal",
        K::AgentsExhausted => "agents_exhausted",
    }
}

/// D4: deterministic digest of the EFFECTIVE grant (Stage F reuses this
/// definition verbatim — never a second one). Length-framed throughout:
/// role names are TOML table keys with no charset guarantee (rule 7), so
/// the roles segment is count + per-role length framing, never a joined
/// string two different sets could collide into.
fn wf_grant_digest(max_agents: u32, max_wall_seconds: u64, roles: &[String]) -> String {
    let mut pre = format!(
        "wfgrant-v1\0max_agents={max_agents}\0max_wall_seconds={max_wall_seconds}\0roles={}",
        roles.len()
    );
    for role in roles {
        pre.push('\0');
        pre.push_str(&role.len().to_string());
        pre.push('\0');
        pre.push_str(role);
    }
    agentstack_core::digest::sha256_hex(pre.as_bytes())
}

/// Identity of the RAW `--args-json` bytes — byte-identical is the Stage F
/// resume precondition, so no re-serialization. The no-args case is pinned
/// distinctly; the `some\0` prefix makes collision impossible by framing.
fn wf_args_digest(args_json: Option<&str>) -> String {
    let pre: Vec<u8> = match args_json {
        Some(text) => {
            let mut v = b"wfargs-v1\0some\0".to_vec();
            v.extend_from_slice(text.as_bytes());
            v
        }
        None => b"wfargs-v1\0none".to_vec(),
    };
    agentstack_core::digest::sha256_hex(&pre)
}

/// Per-step replay-alignment identity (Stage F consumes it): length-framed
/// canonical prompt + opts. Opts must be in the identity — they ride the
/// `SpawnRequest` into the child, so an opts change under an identical
/// prompt is a different request. The prompt TEXT never enters an event;
/// only this digest does.
fn wf_request_digest(request: &SpawnRequest) -> String {
    let opts = serde_json::to_string(&request.opts).unwrap_or_else(|_| "null".to_string());
    let mut pre = format!("wfreq-v1\0{}\0", request.prompt.len());
    pre.push_str(&request.prompt);
    pre.push_str(&format!("{}\0", opts.len()));
    pre.push_str(&opts);
    agentstack_core::digest::sha256_hex(pre.as_bytes())
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

/// Prior step ids whose completed RESULT text appears in `prompt` — D2's
/// bounded detector (see the TAINT_* constants for the bounds and the
/// stated false negatives).
fn taint_marks(prompt: &str, prior_results: &[(u64, String)]) -> Vec<u64> {
    let hay = truncate_on_char_boundary(prompt, TAINT_SCAN_BYTES);
    prior_results
        .iter()
        .filter(|(_, result)| result.len() >= TAINT_MIN_BYTES)
        .filter(|(_, result)| hay.contains(truncate_on_char_boundary(result, TAINT_NEEDLE_BYTES)))
        .map(|(id, _)| *id)
        .collect()
}

/// Append the pre-spawn `StepSpawned` for one request (fail-closed, gate 2)
/// and return the pre-generated child run id the child will run under. The
/// `serial`/`codex_residual` flags are taken from the role's binding —
/// recorded evidence, not stderr-only (Stage E task 4).
fn record_step_spawned(
    wev: &WorkflowEvidence,
    request: &SpawnRequest,
    binding: Option<&RoleBinding>,
    prior_results: &[(u64, String)],
) -> Result<String> {
    let child_run_id = crate::runs::gen_id();
    wev.material(&RunEvent::StepSpawned {
        ts: ts(),
        step: request.id,
        role: request.role.clone(),
        child_run_id: child_run_id.clone(),
        request_digest: wf_request_digest(request),
        label: bound_label(&request.opts),
        taint: taint_marks(&request.prompt, prior_results),
        serial: binding.map(|b| !b.injectable).unwrap_or(false),
        codex_residual: binding.map(|b| b.codex_residual).unwrap_or(false),
    })?;
    Ok(child_run_id)
}

/// The bounded script-authored label for the event (D3), if any.
fn bound_label(opts: &serde_json::Value) -> Option<String> {
    let label = opts.get("label")?.as_str()?;
    if label.len() <= MAX_LABEL_BYTES {
        return Some(label.to_string());
    }
    Some(format!(
        "{}…",
        truncate_on_char_boundary(label, MAX_LABEL_BYTES)
    ))
}

/// One admitted role: the profile name it fences to, the harness the
/// profile binds (default claude-code), and how its children schedule.
struct RoleBinding {
    harness: String,
    /// Injection-capable children fan out concurrently; the rest fall back
    /// to park/swap and run strictly serially, labeled (§12.1).
    injectable: bool,
    /// codex carries the §12.1 connector residual — surfaced PER CHILD in
    /// the run output (a workflow multiplies the exposure N times).
    codex_residual: bool,
}

pub fn run(manifest_dir: Option<&Path>, args: &WorkflowRunArgs) -> Result<()> {
    let final_value = run_value(manifest_dir, args)?;
    // Stdout is the deliverable: the workflow's final value as JSON, nothing
    // else (every banner and progress line goes to stderr).
    println!("{}", serde_json::to_string_pretty(&final_value)?);
    Ok(())
}

/// The full admission + drive composition, returning the final value (the
/// testable seam; `run` adds only the stdout print).
fn run_value(manifest_dir: Option<&Path>, args: &WorkflowRunArgs) -> Result<serde_json::Value> {
    let ctx = super::load(manifest_dir)?;
    let base = crate::manifest::project_root_of(&ctx.dir);
    let machine_policy = crate::machine_policy::load()?;
    let lock = crate::lock::Lock::load(&ctx.dir)?;
    let store = crate::store::Store::default_store();

    // 1. Admission before any parse — the W1 choke point (trust gate FIRST,
    // static validation, strict lock verify, ceiling intersection). Any
    // failure refuses before a single script byte is interpreted.
    let admitted = crate::workflows::normalized_workflows(
        &base,
        &ctx.loaded.manifest,
        &ctx.dir,
        &store,
        &lock,
        &machine_policy.workflows,
    )?;
    let wf = admitted
        .iter()
        .find(|w| w.name == args.name)
        .with_context(|| {
            let names: Vec<&str> = admitted.iter().map(|w| w.name.as_str()).collect();
            format!(
                "no workflow named '{}' — declared and admitted: {}",
                args.name,
                if names.is_empty() {
                    "(none)".to_string()
                } else {
                    names.join(", ")
                }
            )
        })?;

    // 2. Roles → profiles → harnesses, against the MANIFEST (the authority).
    // Validation already proved every role names a declared profile; this
    // resolves the binding and refuses a harness the registry doesn't know
    // or that can't run headless.
    let bindings = resolve_bindings(&ctx, wf)?;

    // Invoker args: untrusted input, size-bounded before parse (depth is
    // bounded by serde_json at parse and by the engine boundary at install).
    let args_value = match &args.args_json {
        Some(text) => {
            anyhow::ensure!(
                text.len() <= MAX_ARGS_JSON_BYTES,
                "--args-json is {} bytes; the bound is {} (rule 7: invoker args are untrusted input)",
                text.len(),
                MAX_ARGS_JSON_BYTES
            );
            serde_json::from_str(text).context("--args-json is not valid JSON")?
        }
        None => serde_json::Value::Null,
    };

    // The pinned bytes, digest-sandwiched (see read_pinned_script).
    let script = read_pinned_script(wf)?;

    // Parse-only meta extraction at the CLI edge (contained — same belt and
    // suspenders as construction). The script parses twice, here and inside
    // the engine's own construction: accepted — parse-only, size-bounded,
    // deterministic — so the engine stays self-validating while the CLI
    // computes the grant below.
    let meta = contained(|| extract_meta(&script))?
        .map_err(|e| anyhow::anyhow!("refusing workflow '{}': {e}", wf.name))?;

    // Cross-check (witness 1, normalization side): script meta.roles must be
    // a SUBSET of the manifest's admitted role set. The manifest is the
    // authority (admission resolved and enforces it); meta.roles stays the
    // script-internal consistency set the bridge checks per call (R2). Moved
    // BEFORE construction in Stage D; the engine re-asserts the same subset
    // at construction (defense in depth against a cross-check bypass).
    for role in &meta.roles {
        anyhow::ensure!(
            wf.roles.contains(role),
            "refusing workflow '{}': the script's meta.roles names '{role}', which the \
             manifest's [workflows.{}] roles does not declare — manifest roles are the \
             authority; the script cannot widen them",
            wf.name,
            wf.name
        );
    }

    // Stage D ceiling chain, completed: effective = machine cap ∩ manifest
    // request ∩ script `meta` request. Admission already produced
    // min(machine, manifest) in `NormalizedWorkflow`; the script's request
    // may only NARROW that (rule 2 all the way down) — a meta asking for
    // MORE is clamped by the min, never an error, never a widen. The engine
    // receives only these final values; it never sees the wider layers.
    let effective_agents = meta
        .max_agents
        .map_or(wf.max_agents, |m| m.min(wf.max_agents));
    let effective_wall = meta
        .max_wall_seconds
        .map_or(wf.max_wall_seconds, |m| m.min(wf.max_wall_seconds));

    // Stage F: gate the resume BEFORE any recorder handle exists. Full
    // admission already re-ran above exactly as for a fresh run (rule 4:
    // resume is never an admission bypass); here every identity dimension is
    // compared against the journaled `WorkflowStarted` using the SAME digest
    // functions the original session used — never a second definition. An
    // identity refusal (and every replay refusal below) leaves the journal
    // byte-untouched, so a corrected re-attempt reads the same journal; the
    // `WorkflowResumed` marker appends only after the whole journaled prefix
    // has replayed cleanly.
    let mut replay = match &args.resume {
        Some(resume_id) => {
            let journal = ReplayJournal::load(resume_id)?;
            journal.verify_identity(
                &wf.name,
                &wf.checksum,
                &wf_grant_digest(effective_agents, effective_wall, &wf.roles),
                &wf_args_digest(args.args_json.as_deref()),
                effective_agents,
                effective_wall,
            )?;
            Some(journal)
        }
        None => None,
    };

    // Workflow-level evidence (Stage E): identity + recorder BEFORE engine
    // construction, so a construction refusal is itself recorded. Admission
    // refusals (trust/lock) and meta extraction happen pre-identity and stay
    // unrecorded at THIS level — an accepted v1 narrowing, recorded as a
    // decision (children still record their own attempts and gates; a
    // pre-gate workflow attempt record is additive later if ever wanted).
    //
    // Stage F: a resume reuses the journaled run id — the single log stays
    // the single journal (one mechanism; `RunLog::create` is idempotent on
    // an existing run dir) — and does NOT append a second `WorkflowStarted`:
    // the original identity row was just verified byte-identical, and the
    // resume marker is the session boundary instead.
    let run_id = match &args.resume {
        Some(resume_id) => resume_id.clone(),
        None => crate::runs::gen_workflow_id(),
    };
    let log = RunLog::create(&run_id).with_context(|| {
        format!(
            "could not create the workflow flight recorder for run {run_id} — refusing to run \
             unobserved"
        )
    })?;
    let wev = WorkflowEvidence {
        log,
        run_id: run_id.clone(),
        started: Instant::now(),
    };
    if replay.is_none() {
        wev.material(&RunEvent::WorkflowStarted {
            ts: ts(),
            workflow: wf.name.clone(),
            workflow_digest: wf.checksum.clone(),
            grant_digest: wf_grant_digest(effective_agents, effective_wall, &wf.roles),
            args_digest: wf_args_digest(args.args_json.as_deref()),
            max_agents: effective_agents,
            max_wall_seconds: effective_wall,
        })?;
    }

    // 3. Engine construction inside catch_unwind at the CLI edge. Both
    // refusal shapes are recorded terminally: an engine refusal under its
    // kind slug, an escaped panic (past both containments) as `panicked`.
    let grant = Grant {
        max_agents: effective_agents,
        max_wall_seconds: effective_wall,
        admitted_roles: wf.roles.clone(),
    };
    let constructed = match contained(|| {
        WorkflowRun::new(&script, RuntimeLimits::default(), args_value, grant)
    }) {
        Ok(inner) => inner,
        Err(e) => return Err(wev.fail("failed:panicked", false, e)),
    };
    let mut run = match constructed {
        Ok(run) => run,
        Err(e) => {
            let outcome = format!("failed:{}", kind_slug(e.kind));
            return Err(wev.fail(
                &outcome,
                false,
                anyhow::anyhow!("refusing workflow '{}': {e}", wf.name),
            ));
        }
    };

    // Effective concurrency: machine-configurable, engine-owned, never
    // script-visible. `.max(1)` so a machine cap of 0 bounds to serial
    // instead of deadlocking the drive.
    let max_concurrent = machine_policy
        .workflows
        .max_concurrent
        .unwrap_or(DEFAULT_MAX_CONCURRENT)
        .max(1) as usize;

    let narrowed = effective_agents != wf.max_agents || effective_wall != wf.max_wall_seconds;
    // The run id prints UNSTYLED: it is the `workflow report` entry point and
    // must be copyable/parseable from stderr without escape sequences.
    eprintln!(
        "{} workflow '{}' admitted: run {}, {} role(s), effective ceilings max_agents={} \
         max_wall_seconds={}{}, concurrency cap {}",
        "▶".green(),
        wf.name.bold(),
        run_id,
        wf.roles.len(),
        effective_agents,
        effective_wall,
        if narrowed {
            format!(
                " (script-narrowed from {}/{})",
                wf.max_agents, wf.max_wall_seconds
            )
        } else {
            String::new()
        },
        max_concurrent,
    );
    if let Some(journal) = &replay {
        eprintln!(
            "{} resuming: {} journaled step(s) will replay from verified artifacts — no \
             journaled step re-executes; the wall clock restarts at the full effective ceiling",
            "↻".cyan(),
            journal.remaining(),
        );
    }

    // 4. The out-of-thread watchdog, armed before the first step() at the
    // EFFECTIVE (possibly script-narrowed) wall ceiling plus a fixed grace —
    // the hard backstop above the cooperative deadline below. `done_tx` lives
    // to the end of this function; every return path drops it, which wakes
    // the watchdog with Disconnected and retires it. The exhaustion state
    // reaches the watchdog through the ENGINE's own cross-thread signal —
    // set at the refusal site itself — so a kill firing while the drive
    // thread is stuck inside a slice (after a caught refusal in the same
    // slice) still records the exhaustion truthfully; a drive-side mirror
    // polled between steps would be stale in exactly that scenario.
    let pids: crate::runs::ChildPids = Arc::new(Mutex::new(HashSet::new()));
    let done_tx = spawn_watchdog(
        wf.name.clone(),
        run_id.clone(),
        effective_wall,
        Arc::clone(&pids),
        run.exhausted_signal(),
        wev.started,
    );

    // The cooperative wall deadline (Stage D): checked at every batch
    // boundary, refusing the NEXT batch once the effective ceiling has
    // passed and failing the workflow cleanly, in-band, through the CLI's
    // normal error path (exit 1 — distinct from the watchdog's 124). This is
    // a LIVE-RUN backstop only: the clock lives here in the CLI, never in
    // the engine and never in replayable state — Stage F resume must not
    // spuriously time out replaying a run that originally took its full
    // wall clock.
    let deadline = std::time::Instant::now() + Duration::from_secs(effective_wall);

    // The drive loop: step → fan out the batch as locked children → feed the
    // results back — until Done or Failed. Exhaustion of the granted
    // max_agents ceiling is enforced INSIDE the engine, per call (Stage D):
    // the pending agent() call fails closed and the non-forgeable flag makes
    // it observable here.
    let mut results: Vec<StepResult> = Vec::new();
    let mut spawned_total: u64 = 0;
    // Completed result strings by step id, kept for the taint detector (D2).
    // Bounded: at most the effective agent ceiling entries, each at most the
    // child stdout capture cap.
    let mut completed_results: Vec<(u64, String)> = Vec::new();
    // Stage F: steps fed from the journal so far (the marker's count).
    let mut replayed_count: u64 = 0;
    let final_value = loop {
        let outcome = run.step(std::mem::take(&mut results));
        print_progress(run.take_progress());
        match outcome {
            StepOutcome::Batch(batch) => {
                if Instant::now() >= deadline {
                    drop(done_tx);
                    note_exhaustion(&run, effective_agents);
                    return Err(wev.fail(
                        "wall_deadline",
                        run.exhausted(),
                        anyhow::anyhow!(
                            "workflow '{}' exceeded its effective wall-clock ceiling ({}s) — \
                             refusing the next batch and failing cleanly in-band (the \
                             out-of-thread watchdog at ceiling+{}s remains the stall backstop; \
                             outcome recorded — see `agentstack workflow report {}`)",
                            wf.name,
                            effective_wall,
                            WATCHDOG_GRACE_SECS,
                            run_id
                        ),
                    ));
                }
                spawned_total += batch.requests.len() as u64;
                if spawned_total > u64::from(effective_agents) {
                    // Unreachable by design — the engine enforces the ceiling
                    // per call and can never hand out more spawns than the
                    // grant. Kept as defense in depth at the composition root:
                    // if it ever fires, an engine defect is voiding a
                    // negotiated machine limit, and the run fails closed.
                    // Deliberately witness-free (a test would have to fake an
                    // engine defect), like the serial-fallback label in C.
                    drop(done_tx);
                    return Err(wev.fail(
                        "engine_invariant_breach",
                        run.exhausted(),
                        anyhow::anyhow!(
                            "workflow '{}': engine-invariant breach — {} spawns handed out \
                             against a granted ceiling of {} — failing closed (this is an \
                             engine defect, not a script error; please report it)",
                            wf.name,
                            spawned_total,
                            effective_agents
                        ),
                    ));
                }
                // Stage F replay consumption: while a journal is active,
                // every batch member is checked against it BEFORE anything
                // spawns — a refused resume has spawned nothing and appended
                // nothing (refusals return in-band, deliberately NOT through
                // `wev.fail`: a terminal append would mutate the journal a
                // corrected re-attempt needs to read unchanged). Journaled
                // members feed as pre-resolved results; the only live spawns
                // are steps past the journal's end.
                let mut replay_feed: Vec<StepResult> = Vec::new();
                let mut live_owned: Vec<SpawnRequest> = Vec::new();
                let live: &[SpawnRequest] = if let Some(journal) = replay.as_mut() {
                    let mut taken = Vec::with_capacity(batch.requests.len());
                    for request in &batch.requests {
                        taken.push(journal.take(request, &wf_request_digest(request))?);
                    }
                    let journaled = taken.iter().filter(|t| t.is_some()).count();
                    if journaled == 0 {
                        // Past the journal's end — legal only once the
                        // journal is fully consumed (leftover entries name
                        // steps the engine never re-issued).
                        if journal.remaining() > 0 {
                            return Err(journal.refuse_leftover("at the first fully-live batch"));
                        }
                        wev.resumed(replayed_count)?;
                        replay = None;
                        &batch.requests
                    } else if journaled < batch.requests.len() {
                        // Never a genuine shape: spawn events are appended
                        // fail-closed for a WHOLE batch before it executes,
                        // so a batch mixing journaled and never-spawned
                        // members means a torn middle line or a doctored
                        // journal — refuse via the alignment gate.
                        let missing: Vec<String> = batch
                            .requests
                            .iter()
                            .zip(&taken)
                            .filter(|(_, t)| t.is_none())
                            .map(|(r, _)| format!("#{}", r.id))
                            .collect();
                        anyhow::bail!(
                            "refusing resume: the engine's batch mixes journaled steps with \
                             steps the journal never spawned ({}) — a genuine journal records \
                             a whole batch's spawns before executing it, so this journal is \
                             torn mid-line or doctored",
                            missing.join(", ")
                        );
                    } else {
                        // All members journaled: terminal-bearing ones
                        // replay; the rest re-execute LIVE (the straddle
                        // batch — spawned, then crash/kill before their
                        // terminals). AT-LEAST-ONCE, stated honestly: a
                        // re-executed member's original child may have run
                        // to completion (side effects included) without its
                        // terminal ever appending; salvaging its result from
                        // the child's own log is partial-result salvage —
                        // out of scope — so the step runs again.
                        let mut completed_taken = Vec::new();
                        for (request, t) in batch.requests.iter().zip(taken) {
                            let t = t.expect("counted journaled above");
                            if t.terminal.is_some() {
                                completed_taken.push(t);
                            } else {
                                live_owned.push(request.clone());
                            }
                        }
                        // Feed order = journal terminal order: that order IS
                        // the settlement order the live drive fed the engine
                        // (results were pushed in the same iteration that
                        // appended the events), and promise settlement order
                        // is script-observable.
                        completed_taken.sort_by_key(|t| t.terminal.map(|(_, idx)| idx));
                        for t in completed_taken {
                            match t.terminal.expect("filtered on Some").0 {
                                JournaledTerminal::Completed => {
                                    let text = read_verified_result(t.step, &t.child_run_id)?;
                                    // Replayed results are taint sources too,
                                    // same bounds — live-tail StepSpawned
                                    // events keep faithful marks.
                                    if text.len() >= TAINT_MIN_BYTES
                                        && completed_results.len() < TAINT_MAX_SOURCES
                                    {
                                        completed_results.push((
                                            t.step,
                                            truncate_on_char_boundary(&text, TAINT_NEEDLE_BYTES)
                                                .to_string(),
                                        ));
                                    }
                                    replay_feed.push(StepResult {
                                        request_id: t.step,
                                        output: StepOutput::Completed(serde_json::Value::String(
                                            text,
                                        )),
                                    });
                                }
                                // §3.1 / R1: a journaled failure replays as
                                // null — a failed step is NEVER respawned;
                                // retrying is a human's re-run decision.
                                JournaledTerminal::Failed => replay_feed.push(StepResult {
                                    request_id: t.step,
                                    output: StepOutput::Failed,
                                }),
                            }
                        }
                        replayed_count += replay_feed.len() as u64;
                        if live_owned.is_empty() {
                            // Fully-journaled batch: nothing spawns; the
                            // journal (possibly now empty) stays active — the
                            // marker appends at the live transition or at the
                            // run's end, whichever comes first.
                            &live_owned
                        } else {
                            // The straddle batch ends the replay: the marker
                            // precedes its live members' spawn events.
                            if journal.remaining() > 0 {
                                return Err(journal.refuse_leftover("at the straddle batch"));
                            }
                            wev.resumed(replayed_count)?;
                            replay = None;
                            &live_owned
                        }
                    }
                } else {
                    &batch.requests
                };

                // Stage E pre-spawn evidence: one StepSpawned per LIVE
                // request, appended FAIL-CLOSED before any child launches
                // (gate 2: an unrecordable spawn does not launch). The child
                // run id is pre-generated here so the event can name it.
                // Replayed steps emit nothing new — their events already
                // exist in the journal.
                let mut child_ids: HashMap<u64, String> = HashMap::new();
                for request in live {
                    let child_run_id = record_step_spawned(
                        &wev,
                        request,
                        bindings.get(&request.role),
                        &completed_results,
                    )?;
                    child_ids.insert(request.id, child_run_id);
                }
                let steps = execute_batch(
                    manifest_dir,
                    &bindings,
                    live,
                    max_concurrent,
                    &pids,
                    &child_ids,
                );
                // Step completions are material evidence too: an
                // unrecordable completion fails the run (gate 2) — better an
                // error than an unrecorded step. Replayed results feed FIRST
                // (their journal order), live results after in completion
                // order — for a straddle batch nothing was ever fed to the
                // original engine, so no prior settlement observation exists
                // to contradict.
                results = replay_feed;
                results.reserve(steps.len());
                for step in steps {
                    match &step.result.output {
                        StepOutput::Completed(value) => {
                            wev.material(&RunEvent::StepCompleted {
                                ts: ts(),
                                step: step.result.request_id,
                            })?;
                            // Bounded taint-source retention: only the first
                            // TAINT_MAX_SOURCES qualifying results, each kept
                            // as its needle prefix only — the detector never
                            // uses more, so the truncation changes no mark.
                            if let Some(text) = value.as_str() {
                                if text.len() >= TAINT_MIN_BYTES
                                    && completed_results.len() < TAINT_MAX_SOURCES
                                {
                                    completed_results.push((
                                        step.result.request_id,
                                        truncate_on_char_boundary(text, TAINT_NEEDLE_BYTES)
                                            .to_string(),
                                    ));
                                }
                            }
                        }
                        StepOutput::Failed => {
                            wev.material(&RunEvent::StepFailed {
                                ts: ts(),
                                step: step.result.request_id,
                                reason: step.reason.unwrap_or("failed").to_string(),
                            })?;
                        }
                    }
                    results.push(step.result);
                }
            }
            StepOutcome::Done(value) => {
                // Stage F: the run ended while the journal was still active
                // (a resume whose live tail was empty, or a journal naming
                // steps the engine never re-issued). Leftovers refuse
                // WITHOUT appending; a cleanly-exhausted journal marks the
                // session before the terminal below.
                if let Some(journal) = replay.take() {
                    if journal.remaining() > 0 {
                        return Err(journal.refuse_leftover("by the run's end"));
                    }
                    wev.resumed(replayed_count)?;
                }
                break value;
            }
            StepOutcome::Failed(err) => {
                drop(done_tx);
                note_exhaustion(&run, effective_agents);
                // Stage F, same discipline as Done: leftover journal entries
                // refuse without appending (the failure here is the REPLAY's
                // divergence, not a new outcome of the run); a cleanly
                // replayed prefix that then fails past the journal is a live
                // outcome of the resumed session — mark it, then record it.
                if let Some(journal) = replay.take() {
                    if journal.remaining() > 0 {
                        return Err(journal.refuse_leftover("at the engine failure"));
                    }
                    wev.resumed(replayed_count)?;
                }
                let outcome = format!("failed:{}", kind_slug(err.kind));
                return Err(wev.fail(
                    &outcome,
                    run.exhausted(),
                    anyhow::anyhow!("workflow '{}' failed: {err}", wf.name),
                ));
            }
        }
    };
    drop(done_tx);
    note_exhaustion(&run, effective_agents);
    // Done is material too (gate 2): evidence incompleteness fails the run
    // even after the value exists — an unrecorded success is not a success.
    wev.terminal("done", run.exhausted())?;
    Ok(final_value)
}

/// The Stage D honesty line: if any `agent()` call was refused at the granted
/// ceiling (the engine's non-forgeable flag — a script that caught and
/// absorbed the refusal cannot hide it), say so on stderr regardless of how
/// the run ended. The recorded half is the `exhausted` field on the terminal
/// `WorkflowCompleted` event; `workflow report` states it from the record.
fn note_exhaustion(run: &WorkflowRun, granted: u32) {
    if run.exhausted() {
        eprintln!(
            "{} the granted agent ceiling ({granted}) was exhausted during this run — refused \
             agent() calls failed closed (the script saw each refusal); the exhaustion is \
             recorded in the workflow run evidence",
            "⚠".yellow()
        );
    }
}

/// The CLI-edge panic containment (belt and suspenders beside the engine's
/// own AL5): a panic unwinding out of `f` becomes a clean refusal, never an
/// abort of the launcher. Generic so the witness test can drive it with a
/// panicking closure directly.
fn contained<T>(f: impl FnOnce() -> T) -> Result<T> {
    catch_unwind(AssertUnwindSafe(f)).map_err(|_| {
        anyhow::anyhow!("the workflow engine panicked at the CLI edge — refusing the run")
    })
}

/// Resolve each admitted role to its profile's harness binding. `Profile.harness`
/// is consulted ONLY here (interactive `run <harness> --profile` keeps its
/// positional harness); absent means the engine default, claude-code.
fn resolve_bindings(
    ctx: &super::Context,
    wf: &NormalizedWorkflow,
) -> Result<HashMap<String, RoleBinding>> {
    let mut bindings = HashMap::new();
    for role in &wf.roles {
        let profile = ctx.loaded.manifest.profiles.get(role).with_context(|| {
            format!(
                "role '{role}' names no declared profile — admission should have refused this \
                 (validation drift?)"
            )
        })?;
        let harness = profile
            .harness
            .clone()
            .unwrap_or_else(|| "claude-code".to_string());
        let desc = ctx.registry.get(&harness).with_context(|| {
            format!(
                "role '{role}' binds harness '{harness}', which is not a known adapter — \
                 see `agentstack adapters list`"
            )
        })?;
        anyhow::ensure!(
            desc.headless.is_some(),
            "role '{role}' binds harness '{harness}', which declares no headless invocation \
             spec — workflow children are headless locked runs; shipped support: claude-code, \
             codex",
        );
        bindings.insert(
            role.clone(),
            RoleBinding {
                harness: harness.clone(),
                injectable: supports_injection(desc),
                codex_residual: desc.id == "codex",
            },
        );
    }
    Ok(bindings)
}

/// Read the pinned script bytes under a digest sandwich: digest → bounded
/// read → digest again, all three required to equal the ADMITTED checksum.
/// The sandwich narrows the admission-to-read TOCTOU window to a concurrent
/// mutation that is reverted between the two digest walks — vanishingly
/// narrow and outside the local threat model; the load-bearing guarantee
/// ("the bytes handed to the engine are bytes whose digest matches the pin")
/// holds at both instants surrounding the read.
fn read_pinned_script(wf: &NormalizedWorkflow) -> Result<String> {
    let digest_now = |when: &str| -> Result<()> {
        let digest = agentstack_core::digest::integrity_root_digest(&wf.anchor, &wf.declared)
            .with_context(|| format!("re-digesting workflow '{}' {when}", wf.name))?
            .hex()
            .to_string();
        anyhow::ensure!(
            digest == wf.checksum,
            "workflow '{}' drifted {when}: content digest no longer matches the admitted pin — \
             run `agentstack lock`, review, and re-trust",
            wf.name
        );
        Ok(())
    };

    digest_now("between admission and read")?;
    let path = script_entry_path(&wf.anchor, &wf.declared)?;
    let script = crate::util::read_to_string_bounded(&path, MAX_SCRIPT_BYTES)
        .with_context(|| format!("reading the pinned workflow script {}", path.display()))?;
    digest_now("during the read")?;
    Ok(script)
}

/// The script entry file for a pinned source: the declared path itself when
/// it is a file (§3: a workflow is one file), or `main.js` under it when the
/// declared path is a directory.
fn script_entry_path(anchor: &Path, declared: &str) -> Result<PathBuf> {
    let root = anchor.join(declared);
    if root.is_file() {
        return Ok(root);
    }
    let entry = root.join("main.js");
    anyhow::ensure!(
        entry.is_file(),
        "workflow source {} is a directory with no main.js entry file",
        root.display()
    );
    Ok(entry)
}

/// Arm the out-of-thread watchdog at `effective_wall` (the effective,
/// possibly script-narrowed ceiling in seconds) plus [`WATCHDOG_GRACE_SECS`].
/// Returns the completion sender: dropping it (any normal exit path) retires
/// the watchdog; a wall-clock overrun prints an honest line — naming the
/// effective ceiling and the grace SEPARATELY, so the message never
/// contradicts the admitted ceiling or `budget.maxWallSeconds` — appends the
/// terminal event BEST-EFFORT (witness 5's recorded half), best-effort
/// SIGTERMs the live child process groups, and force-exits the PROCESS — the
/// §12.2 hard backstop, deliberately not a cooperative check (a drive thread
/// stuck inside a Boa builtin slice cannot observe a deadline).
fn spawn_watchdog(
    name: String,
    run_id: String,
    effective_wall: u64,
    pids: crate::runs::ChildPids,
    exhausted: Arc<AtomicBool>,
    started: Instant,
) -> mpsc::Sender<()> {
    let armed = Duration::from_secs(effective_wall.saturating_add(WATCHDOG_GRACE_SECS));
    let (done_tx, done_rx) = mpsc::channel::<()>();
    std::thread::spawn(move || match done_rx.recv_timeout(armed) {
        // Completion (or the sender dropped on any exit path): retire.
        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {}
        Err(mpsc::RecvTimeoutError::Timeout) => {
            eprintln!(
                "✗ workflow '{name}' ran past its effective wall-clock ceiling ({effective_wall}s) \
                 plus the {WATCHDOG_GRACE_SECS}s watchdog grace — force-exiting (out-of-thread \
                 watchdog: a stalled engine slice cannot be interrupted cooperatively). Live \
                 children receive SIGTERM; the outcome is recorded best-effort — see \
                 `agentstack workflow report {run_id}`.",
            );
            // The ONE exception to the fail-closed recording gate: this
            // thread is already dying, so the append is BEST-EFFORT (the
            // unchecked variant) and a failed append still exits 124 —
            // dying honestly beats not dying; the exit is the hard
            // guarantee, not this record. `RunLog::create` is idempotent
            // (a directory handle), and the single O_APPEND write cannot
            // tear against a concurrent drive-thread append.
            //
            // Known boundary race, documented not "fixed": if the drive
            // completes at the exact deadline instant, `recv_timeout` may
            // have already selected Timeout, and the log can carry BOTH a
            // `done` and a `watchdog_kill` terminal (both genuinely
            // happened; the report shows the last). Narrowing it would mean
            // re-checking the channel before killing — an enforcement-timing
            // change, out of scope for evidence-only Stage E; candidate for
            // a later stage with its own approval.
            if let Some(log) = RunLog::create(&run_id) {
                log.append(&RunEvent::WorkflowCompleted {
                    ts: ts(),
                    outcome: "watchdog_kill".to_string(),
                    exhausted: exhausted.load(Ordering::Relaxed),
                    duration_ms: started.elapsed().as_millis() as u64,
                });
            }
            let held: Vec<i32> = pids
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .copied()
                .collect();
            for pid in held {
                // Best-effort cleanup (children are their own process groups);
                // the hard guarantee is the exit below, not this signal.
                let _ = crate::sys::signal_group(pid, crate::sys::Signal::Term);
            }
            std::process::exit(WATCHDOG_EXIT_CODE);
        }
    });
    done_tx
}

fn print_progress(progress: Vec<Progress>) {
    for event in progress {
        eprintln!("{}", format_progress(&event));
    }
}

/// One progress event as the line the terminal gets. Script-controlled text:
/// sanitized here (rule 7), so no `phase()`/`log()` string can smuggle
/// terminal escapes past this seam.
fn format_progress(event: &Progress) -> String {
    match event {
        Progress::Phase(title) => format!("{} {}", "◆".cyan(), sanitize_line(title).bold()),
        Progress::Log(line) => format!("  {} {}", "·".dimmed(), sanitize_line(line)),
    }
}

/// One executed step: the engine-facing result plus the launcher-authored
/// failure category the drive loop records in `StepFailed.reason` (Stage E).
/// The category is OURS, never upstream or script text (redaction gate 3).
struct ChildStep {
    result: StepResult,
    reason: Option<&'static str>,
}

/// Fan one engine batch out as locked children. Injection-capable children
/// run concurrently under the cap (each is an independent worker thread
/// re-running the FULL per-child gate sequence — trust, strict verify,
/// admission, grant freeze — via `run_locked_child`); park/swap children run
/// strictly serially afterwards, labeled `serial (config-swap)` (§12.1: the
/// one deliberate degrade, stated honestly and recorded on `StepSpawned`).
/// Each child runs under the run id the drive loop pre-announced in its
/// `StepSpawned` event (`child_ids`, keyed by request id). Results are keyed
/// by request id, so completion order is irrelevant.
fn execute_batch(
    manifest_dir: Option<&Path>,
    bindings: &HashMap<String, RoleBinding>,
    requests: &[SpawnRequest],
    max_concurrent: usize,
    pids: &crate::runs::ChildPids,
    child_ids: &HashMap<u64, String>,
) -> Vec<ChildStep> {
    let (concurrent, serial): (Vec<&SpawnRequest>, Vec<&SpawnRequest>) = requests
        .iter()
        .partition(|r| bindings.get(&r.role).map(|b| b.injectable).unwrap_or(false));

    let results: Mutex<Vec<ChildStep>> = Mutex::new(Vec::with_capacity(requests.len()));
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..max_concurrent.min(concurrent.len()) {
            // Workers pull from a shared index; each borrows the batch state
            // for the scope's lifetime (shared refs are Copy, so `move` just
            // copies the borrows into the worker).
            let (next, results, concurrent) = (&next, &results, &concurrent);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::SeqCst);
                let Some(request) = concurrent.get(i) else {
                    break;
                };
                let result = run_child(manifest_dir, bindings, request, pids, false, child_ids);
                results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(result);
            });
        }
    });
    for request in serial {
        let result = run_child(manifest_dir, bindings, request, pids, true, child_ids);
        results
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(result);
    }
    results.into_inner().unwrap_or_else(|e| e.into_inner())
}

/// Run one spawn request as a locked child and consume its outcome. F5: the
/// child's bounded stdout resolves the `agent()` promise DIRECTLY as one
/// verbatim string (`from_utf8_lossy` is the only transform — no courier, no
/// JSON hand-copy, no trim). F3: success/failure is consumed from the child
/// run's RECORDED `LockedOutcome`, never the process exit alone.
fn run_child(
    manifest_dir: Option<&Path>,
    bindings: &HashMap<String, RoleBinding>,
    request: &SpawnRequest,
    pids: &crate::runs::ChildPids,
    serial: bool,
    child_ids: &HashMap<u64, String>,
) -> ChildStep {
    let completed = |value: serde_json::Value| ChildStep {
        result: StepResult {
            request_id: request.id,
            output: StepOutput::Completed(value),
        },
        reason: None,
    };
    let failed = |reason: &'static str| ChildStep {
        result: StepResult {
            request_id: request.id,
            output: StepOutput::Failed,
        },
        reason: Some(reason),
    };
    let Some(binding) = bindings.get(&request.role) else {
        // The bridge + admission cross-check make this unreachable; refusing
        // the step (not the process) keeps the failure in-band regardless.
        eprintln!(
            "  ✗ agent #{} names unbound role '{}' — failing the step closed",
            request.id,
            sanitize_line(&request.role)
        );
        return failed("unbound_role");
    };
    let Some(run_id) = child_ids.get(&request.id) else {
        // Same class: the drive loop pre-announces every id; a missing one
        // is a composition bug, and the step fails closed in-band.
        eprintln!(
            "  ✗ agent #{} has no pre-announced child run id — failing the step closed",
            request.id,
        );
        return failed("missing_child_id");
    };

    // The per-child header line. The label is SCRIPT-controlled → sanitized.
    let label = request
        .opts
        .get("label")
        .and_then(|v| v.as_str())
        .map(|l| format!(" [{}]", sanitize_line(l)))
        .unwrap_or_default();
    eprintln!(
        "  {} agent #{}{label} role={} harness={}{}",
        "▶".green(),
        request.id,
        request.role,
        binding.harness,
        if serial {
            " — serial (config-swap)"
        } else {
            ""
        },
    );
    if binding.codex_residual {
        // Gate condition 1 (§12.1): the codex connector residual is surfaced
        // at RUN TIME, per codex child — N children multiply the exposure.
        eprintln!(
            "    {} codex's account/plugin connector layer (codex_apps) is NOT scoped by the \
             per-run MCP config or --ignore-user-config — those connectors stay live and \
             network-reaching around the gateway on the host tier. Use --lockdown for \
             kernel-level fencing.",
            "⚠".yellow()
        );
    }

    let child_args = RunArgs {
        harness: binding.harness.clone(),
        locked: true,
        prompt: Some(request.prompt.clone()),
        // The role's profile FENCES the child (witness 9: its grant is ≤ the
        // profile's capability set — the shipped W2 profile-fence semantics).
        profile: Some(request.role.clone()),
        scope: None,
        keep: false,
        sandbox: false,
        lockdown: false,
        plan: false,
        args: Vec::new(),
    };

    // Belt and suspenders, the same principle as the engine's AL5 and the
    // `contained()` CLI edge: the locked seam parses hostile input (manifest,
    // lock, executable pins — rule 7), so a panic in ONE child must fail THAT
    // step closed. Uncontained, a worker panic would re-raise when
    // `thread::scope` joins and abort the whole workflow — one hostile child
    // taking down its siblings and the run.
    let spawned = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(test)]
        panic_probe(&request.prompt);
        run_locked_child(manifest_dir, &child_args, pids, run_id)
    }));
    let spawned = match spawned {
        Ok(result) => result,
        Err(_) => {
            eprintln!(
                "  {} agent #{} panicked in the child spawner — failing the step closed \
                 (the script sees null and decides severity)",
                "✗".red(),
                request.id,
            );
            return failed("spawner_panic");
        }
    };
    match spawned {
        Ok(report) => {
            // F3: the recorded outcome is the authority, not the process exit.
            match recorded_outcome(&report.run_id) {
                Some((outcome, exit_code)) if outcome == "completed" && exit_code == Some(0) => {
                    eprintln!(
                        "  {} agent #{} completed (run {}, grant {}{})",
                        "✓".green(),
                        request.id,
                        report.run_id,
                        report.grant_digest,
                        if report.truncated {
                            " — output truncated at the capture cap"
                        } else {
                            ""
                        },
                    );
                    completed(serde_json::Value::String(
                        String::from_utf8_lossy(&report.stdout).into_owned(),
                    ))
                }
                recorded => {
                    eprintln!(
                        "  {} agent #{} failed (run {}, recorded outcome: {:?}) — the script \
                         sees null and decides severity",
                        "✗".red(),
                        request.id,
                        report.run_id,
                        recorded,
                    );
                    failed("child_failed")
                }
            }
        }
        Err(e) => {
            // A gate refusal (trust, verify, admission, freeze) or launch
            // failure: recorded by the child path itself; the step fails
            // closed and the script decides severity (R1).
            eprintln!("  {} agent #{} refused: {:#}", "✗".red(), request.id, e);
            failed("refused")
        }
    }
}

/// Test-only seam for the per-child panic-containment witness: a sentinel
/// prompt panics inside the contained spawner closure, proving a panicking
/// child fails its own step while its siblings and the workflow survive.
/// `cfg(test)` only — no production prompt can reach it.
#[cfg(test)]
fn panic_probe(prompt: &str) {
    if prompt == "__agentstack_test_child_panic__" {
        panic!("intentional child-spawner panic (test probe)");
    }
}

/// The child run's recorded terminal outcome: `(outcome, exit_code)` from the
/// LAST `LockedOutcome` event in its evidence log, or `None` when no outcome
/// was recorded (which the caller treats as failure — observed evidence or
/// nothing, never an assumption).
fn recorded_outcome(run_id: &str) -> Option<(String, Option<i32>)> {
    crate::calllog::RunLog::read(run_id)
        .into_iter()
        .rev()
        .find_map(|event| match event {
            crate::calllog::RunEvent::LockedOutcome {
                outcome, exit_code, ..
            } => Some((outcome, exit_code)),
            _ => None,
        })
}

/// `agentstack workflow report <run-id>` — render the recorded evidence tree
/// as text, or as JSON with `--json` (the same join, structured for
/// scripting instead of a human).
pub fn report(args: &WorkflowReportArgs) -> Result<()> {
    if args.json {
        // Stdout is the deliverable here too (same convention as
        // `report_json` in commands/report.rs): pretty JSON, nothing else.
        println!("{}", render_workflow_report_json(&args.run_id)?);
    } else {
        print!("{}", render_workflow_report(&args.run_id)?);
    }
    Ok(())
}

/// One `StepSpawned` OCCURRENCE (not deduplicated by step id — a resumed run
/// that re-executes a straddling member appends a FRESH spawn event for the
/// same step id, and both the superseded and the live occurrence render, each
/// annotated) joined with its child run's own recorded evidence.
struct StepEvidence {
    step: u64,
    role: String,
    child_run_id: String,
    label: Option<String>,
    taint: Vec<u64>,
    serial: bool,
    codex_residual: bool,
    /// This occurrence's position in the workflow log — the session-relative
    /// annotation is computed from it against `last_marker_idx`.
    spawn_idx: usize,
    /// The step id's LAST recorded completion in the workflow log (last-wins
    /// across a resumed session) — shared by every occurrence of that step
    /// id, matching the pre-refactor behavior exactly.
    step_completed: bool,
    step_failed_reason: Option<String>,
    step_terminal_idx: Option<usize>,
    /// Whether this specific child run has ANY recorded evidence at all —
    /// the "spawned" (nothing yet) vs "running" (attempt started, no
    /// terminal) distinction below.
    child_log_present: bool,
    posture: Option<String>,
    child_grant_digest: Option<String>,
    outcome: Option<String>,
    exit_code: Option<i32>,
    duration_ms: Option<u64>,
    usage: Option<String>,
    /// Count of `ToolCall`-kind events in the child's OWN log — never the
    /// machine-global `calls.jsonl` (that dedup is `report_json`'s concern
    /// for a gateway-routed run; a workflow child's own log is the join
    /// source here, per the child's `RunLog` alone).
    tool_calls: usize,
}

/// The full evidence join for one workflow run — computed ONCE from the
/// recorded events (RunLog::read is the only read), then rendered as text
/// ([`render_workflow_report`]) or JSON ([`render_workflow_report_json`]).
/// Every field traces to a specific recorded event; nothing here is
/// reconstructed.
struct WorkflowReportEvidence {
    run_id: String,
    workflow: String,
    workflow_digest: String,
    grant_digest: String,
    args_digest: String,
    max_agents: u32,
    max_wall_seconds: u64,
    /// Resume markers in log order: `(ts, replayed_steps)`.
    resumes: Vec<(u64, u64)>,
    last_marker_idx: Option<usize>,
    steps: Vec<StepEvidence>,
    /// ALL recorded `WorkflowCompleted` terminals, oldest first (unsanitized
    /// — sanitization is a rendering concern, done at each renderer's own
    /// terminal seam).
    terminals: Vec<(String, bool, u64)>,
}

/// A step's log-recorded completion, if any — `None` means neither
/// `StepCompleted` nor `StepFailed` has been recorded for that step id yet.
enum StepTerminal {
    Completed,
    Failed(String),
}

/// Read a workflow run's log and JOIN every step's evidence from its child's
/// own log via `child_run_id` (the §6 step-3 join-table shape — `StepSpawned`
/// structurally carries neither grant digest nor posture nor outcome).
/// Refuses (bails) when the run is not a workflow run at all — same refusal
/// both renderers surface identically.
fn collect_workflow_report(run_id: &str) -> Result<WorkflowReportEvidence> {
    let events = RunLog::read(run_id);
    anyhow::ensure!(
        !events.is_empty(),
        "no recorded events for run '{run_id}' — workflow run ids (w-…) are printed on the \
         run's admission banner"
    );
    let started = events.iter().find_map(|e| match e {
        RunEvent::WorkflowStarted {
            workflow,
            workflow_digest,
            grant_digest,
            args_digest,
            max_agents,
            max_wall_seconds,
            ..
        } => Some((
            workflow.clone(),
            workflow_digest.clone(),
            grant_digest.clone(),
            args_digest.clone(),
            *max_agents,
            *max_wall_seconds,
        )),
        _ => None,
    });
    let Some((workflow, workflow_digest, grant_digest, args_digest, max_agents, max_wall_seconds)) =
        started
    else {
        anyhow::bail!(
            "run '{run_id}' is not a workflow run (no workflow_started event) — for a locked \
             or sandboxed run, use `agentstack report run {run_id}`"
        );
    };

    // Stage F: resume markers segment the log into sessions — everything
    // after the LAST marker is the newest session's live tail; steps before
    // it were carried forward by replay (or superseded and re-executed).
    let markers: Vec<(usize, u64, u64)> = events
        .iter()
        .enumerate()
        .filter_map(|(idx, e)| match e {
            RunEvent::WorkflowResumed {
                ts, replayed_steps, ..
            } => Some((idx, *ts, *replayed_steps)),
            _ => None,
        })
        .collect();
    let last_marker_idx = markers.last().map(|(idx, _, _)| *idx);
    let resumes = markers
        .iter()
        .map(|(_, ts, replayed)| (*ts, *replayed))
        .collect();

    // Step completion states, keyed by step id (last-wins, so a re-executed
    // step shows its resumed session's terminal), plus each step's last
    // terminal POSITION for the replayed/superseded distinction below.
    let mut step_terminal: HashMap<u64, StepTerminal> = HashMap::new();
    let mut step_terminal_idx: HashMap<u64, usize> = HashMap::new();
    for (idx, event) in events.iter().enumerate() {
        match event {
            RunEvent::StepCompleted { step, .. } => {
                step_terminal.insert(*step, StepTerminal::Completed);
                step_terminal_idx.insert(*step, idx);
            }
            RunEvent::StepFailed { step, reason, .. } => {
                step_terminal.insert(*step, StepTerminal::Failed(reason.clone()));
                step_terminal_idx.insert(*step, idx);
            }
            _ => {}
        }
    }

    let mut steps = Vec::new();
    for (idx, event) in events.iter().enumerate() {
        let RunEvent::StepSpawned {
            step,
            role,
            child_run_id,
            label,
            taint,
            serial,
            codex_residual,
            ..
        } = event
        else {
            continue;
        };

        // The JOIN: this child's own recorded evidence, by run id. Absent
        // pieces are stated absent — evidence or nothing, never assumption.
        let child_events = RunLog::read(child_run_id);
        let child_log_present = !child_events.is_empty();
        let posture = child_events.iter().find_map(|e| match e {
            RunEvent::AttemptStarted { posture, .. } => Some(posture.clone()),
            _ => None,
        });
        let child_grant_digest = child_events.iter().find_map(|e| match e {
            RunEvent::GrantFrozen { grant_digest, .. } => Some(grant_digest.clone()),
            _ => None,
        });
        let terminal = child_events.iter().rev().find_map(|e| match e {
            RunEvent::LockedOutcome {
                outcome,
                exit_code,
                duration_ms,
                usage,
                ..
            } => Some((outcome.clone(), *exit_code, *duration_ms, usage.clone())),
            _ => None,
        });
        let tool_calls = child_events
            .iter()
            .filter(|e| matches!(e, RunEvent::ToolCall { .. }))
            .count();

        let (step_completed, step_failed_reason) = match step_terminal.get(step) {
            Some(StepTerminal::Completed) => (true, None),
            Some(StepTerminal::Failed(reason)) => (false, Some(reason.clone())),
            None => (false, None),
        };
        let (outcome, exit_code, duration_ms, usage) = match terminal {
            Some((outcome, exit_code, duration_ms, usage)) => {
                (Some(outcome), exit_code, Some(duration_ms), Some(usage))
            }
            None => (None, None, None, None),
        };

        steps.push(StepEvidence {
            step: *step,
            role: role.clone(),
            child_run_id: child_run_id.clone(),
            label: label.clone(),
            taint: taint.clone(),
            serial: *serial,
            codex_residual: *codex_residual,
            spawn_idx: idx,
            step_completed,
            step_failed_reason,
            step_terminal_idx: step_terminal_idx.get(step).copied(),
            child_log_present,
            posture,
            child_grant_digest,
            outcome,
            exit_code,
            duration_ms,
            usage,
            tool_calls,
        });
    }

    // ALL recorded terminals, oldest first: the last is the run's outcome;
    // earlier ones are stated too (a log carries more than one after a
    // resume, or on the documented done/watchdog_kill boundary race).
    let terminals: Vec<(String, bool, u64)> = events
        .iter()
        .filter_map(|e| match e {
            RunEvent::WorkflowCompleted {
                outcome,
                exhausted,
                duration_ms,
                ..
            } => Some((outcome.clone(), *exhausted, *duration_ms)),
            _ => None,
        })
        .collect();

    Ok(WorkflowReportEvidence {
        run_id: run_id.to_string(),
        workflow,
        workflow_digest,
        grant_digest,
        args_digest,
        max_agents,
        max_wall_seconds,
        resumes,
        last_marker_idx,
        steps,
        terminals,
    })
}

/// Everything rendered is FROM THE RECORD (see [`collect_workflow_report`]):
/// child grant digests, postures, and outcomes are JOINED from each child's
/// own run log via `child_run_id`, `usage` is the recorded value
/// (`"unavailable"` today, never a fabricated number), and nothing is
/// reconstructed. Script-authored strings (labels) are sanitized HERE, at
/// the terminal seam (rule 7 — the event file stores them as bounded JSON
/// data).
fn render_workflow_report(run_id: &str) -> Result<String> {
    use std::fmt::Write as _;

    let ev = collect_workflow_report(run_id)?;

    let mut out = String::new();
    writeln!(
        out,
        "Workflow run {}: '{}'",
        ev.run_id,
        sanitize_line(&ev.workflow)
    )?;
    writeln!(out, "  script digest   {}", ev.workflow_digest)?;
    writeln!(out, "  grant digest    {}", ev.grant_digest)?;
    writeln!(out, "  args digest     {}", ev.args_digest)?;
    writeln!(
        out,
        "  effective ceilings  max_agents={} max_wall_seconds={}",
        ev.max_agents, ev.max_wall_seconds
    )?;
    writeln!(out)?;

    if !ev.resumes.is_empty() {
        writeln!(out, "Resumed {} time(s):", ev.resumes.len())?;
        for (ts, replayed) in &ev.resumes {
            writeln!(
                out,
                "  at ts={ts}: {replayed} step(s) replayed from the journal (no journaled \
                 step re-executed)"
            )?;
        }
        writeln!(out)?;
    }

    writeln!(out, "Steps:")?;
    if ev.steps.is_empty() {
        writeln!(out, "  (no steps spawned)")?;
    }
    for step in &ev.steps {
        let mut line = format!("  #{} role={}", step.step, sanitize_line(&step.role));
        // Stage F session annotation, relative to the LAST resume marker: a
        // spawn after it ran live in the resumed session; a spawn before it
        // either replayed (its terminal predates the marker) or was
        // superseded (spawned, never terminated — re-executed live, and its
        // later spawn event shows the re-execution).
        if let Some(marker) = ev.last_marker_idx {
            if step.spawn_idx > marker {
                line.push_str(" [live, resumed session]");
            } else if step.step_terminal_idx.is_some_and(|t| t < marker) {
                line.push_str(" [replayed on resume]");
            } else {
                line.push_str(" [superseded — re-executed live on resume]");
            }
        }
        if let Some(label) = &step.label {
            line.push_str(&format!(" [{}]", sanitize_line(label)));
        }
        line.push_str(&format!(" child={}", step.child_run_id));
        if step.serial {
            line.push_str(" — serial (config-swap)");
        }
        if step.codex_residual {
            line.push_str(" — codex_apps residual (connector layer unfenced on host tier)");
        }
        if !step.taint.is_empty() {
            let sources: Vec<String> = step.taint.iter().map(|t| format!("#{t}")).collect();
            line.push_str(&format!(
                " — taint: prompt embeds output of {}",
                sources.join(", ")
            ));
        }
        writeln!(out, "{line}")?;

        let (outcome_text, usage_text) = match (&step.outcome, &step.usage) {
            (Some(outcome), Some(usage)) => (
                match step.exit_code {
                    Some(code) => format!("{outcome} (exit {code})"),
                    None => outcome.clone(),
                },
                usage.clone(),
            ),
            _ => (
                "no recorded outcome (refused pre-launch or interrupted)".to_string(),
                "unavailable".to_string(),
            ),
        };
        writeln!(
            out,
            "     child: grant={} posture={} outcome={} usage={}",
            step.child_grant_digest.as_deref().unwrap_or("not frozen"),
            step.posture.as_deref().unwrap_or("unrecorded"),
            outcome_text,
            usage_text,
        )?;
        writeln!(
            out,
            "     step:  {}",
            if step.step_completed {
                "completed".to_string()
            } else if let Some(reason) = &step.step_failed_reason {
                format!("failed ({})", sanitize_line(reason))
            } else {
                "no completion recorded (interrupted)".to_string()
            }
        )?;
    }
    writeln!(out)?;

    // `outcome` is launcher-authored on a genuine log, but the log is
    // editable — sanitized at this terminal seam like every other field.
    match ev.terminals.last() {
        Some((outcome, exhausted, duration_ms)) => {
            writeln!(
                out,
                "Outcome: {} ({duration_ms} ms)",
                sanitize_line(outcome)
            )?;
            for (earlier, _, earlier_ms) in &ev.terminals[..ev.terminals.len() - 1] {
                writeln!(
                    out,
                    "  earlier terminal: {} ({earlier_ms} ms) — superseded by a later \
                     session or the recorded boundary race",
                    sanitize_line(earlier)
                )?;
            }
            if *exhausted {
                writeln!(
                    out,
                    "  the granted agent ceiling was EXHAUSTED during this run — refused \
                     agent() calls failed closed (the script saw each refusal)"
                )?;
            }
        }
        None => writeln!(
            out,
            "Outcome: NO TERMINAL EVENT RECORDED — the run crashed, was killed before the \
             watchdog could record, is still running, or a recording failure refused it \
             in-band (evidence stops where recording stopped)"
        )?,
    }
    writeln!(out)?;

    // The honesty block: the exported POSTURE_LABEL, verbatim — the single
    // source of the §12.2/§12.3 claim, including the runtime-data ReDoS
    // residual paragraph. Never paraphrased, never forked.
    writeln!(out, "Honest posture (§12.2, verbatim):")?;
    writeln!(out, "{}", agentstack_workflow::POSTURE_LABEL)?;
    Ok(out)
}

/// The JSON twin of [`render_workflow_report`] — the SAME join
/// ([`collect_workflow_report`]), reshaped for scripting rather than a
/// human. Deliberately narrower than the text render: it carries the fields
/// a caller would script against (identity, ceilings, per-step outcome
/// evidence) and omits text-only presentation (resume-marker prose, the
/// superseded/replayed/live session annotation, the verbatim honesty
/// block) — additive later if a scripting need for those ever appears.
fn render_workflow_report_json(run_id: &str) -> Result<String> {
    let ev = collect_workflow_report(run_id)?;

    // Top-level `outcome` is a normalized 3-way slug, distinct from the raw
    // recorded strings ("done", "failed:<kind>", "wall_deadline",
    // "engine_invariant_breach", "watchdog_kill"): "completed" only for the
    // literal "done" terminal, "failed" for every other terminal shape, and
    // "running" when no `WorkflowCompleted` has been recorded yet (in which
    // case `duration_ms` is also null — there is no terminal to read it
    // from).
    let (outcome, exhausted, duration_ms): (&str, bool, Option<u64>) = match ev.terminals.last() {
        Some((raw, exhausted, duration_ms)) => (
            if raw == "done" { "completed" } else { "failed" },
            *exhausted,
            Some(*duration_ms),
        ),
        None => ("running", false, None),
    };

    let steps: Vec<serde_json::Value> = ev
        .steps
        .iter()
        .map(|step| {
            // "completed"/"failed" come from the WORKFLOW log's own
            // completion record for this step id (authoritative — it is
            // what resolved or rejected the `agent()` promise); absent
            // that, "running" vs "spawned" is read from whether THIS
            // occurrence's child has any recorded evidence at all yet.
            let state = if step.step_completed {
                "completed"
            } else if step.step_failed_reason.is_some() {
                "failed"
            } else if step.child_log_present {
                "running"
            } else {
                "spawned"
            };
            serde_json::json!({
                "step": step.step,
                "role": sanitize_line(&step.role),
                "label": step.label.as_deref().map(sanitize_line),
                "child_run_id": step.child_run_id,
                "state": state,
                "serial": step.serial,
                "taint": step.taint,
                "posture": step.posture,
                "grant_digest": step.child_grant_digest,
                "outcome": step.outcome,
                "exit_code": step.exit_code,
                "tool_calls": step.tool_calls,
                "duration_ms": step.duration_ms,
            })
        })
        .collect();

    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "run": ev.run_id,
        "workflow": sanitize_line(&ev.workflow),
        "workflow_digest": ev.workflow_digest,
        "grant_digest": ev.grant_digest,
        "args_digest": ev.args_digest,
        // No top-level "posture" concept is recorded for a workflow run
        // (only a LOCKED CHILD carries `AttemptStarted.posture`) — always
        // null; the per-step `posture` field above is where this signal
        // actually lives, once per child.
        "posture": None::<String>,
        "outcome": outcome,
        "exhausted": exhausted,
        "duration_ms": duration_ms,
        "max_agents": ev.max_agents,
        "max_wall_seconds": ev.max_wall_seconds,
        "steps": steps,
    }))?)
}

/// One `[workflows.*]` manifest entry's admission status — the row `list`
/// prints or emits.
struct WorkflowListRow {
    name: String,
    /// Always `true`: every row in this list comes from an entry that IS
    /// declared in the manifest — `list` never invents undeclared entries.
    declared: bool,
    trusted: bool,
    lock_status: &'static str,
    roles: Vec<String>,
    max_agents: u32,
    max_wall_seconds: u64,
    checksum: Option<String>,
}

/// `agentstack workflow list` — every declared `[workflows.*]` entry with its
/// admission status, READ-ONLY and refusal-free: unlike `run` (which gates
/// the named workflow through the full W1 admission choke point,
/// [`crate::workflows::normalized_workflows`], and refuses the WHOLE call on
/// the first untrusted or drifted entry), `list` must surface every declared
/// name regardless of admission state — so it does not call that choke
/// point. Trust is checked once (bundle-wide, rule 3's gate), and each
/// entry's lock status is resolved independently via
/// [`crate::resolve::workflow_lock_status`]'s per-entry building blocks, so
/// one entry's drift or unresolvable source never hides its siblings.
///
/// Untrusted/drifted entries ARE listed (with `trusted: false` and/or a
/// non-`"matches"` `lock_status`) — nothing here makes such an entry
/// runnable; `run` still re-gates independently through the choke point.
pub fn list(manifest_dir: Option<&Path>, args: &crate::cli::WorkflowListArgs) -> Result<()> {
    let rows = collect_workflow_list_rows(manifest_dir)?;
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&workflow_list_json(&rows))?
        );
    } else {
        print_workflow_list_table(&rows);
    }
    Ok(())
}

/// The row-gathering half of `list` (the testable seam): every declared
/// `[workflows.*]` entry, admission state annotated, never gated on it.
fn collect_workflow_list_rows(manifest_dir: Option<&Path>) -> Result<Vec<WorkflowListRow>> {
    let ctx = super::load(manifest_dir)?;
    let base = crate::manifest::project_root_of(&ctx.dir);
    let machine_policy = crate::machine_policy::load()?;
    let lock = crate::lock::Lock::load(&ctx.dir)?;
    let store = crate::store::Store::default_store();

    // Trust is a bundle-wide grant (rule 3), not per-workflow — one check
    // covers every declared entry. `Changed` (trusted, then edited) is NOT
    // currently trusted for running, same as `Untrusted`.
    let trusted = matches!(
        crate::trust::check(&base),
        crate::trust::TrustState::Trusted
    );

    let mut rows = Vec::new();
    for (name, wf) in &ctx.loaded.manifest.workflows {
        let resolved = crate::resolve::resolve_workflow_entry(
            name,
            wf,
            &ctx.dir,
            &store,
            crate::resolve::ResolveMode::NoFetch,
        );
        let (roles, checksum, lock_status) = match &resolved {
            Ok(r) => {
                let status = crate::resolve::classify_workflow(
                    name,
                    &r.checksum,
                    &r.roles,
                    r.rev.as_deref(),
                    &lock,
                );
                (
                    r.roles.clone(),
                    Some(r.checksum.clone()),
                    lock_status_slug(&status),
                )
            }
            // Unresolvable (offline git, missing/symlinked path, sourceless):
            // never a failure for a read path — state it and move on, the
            // declared roles are the only thing left to report.
            Err(crate::resolve::WorkflowResolveError::NotAvailableOffline { .. }) => {
                (wf.roles_sorted_unique(), None, "unavailable")
            }
            Err(_) => (wf.roles_sorted_unique(), None, "resolve_failed"),
        };

        // Effective ceilings: same min(request, machine cap) rule as
        // `normalized_workflows` (rule 2) — a request can only narrow the
        // machine cap, never raise it. Computed independently of admission:
        // an untrusted/drifted entry still has a well-defined effective
        // ceiling, it just can't run yet.
        let requested_agents = wf
            .max_agents
            .unwrap_or(crate::workflows::DEFAULT_MAX_AGENTS);
        let max_agents = machine_policy
            .workflows
            .max_agents
            .map_or(requested_agents, |cap| requested_agents.min(cap));
        let requested_wall = wf
            .max_wall_seconds
            .unwrap_or(crate::workflows::DEFAULT_MAX_WALL_SECONDS);
        let max_wall_seconds = machine_policy
            .workflows
            .max_wall_seconds
            .map_or(requested_wall, |cap| requested_wall.min(cap));

        rows.push(WorkflowListRow {
            name: name.clone(),
            declared: true,
            trusted,
            lock_status,
            roles,
            max_agents,
            max_wall_seconds,
            checksum,
        });
    }
    Ok(rows)
}

/// The JSON shape for `workflow list --json`: `{"workflows": [...]}`.
/// Manifest content is untrusted input (rule 7) even when listing an
/// untrusted bundle by design — sanitized the same as any other
/// terminal-bound field, though JSON's own control-character escaping
/// already makes this belt and suspenders.
fn workflow_list_json(rows: &[WorkflowListRow]) -> serde_json::Value {
    let json_rows: Vec<serde_json::Value> = rows
        .iter()
        .map(|r| {
            serde_json::json!({
                "name": sanitize_line(&r.name),
                "declared": r.declared,
                "trusted": r.trusted,
                "lock_status": r.lock_status,
                "roles": r.roles.iter().map(|s| sanitize_line(s)).collect::<Vec<_>>(),
                "max_agents": r.max_agents,
                "max_wall_seconds": r.max_wall_seconds,
                "checksum": r.checksum,
            })
        })
        .collect();
    serde_json::json!({ "workflows": json_rows })
}

/// Map a per-entry [`crate::resolve::WorkflowLockStatus`] to the `list`
/// surface's three-way slug: the fine-grained drift REASON (checksum vs
/// roles vs rev) is deliberately collapsed to `"drifted"` here — `list` is a
/// status overview, and `agentstack lock`/`doctor` already report the
/// specific drift reason for a caller who needs it.
fn lock_status_slug(status: &crate::resolve::WorkflowLockStatus) -> &'static str {
    use crate::resolve::WorkflowLockStatus::*;
    match status {
        Matches => "matches",
        MissingLockEntry => "missing",
        ChecksumDrift { .. } | RolesDrift { .. } | RevDrift { .. } => "drifted",
        NotAvailableOffline { .. } => "unavailable",
        ResolveFailed { .. } => "resolve_failed",
    }
}

fn print_workflow_list_table(rows: &[WorkflowListRow]) {
    if rows.is_empty() {
        println!("(no workflows declared)");
        return;
    }
    println!(
        "{:<24} {:<8} {:<14} {:<8} {:<10} ROLES",
        "NAME", "TRUSTED", "LOCK", "AGENTS", "WALL(s)"
    );
    for r in rows {
        println!(
            "{:<24} {:<8} {:<14} {:<8} {:<10} {}",
            sanitize_line(&r.name),
            r.trusted,
            r.lock_status,
            r.max_agents,
            r.max_wall_seconds,
            r.roles
                .iter()
                .map(|s| sanitize_line(s))
                .collect::<Vec<_>>()
                .join(","),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_fs::prelude::*;

    /// The Stage C fixture: serialized env (AGENTSTACK_HOME + PATH), a fake
    /// `claude` harness on PATH whose behavior is driven by the prompt (the
    /// LAST argv element under the claude-code headless spec), and a fresh
    /// project tempdir.
    fn workflow_fixture(f: impl FnOnce(&assert_fs::TempDir, &assert_fs::TempDir)) {
        let _guard = crate::util::TEST_ENV_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let home = assert_fs::TempDir::new().unwrap();
        let proj = assert_fs::TempDir::new().unwrap();
        let bins = home.child("fakebin");
        bins.create_dir_all().unwrap();
        let fake = bins.child("claude");
        // Prompt-driven fake harness. `overlap <dir> <name> <peer>` is the
        // witness-8 rendezvous: record whether .mcp.json exists, mark our
        // start, then WAIT for the peer's start marker — genuine overlap or
        // a bounded-wait failure, never a flaky clock assertion.
        fake.write_str(concat!(
            "#!/bin/sh\n",
            "last=\"\"\n",
            "for a in \"$@\"; do last=\"$a\"; done\n",
            "case \"$last\" in\n",
            "  emit-json*) printf '%s' '{\"a\":1,\"b\":[1,2,3]}' ;;\n",
            "  sleep*) sleep 1.5; printf 'ok' ;;\n",
            "  long*) printf '%080d' 7 ;;\n",
            "  fail*) exit 3 ;;\n",
            "  overlap*)\n",
            "    set -- $last\n",
            "    dir=\"$2\"; name=\"$3\"; peer=\"$4\"\n",
            "    if [ -f .mcp.json ]; then echo yes > \"$dir/$name.mcp\"; else echo no > \"$dir/$name.mcp\"; fi\n",
            "    : > \"$dir/$name.start\"\n",
            "    i=0\n",
            "    while [ ! -f \"$dir/$peer.start\" ] && [ $i -lt 100 ]; do sleep 0.1; i=$((i+1)); done\n",
            "    [ -f \"$dir/$peer.start\" ] || exit 1\n",
            "    printf '%s' \"$name-ok\" ;;\n",
            "  *) printf 'ok' ;;\n",
            "esac\n",
        ))
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(fake.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let old_path = std::env::var_os("PATH");
        let new_path = std::env::join_paths(
            std::iter::once(bins.path().to_path_buf())
                .chain(old_path.iter().flat_map(std::env::split_paths)),
        )
        .unwrap();
        std::env::set_var("AGENTSTACK_HOME", home.path());
        std::env::set_var("PATH", &new_path);

        f(&home, &proj);

        std::env::remove_var("AGENTSTACK_HOME");
        match old_path {
            Some(p) => std::env::set_var("PATH", p),
            None => std::env::remove_var("PATH"),
        }
    }

    /// Write, pin, and trust a workflow project: manifest at the root, the
    /// script at ./workflows/main.js, workflow pins recorded, trust granted.
    fn pin_and_trust(proj: &assert_fs::TempDir, manifest_toml: &str, script: &str) {
        proj.child("workflows/main.js").write_str(script).unwrap();
        proj.child("agentstack.toml")
            .write_str(manifest_toml)
            .unwrap();
        let manifest: crate::manifest::Manifest = toml::from_str(manifest_toml).unwrap();
        let store = crate::store::Store::default_store();
        crate::commands::lock::record_workflow_pins(proj.path(), &manifest, &store).unwrap();
        crate::trust::trust(proj.path()).unwrap();
    }

    fn wf_args(name: &str, args_json: Option<&str>) -> crate::cli::WorkflowRunArgs {
        crate::cli::WorkflowRunArgs {
            name: name.to_string(),
            args_json: args_json.map(String::from),
            resume: None,
        }
    }

    /// The Stage F invocation shape: same name/args, resuming `run_id`.
    fn wf_resume(name: &str, args_json: Option<&str>, run_id: &str) -> crate::cli::WorkflowRunArgs {
        crate::cli::WorkflowRunArgs {
            name: name.to_string(),
            args_json: args_json.map(String::from),
            resume: Some(run_id.to_string()),
        }
    }

    /// Every GrantFrozen digest recorded under the isolated home, across all
    /// child runs.
    fn recorded_grant_digests(home: &assert_fs::TempDir) -> Vec<String> {
        let runs = home.path().join("runs");
        let mut digests = Vec::new();
        for entry in std::fs::read_dir(&runs).into_iter().flatten().flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            for event in crate::calllog::RunLog::read(&id) {
                if let crate::calllog::RunEvent::GrantFrozen { grant_digest, .. } = event {
                    digests.push(grant_digest);
                }
            }
        }
        digests
    }

    const SIMPLE_MANIFEST: &str = r#"
        version = 1
        [profiles.w]
        [workflows.t]
        path = "./workflows/main.js"
        roles = ["w"]
        "#;

    /// Witness 1 (normalization side): a script whose meta.roles names a role
    /// outside the manifest's admitted role set is refused after engine
    /// construction, before any spawn — the manifest is the authority.
    #[test]
    fn script_role_outside_manifest_set_is_refused() {
        workflow_fixture(|_home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w', 'ghost'] };\nreturn 1;",
            );
            let err = run_value(Some(proj.path()), &wf_args("t", None))
                .unwrap_err()
                .to_string();
            assert!(err.contains("ghost"), "{err}");
            assert!(err.contains("authority"), "{err}");
        });
    }

    /// F5 fidelity: a child emitting exact JSON on bounded stdout resolves
    /// the `agent()` promise to that exact text — byte-faithful, no courier,
    /// no trim, no parse.
    #[cfg(unix)]
    #[test]
    fn f5_child_stdout_resolves_verbatim() {
        workflow_fixture(|_home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 return await agent('emit-json', { role: 'w' });",
            );
            let value = run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            assert_eq!(
                value,
                serde_json::Value::String("{\"a\":1,\"b\":[1,2,3]}".to_string()),
                "the resolved value must be the child's exact stdout bytes"
            );
        });
    }

    /// Witness 8, re-run under the engine: two concurrent children in one
    /// project genuinely OVERLAP (each waits for the other's start marker —
    /// serial scheduling would dead-end the bounded wait and fail the test),
    /// the shared project `.mcp.json` stays untouched (absent) throughout,
    /// and the two children froze DISTINCT grant digests.
    #[cfg(unix)]
    #[test]
    fn concurrent_children_overlap_with_project_config_untouched() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const outs = await parallel([\n\
                   () => agent(`overlap ${args.dir} A B`, { role: 'w' }),\n\
                   () => agent(`overlap ${args.dir} B A`, { role: 'w' }),\n\
                 ]);\n\
                 return outs;",
            );
            let markers = proj.child("markers");
            markers.create_dir_all().unwrap();
            let args_json = serde_json::json!({ "dir": markers.path() }).to_string();

            let value = run_value(Some(proj.path()), &wf_args("t", Some(&args_json))).unwrap();
            assert_eq!(
                value,
                serde_json::json!(["A-ok", "B-ok"]),
                "both children must complete — overlap is load-bearing (a serial \
                 schedule dead-ends the rendezvous wait)"
            );

            // The shared project config was never touched — no park/swap, no
            // residue — and each child observed it absent at its own runtime.
            assert!(!proj.child(".mcp.json").path().exists());
            for name in ["A", "B"] {
                let seen =
                    std::fs::read_to_string(markers.path().join(format!("{name}.mcp"))).unwrap();
                assert_eq!(seen.trim(), "no", "child {name} saw a project .mcp.json");
            }

            // Two children, two DISTINCT frozen grants (per-child identity).
            let digests = recorded_grant_digests(home);
            assert_eq!(digests.len(), 2, "{digests:?}");
            assert_ne!(digests[0], digests[1]);
        });
    }

    /// Witness 9 + the per-child gate-refusal witness for the locked.rs
    /// extraction: the role profile FENCES the child's grant surface — a
    /// server whose executable pin has drifted refuses the child that
    /// includes it (recorded as a locked-verify gate refusal, surfacing as
    /// `null` to the script) while the child whose profile excludes it runs
    /// clean. The child path lost no gate in the extraction.
    #[cfg(unix)]
    #[test]
    fn role_profile_fences_child_grant_and_gates_still_refuse() {
        workflow_fixture(|home, proj| {
            use std::os::unix::fs::PermissionsExt;
            proj.child("bad.sh")
                .write_str("#!/bin/sh\necho v1\n")
                .unwrap();
            std::fs::set_permissions(
                proj.child("bad.sh").path(),
                std::fs::Permissions::from_mode(0o755),
            )
            .unwrap();
            let manifest_toml = r#"
                version = 1
                [servers.bad]
                type = "stdio"
                command = "./bad.sh"
                [profiles.fenced]
                servers = []
                [profiles.broken]
                servers = ["bad"]
                [workflows.t]
                path = "./workflows/main.js"
                roles = ["fenced", "broken"]
                "#;
            proj.child("workflows/main.js")
                .write_str(
                    "export const meta = { roles: ['fenced', 'broken'] };\n\
                     const ok = await agent('hi', { role: 'fenced' });\n\
                     const bad = await agent('hi', { role: 'broken' });\n\
                     return { ok: typeof ok === 'string' && ok.length > 0, bad };",
                )
                .unwrap();
            proj.child("agentstack.toml")
                .write_str(manifest_toml)
                .unwrap();
            let manifest: crate::manifest::Manifest = toml::from_str(manifest_toml).unwrap();
            let store = crate::store::Store::default_store();
            crate::commands::lock::record_workflow_pins(proj.path(), &manifest, &store).unwrap();
            // Pin the executable surface, then trust the final lock bytes.
            let mut lock = agentstack_core::lock::Lock::load(proj.path()).unwrap();
            for pin in crate::executable::derive_executable_pins(
                proj.path(),
                "bad",
                manifest.servers.get("bad").unwrap(),
            )
            .unwrap()
            {
                lock.upsert_executable(pin);
            }
            lock.save(proj.path()).unwrap();
            crate::trust::trust(proj.path()).unwrap();

            // Tamper the pinned executable AFTER trust: trust (manifest+lock)
            // still passes; only per-child strict verification can catch it.
            proj.child("bad.sh")
                .write_str("#!/bin/sh\necho TAMPERED\n")
                .unwrap();

            let value = run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            assert_eq!(
                value,
                serde_json::json!({ "ok": true, "bad": null }),
                "the fenced child (grant ≤ its profile's empty set) runs clean; \
                 the child including the drifted server fails closed"
            );

            // The refusal is RECORDED at the same gate an interactive locked
            // run refuses at — the extraction dropped no gate for children.
            let refused_at_verify = std::fs::read_dir(home.path().join("runs"))
                .into_iter()
                .flatten()
                .flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .flat_map(|id| crate::calllog::RunLog::read(&id))
                .any(|event| {
                    matches!(
                        event,
                        crate::calllog::RunEvent::GateDecision { gate, passed: false, .. }
                            if gate == "locked-verify"
                    )
                });
            assert!(
                refused_at_verify,
                "the child's gate refusal must be recorded"
            );
        });
    }

    /// Per-child panic containment: a child whose spawner panics (test-seam
    /// probe) fails ITS step closed — the script sees `null` — while its
    /// sibling children complete and the workflow itself reaches Done. The
    /// uncontained alternative is a worker panic re-raised at the
    /// `thread::scope` join, aborting the whole run.
    #[cfg(unix)]
    #[test]
    fn panicking_child_fails_its_step_not_the_workflow() {
        workflow_fixture(|_home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const outs = await parallel([\n\
                   () => agent('emit-json', { role: 'w' }),\n\
                   () => agent('__agentstack_test_child_panic__', { role: 'w' }),\n\
                   () => agent('emit-json', { role: 'w' }),\n\
                 ]);\n\
                 return outs;",
            );
            // Silence the default hook's noise for the expected worker panic.
            let previous = std::panic::take_hook();
            std::panic::set_hook(Box::new(|_| {}));
            let value = run_value(Some(proj.path()), &wf_args("t", None));
            std::panic::set_hook(previous);

            assert_eq!(
                value.unwrap(),
                serde_json::json!(["{\"a\":1,\"b\":[1,2,3]}", null, "{\"a\":1,\"b\":[1,2,3]}"]),
                "the panicking child resolves to null; siblings and the run survive"
            );
        });
    }

    /// Stage D ceiling-chain witness (clamp side): a script `meta` requesting
    /// MORE than admission granted is CLAMPED to the admitted values — the
    /// grant never widens — observed through `budget`'s script-visible view
    /// (admitted here = the built-in defaults 25/1800: no manifest request,
    /// no machine cap in the isolated home).
    #[test]
    fn script_meta_requesting_more_is_clamped() {
        workflow_fixture(|_home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'], maxAgents: 99999, maxWallSeconds: 99999 };\n\
                 return [budget.maxAgents, budget.maxWallSeconds];",
            );
            let value = run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            assert_eq!(
                value,
                serde_json::json!([
                    crate::workflows::DEFAULT_MAX_AGENTS,
                    crate::workflows::DEFAULT_MAX_WALL_SECONDS
                ]),
                "a script request above the admitted ceilings is clamped, never widens"
            );
        });
    }

    /// Stage D ceiling-chain witness (narrow side) + witness 3 end-to-end: a
    /// script `meta` requesting LESS narrows both `budget`'s view AND the
    /// enforcement — the second call is refused at the ceiling, fails closed
    /// (the script catches it and adapts), and the run still completes.
    #[cfg(unix)]
    #[test]
    fn script_meta_narrows_enforcement_and_budget() {
        workflow_fixture(|_home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'], maxAgents: 1 };\n\
                 const a = await agent('emit-json', { role: 'w' });\n\
                 let denied = false;\n\
                 try { await agent('emit-json', { role: 'w' }); } catch (e) { denied = true; }\n\
                 return { ma: budget.maxAgents, spawned: budget.spawned(), denied,\n\
                          got: typeof a === 'string' && a.length > 0 };",
            );
            let value = run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            assert_eq!(
                value,
                serde_json::json!({ "ma": 1, "spawned": 1, "denied": true, "got": true }),
                "the narrowed ceiling is enforced per call; the refused call fails closed \
                 while the run completes"
            );
        });
    }

    /// Stage D cooperative wall deadline: once the effective (script-narrowed)
    /// wall ceiling passes, the NEXT batch is refused and the workflow fails
    /// cleanly in-band — through the CLI's normal error path, without the
    /// watchdog (armed at ceiling+grace) firing. The Stage C watchdog stays
    /// the stall backstop above this path.
    #[cfg(unix)]
    #[test]
    fn cooperative_wall_deadline_refuses_next_batch_in_band() {
        workflow_fixture(|_home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'], maxWallSeconds: 1 };\n\
                 const a = await agent('sleep', { role: 'w' });\n\
                 const b = await agent('sleep', { role: 'w' });\n\
                 return [a, b];",
            );
            // The first batch's child sleeps ~1.5s, past the 1s effective
            // ceiling; the second batch must be refused cooperatively. If the
            // watchdog fired instead, the PROCESS would exit 124 and the test
            // harness would report a crash, not an Err.
            let err = run_value(Some(proj.path()), &wf_args("t", None))
                .unwrap_err()
                .to_string();
            assert!(err.contains("wall-clock ceiling"), "{err}");
            assert!(err.contains("in-band"), "{err}");
        });
    }

    /// The single workflow-envelope run (`w-…`) recorded under the isolated
    /// home, with its events.
    fn workflow_run_events(home: &assert_fs::TempDir) -> (String, Vec<RunEvent>) {
        let runs = home.path().join("runs");
        let mut ids: Vec<String> = std::fs::read_dir(&runs)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|id| id.starts_with("w-"))
            .collect();
        assert_eq!(ids.len(), 1, "expected exactly one workflow run: {ids:?}");
        let id = ids.remove(0);
        let events = RunLog::read(&id);
        (id, events)
    }

    /// Witness 3, recorded half (Stage E): an exhausting run's workflow log
    /// carries `exhausted: true` on the terminal event even though the
    /// outcome is `done` (Stage D's non-fatal semantics), and
    /// `workflow report` states the exhaustion from the record.
    #[cfg(unix)]
    #[test]
    fn exhaustion_is_recorded_and_reported() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'], maxAgents: 1 };\n\
                 const a = await agent('emit-json', { role: 'w' });\n\
                 let denied = false;\n\
                 try { await agent('emit-json', { role: 'w' }); } catch (e) { denied = true; }\n\
                 return denied;",
            );
            let value = run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            assert_eq!(value, serde_json::json!(true));

            let (id, events) = workflow_run_events(home);
            let terminal = events
                .iter()
                .rev()
                .find_map(|e| match e {
                    RunEvent::WorkflowCompleted {
                        outcome, exhausted, ..
                    } => Some((outcome.clone(), *exhausted)),
                    _ => None,
                })
                .expect("terminal event recorded");
            assert_eq!(terminal, ("done".to_string(), true));

            let report = render_workflow_report(&id).unwrap();
            assert!(report.contains("EXHAUSTED"), "{report}");
        });
    }

    /// Join-table witness + report honesty (Stage E): the report resolves
    /// each step's child grant digest and outcome from the CHILD's own log
    /// (`StepSpawned` structurally carries neither), and prints the exported
    /// `POSTURE_LABEL` verbatim — asserted against the const, not a copy.
    #[cfg(unix)]
    #[test]
    fn report_joins_child_evidence_and_prints_posture_verbatim() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const outs = await parallel([\n\
                   () => agent('emit-json', { role: 'w', label: 'map:a' }),\n\
                   () => agent('emit-json', { role: 'w', label: 'map:b' }),\n\
                 ]);\n\
                 return outs;",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();

            let (id, events) = workflow_run_events(home);
            let child_ids: Vec<String> = events
                .iter()
                .filter_map(|e| match e {
                    RunEvent::StepSpawned { child_run_id, .. } => Some(child_run_id.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(child_ids.len(), 2);

            let report = render_workflow_report(&id).unwrap();
            for child in &child_ids {
                let grant = RunLog::read(child)
                    .into_iter()
                    .find_map(|e| match e {
                        RunEvent::GrantFrozen { grant_digest, .. } => Some(grant_digest),
                        _ => None,
                    })
                    .expect("child froze its grant");
                assert!(
                    report.contains(&grant),
                    "the child's grant digest must be JOINED into the report"
                );
                assert!(report.contains(child.as_str()));
            }
            assert!(report.contains("outcome=completed (exit 0)"), "{report}");
            assert!(
                report.contains(agentstack_workflow::POSTURE_LABEL),
                "the honesty block is the exported const, verbatim"
            );
        });
    }

    /// `workflow report --json`: the SAME join as the text render, shaped
    /// for scripting — top-level identity/ceilings/outcome, and each step's
    /// grant digest joined from the child's own log, with the recorded exit
    /// code and a completed state.
    #[cfg(unix)]
    #[test]
    fn report_json_matches_recorded_join_shape() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const outs = await parallel([\n\
                   () => agent('emit-json', { role: 'w', label: 'map:a' }),\n\
                   () => agent('emit-json', { role: 'w', label: 'map:b' }),\n\
                 ]);\n\
                 return outs;",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();

            let (id, events) = workflow_run_events(home);
            let child_ids: Vec<String> = events
                .iter()
                .filter_map(|e| match e {
                    RunEvent::StepSpawned { child_run_id, .. } => Some(child_run_id.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(child_ids.len(), 2);

            let json = render_workflow_report_json(&id).unwrap();
            let value: serde_json::Value = serde_json::from_str(&json).unwrap();

            assert_eq!(value["run"], id);
            assert_eq!(value["workflow"], "t");
            assert!(value["workflow_digest"].is_string());
            assert!(value["grant_digest"].is_string());
            assert!(value["args_digest"].is_string());
            assert_eq!(value["posture"], serde_json::Value::Null);
            assert_eq!(value["outcome"], "completed");
            assert_eq!(value["exhausted"], false);
            assert!(value["duration_ms"].is_u64());
            assert_eq!(value["max_agents"], 25); // SIMPLE_MANIFEST requests none: the built-in default
            assert_eq!(value["max_wall_seconds"], 1800);

            let steps = value["steps"].as_array().unwrap();
            assert_eq!(steps.len(), 2);
            for step in steps {
                let child = step["child_run_id"].as_str().unwrap();
                let grant = RunLog::read(child)
                    .into_iter()
                    .find_map(|e| match e {
                        RunEvent::GrantFrozen { grant_digest, .. } => Some(grant_digest),
                        _ => None,
                    })
                    .expect("child froze its grant");
                assert_eq!(step["grant_digest"], grant);
                assert_eq!(step["state"], "completed");
                assert_eq!(step["outcome"], "completed");
                assert_eq!(step["exit_code"], 0);
                assert_eq!(step["role"], "w");
                assert_eq!(step["serial"], false);
                assert_eq!(step["tool_calls"], 0);
                assert!(step["label"].as_str().unwrap().starts_with("map:"));
            }
        });
    }

    /// Taint witness, both directions (Stage E, §11 ruling 3): a prompt that
    /// embeds a prior step's (≥ floor) result is marked with the source
    /// step; an independent prompt is not marked.
    #[cfg(unix)]
    #[test]
    fn taint_marks_embedding_prompts_only() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const seed = await agent('long', { role: 'w' });\n\
                 const used = await agent('verify this: ' + seed, { role: 'w' });\n\
                 const indep = await agent('emit-json', { role: 'w' });\n\
                 return [seed.length, used, indep];",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();

            let (_id, events) = workflow_run_events(home);
            let taints: HashMap<u64, Vec<u64>> = events
                .iter()
                .filter_map(|e| match e {
                    RunEvent::StepSpawned { step, taint, .. } => Some((*step, taint.clone())),
                    _ => None,
                })
                .collect();
            assert_eq!(taints.get(&0), Some(&vec![]));
            assert_eq!(
                taints.get(&1),
                Some(&vec![0]),
                "the embedding prompt is marked with its source step"
            );
            assert_eq!(
                taints.get(&2),
                Some(&vec![]),
                "the independent prompt is unmarked"
            );
        });
    }

    /// Serial-fallback recorded (Stage E task 4), via the CONSTRUCTED path —
    /// no shipped headless adapter reaches serial (claude-code and codex
    /// both inject), so a hand-built non-injectable `RoleBinding` drives the
    /// same pre-spawn append + execute seam the drive loop uses: the event
    /// records `serial: true` and the child still completes over the
    /// config-swap path.
    #[cfg(unix)]
    #[test]
    fn serial_fallback_is_recorded_on_the_spawn_event() {
        workflow_fixture(|_home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\nreturn 1;",
            );
            let run_id = crate::runs::gen_workflow_id();
            let wev = WorkflowEvidence {
                log: RunLog::create(&run_id).unwrap(),
                run_id: run_id.clone(),
                started: Instant::now(),
            };
            let mut bindings = HashMap::new();
            bindings.insert(
                "w".to_string(),
                RoleBinding {
                    harness: "claude-code".into(),
                    injectable: false,
                    codex_residual: false,
                },
            );
            let request = SpawnRequest {
                id: 0,
                role: "w".into(),
                prompt: "hello".into(),
                opts: serde_json::Value::Null,
            };
            let child_id = record_step_spawned(&wev, &request, bindings.get("w"), &[]).unwrap();
            let mut child_ids = HashMap::new();
            child_ids.insert(0, child_id);
            let pids: crate::runs::ChildPids = Arc::new(Mutex::new(HashSet::new()));

            let steps = execute_batch(
                Some(proj.path()),
                &bindings,
                &[request],
                4,
                &pids,
                &child_ids,
            );
            assert!(matches!(steps[0].result.output, StepOutput::Completed(_)));
            let recorded = RunLog::read(&run_id).into_iter().find_map(|e| match e {
                RunEvent::StepSpawned { serial, .. } => Some(serial),
                _ => None,
            });
            assert_eq!(
                recorded,
                Some(true),
                "the config-swap path is recorded evidence, not stderr-only"
            );
        });
    }

    /// Gate 2 (Stage E): a workflow-log creation failure refuses the run
    /// BEFORE any child spawns — nothing trusted runs unobserved.
    #[cfg(unix)]
    #[test]
    fn recording_failure_refuses_before_any_child() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 return await agent('emit-json', { role: 'w' });",
            );
            // Make the runs root a regular FILE so every RunLog::create fails.
            let runs = home.path().join("runs");
            let _ = std::fs::remove_dir_all(&runs);
            std::fs::write(&runs, b"not a dir").unwrap();

            let err = run_value(Some(proj.path()), &wf_args("t", None))
                .unwrap_err()
                .to_string();
            assert!(err.contains("refusing to run unobserved"), "{err}");
        });
    }

    /// CLI-edge containment: a panic out of engine construction is caught at
    /// the CLI boundary and becomes a clean refusal (belt and suspenders on
    /// top of the crate's own AL5 containment).
    #[test]
    fn cli_edge_contains_engine_panics() {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(|_| {}));
        let out: Result<()> = contained(|| panic!("intentional panic through the CLI edge"));
        std::panic::set_hook(previous);
        let err = out.unwrap_err().to_string();
        assert!(err.contains("refusing the run"), "{err}");
    }

    // ───────────────────────── Stage F resume witnesses ─────────────────────────

    /// Rewrite a workflow run's journal to exactly `events` — the test
    /// scalpel standing in for a crash/kill (the journal is just a file).
    fn rewrite_journal(home: &assert_fs::TempDir, run_id: &str, events: &[RunEvent]) {
        let path = home.path().join("runs").join(run_id).join("events.jsonl");
        let text: String = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap() + "\n")
            .collect();
        std::fs::write(path, text).unwrap();
    }

    /// Journal prefix ending at the first event matching `until` (inclusive).
    fn journal_prefix(events: &[RunEvent], until: impl Fn(&RunEvent) -> bool) -> Vec<RunEvent> {
        let idx = events.iter().position(until).expect("cut point exists");
        events[..=idx].to_vec()
    }

    /// Every child run id (`r-…`) with a run dir under the isolated home —
    /// the spawn-absence evidence: replayed steps create no new dirs.
    fn child_run_dirs(home: &assert_fs::TempDir) -> HashSet<String> {
        std::fs::read_dir(home.path().join("runs"))
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|id| id.starts_with("r-"))
            .collect()
    }

    fn is_completed(step: u64) -> impl Fn(&RunEvent) -> bool {
        move |e| matches!(e, RunEvent::StepCompleted { step: s, .. } if *s == step)
    }

    /// The core Stage F witness: a journal cut mid-run resumes — the
    /// journaled step spawns NO child, the live tail runs, the final value
    /// equals the uninterrupted run's, and the log carries the marker,
    /// the replay annotations, and a single `done` terminal.
    #[cfg(unix)]
    #[test]
    fn resume_replays_journal_and_completes_identically() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const a = await agent('emit-json', { role: 'w' });\n\
                 const b = await agent('long', { role: 'w' });\n\
                 return [a, b];",
            );
            let uninterrupted = run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);

            // Simulate the crash: keep everything through step 0's terminal.
            rewrite_journal(home, &id, &journal_prefix(&events, is_completed(0)));
            let before = child_run_dirs(home);

            let value = run_value(Some(proj.path()), &wf_resume("t", None, &id)).unwrap();
            assert_eq!(
                value, uninterrupted,
                "the resumed run's final value must equal the uninterrupted run's"
            );
            // Step 0 replayed (no child spawned); only step 1 ran live.
            let new: Vec<String> = child_run_dirs(home).difference(&before).cloned().collect();
            assert_eq!(new.len(), 1, "exactly one live child: {new:?}");

            let resumed_events = RunLog::read(&id);
            let marker = resumed_events.iter().find_map(|e| match e {
                RunEvent::WorkflowResumed { replayed_steps, .. } => Some(*replayed_steps),
                _ => None,
            });
            assert_eq!(marker, Some(1), "the marker records the replayed count");
            let outcomes: Vec<String> = resumed_events
                .iter()
                .filter_map(|e| match e {
                    RunEvent::WorkflowCompleted { outcome, .. } => Some(outcome.clone()),
                    _ => None,
                })
                .collect();
            assert_eq!(outcomes, ["done"], "one terminal, the resumed session's");

            let report = render_workflow_report(&id).unwrap();
            assert!(report.contains("Resumed 1 time(s)"), "{report}");
            assert!(report.contains("[replayed on resume]"), "{report}");
            assert!(report.contains("[live, resumed session]"), "{report}");
        });
    }

    /// Never-recompute (§3.1 / R1): a journaled `StepFailed` replays as
    /// `null` with no respawn — a failed step is a re-run decision for a
    /// human, never a resume semantic.
    #[cfg(unix)]
    #[test]
    fn resume_replays_failed_step_as_null_without_respawn() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const a = await agent('fail', { role: 'w' });\n\
                 const b = await agent('emit-json', { role: 'w' });\n\
                 return [a, b];",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);
            rewrite_journal(
                home,
                &id,
                &journal_prefix(&events, |e| {
                    matches!(e, RunEvent::StepFailed { step: 0, .. })
                }),
            );
            let before = child_run_dirs(home);

            let value = run_value(Some(proj.path()), &wf_resume("t", None, &id)).unwrap();
            assert_eq!(
                value,
                serde_json::json!([null, "{\"a\":1,\"b\":[1,2,3]}"]),
                "the journaled failure replays as null"
            );
            let new = child_run_dirs(home).len() - before.len();
            assert_eq!(
                new, 1,
                "the failed step must not respawn; only the tail is live"
            );
        });
    }

    /// Resumability refusals: a `done` run has nothing to resume, and a
    /// recorded deterministic failure would replay identically — both refuse
    /// with the reason named.
    #[cfg(unix)]
    #[test]
    fn resume_refuses_done_and_failed_terminals() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 return await agent('emit-json', { role: 'w' });",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, _) = workflow_run_events(home);
            let err = run_value(Some(proj.path()), &wf_resume("t", None, &id))
                .unwrap_err()
                .to_string();
            assert!(err.contains("completed"), "{err}");
            assert!(err.contains("nothing to resume"), "{err}");

            // A deterministic failure: swap the terminal for a failed:* one.
            let mut events = RunLog::read(&id);
            if let Some(RunEvent::WorkflowCompleted { outcome, .. }) = events.last_mut() {
                *outcome = "failed:runtime_error".to_string();
            } else {
                panic!("expected a terminal event");
            }
            rewrite_journal(home, &id, &events);
            let err = run_value(Some(proj.path()), &wf_resume("t", None, &id))
                .unwrap_err()
                .to_string();
            assert!(err.contains("deterministic failure"), "{err}");
            assert!(err.contains("re-run fresh"), "{err}");
        });
    }

    /// Divergence refusal, SCRIPT dimension: a byte-changed (re-pinned,
    /// re-trusted — admission itself passes) script refuses resume naming
    /// the script identity.
    #[cfg(unix)]
    #[test]
    fn resume_refuses_script_divergence() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 return await agent('emit-json', { role: 'w' });",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);
            rewrite_journal(home, &id, &journal_prefix(&events, is_completed(0)));

            // One changed byte, re-pinned and re-trusted: fresh admission
            // passes; the resume identity gate must still refuse.
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 return await agent('emit-json2', { role: 'w' });",
            );
            let err = run_value(Some(proj.path()), &wf_resume("t", None, &id))
                .unwrap_err()
                .to_string();
            assert!(err.contains("SCRIPT identity"), "{err}");
        });
    }

    /// Divergence refusal, ARGS dimension: --args-json must be byte-identical.
    #[cfg(unix)]
    #[test]
    fn resume_refuses_args_divergence() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 return await agent('emit-json', { role: 'w' });",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);
            rewrite_journal(home, &id, &journal_prefix(&events, is_completed(0)));

            let err = run_value(Some(proj.path()), &wf_resume("t", Some("{}"), &id))
                .unwrap_err()
                .to_string();
            assert!(err.contains("ARGS identity"), "{err}");
        });
    }

    /// Divergence refusal, GRANT dimension (ceilings): a manifest ceiling
    /// change under identical script bytes refuses with the ceiling hint.
    #[cfg(unix)]
    #[test]
    fn resume_refuses_ceiling_divergence() {
        workflow_fixture(|home, proj| {
            let script = "export const meta = { roles: ['w'] };\n\
                 return await agent('emit-json', { role: 'w' });";
            pin_and_trust(proj, SIMPLE_MANIFEST, script);
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);
            rewrite_journal(home, &id, &journal_prefix(&events, is_completed(0)));

            pin_and_trust(
                proj,
                r#"
                version = 1
                [profiles.w]
                [workflows.t]
                path = "./workflows/main.js"
                roles = ["w"]
                max_agents = 5
                "#,
                script,
            );
            let err = run_value(Some(proj.path()), &wf_resume("t", None, &id))
                .unwrap_err()
                .to_string();
            assert!(err.contains("GRANT diverged"), "{err}");
            assert!(err.contains("ceilings moved"), "{err}");
        });
    }

    /// Divergence refusal, GRANT dimension (roles): a widened role set under
    /// identical ceilings refuses with the role-set hint.
    #[cfg(unix)]
    #[test]
    fn resume_refuses_role_set_divergence() {
        workflow_fixture(|home, proj| {
            let script = "export const meta = { roles: ['w'] };\n\
                 return await agent('emit-json', { role: 'w' });";
            pin_and_trust(proj, SIMPLE_MANIFEST, script);
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);
            rewrite_journal(home, &id, &journal_prefix(&events, is_completed(0)));

            pin_and_trust(
                proj,
                r#"
                version = 1
                [profiles.w]
                [profiles.v]
                [workflows.t]
                path = "./workflows/main.js"
                roles = ["w", "v"]
                "#,
                script,
            );
            let err = run_value(Some(proj.path()), &wf_resume("t", None, &id))
                .unwrap_err()
                .to_string();
            assert!(err.contains("GRANT diverged"), "{err}");
            assert!(err.contains("ROLE SET moved"), "{err}");
        });
    }

    /// Per-step alignment: a doctored mid-journal `StepSpawned` (edited
    /// request digest) refuses via the misalignment check — feeding results
    /// into a misaligned request would be corruption, not recovery.
    #[cfg(unix)]
    #[test]
    fn resume_refuses_doctored_request_digest() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const a = await agent('emit-json', { role: 'w' });\n\
                 const b = await agent('long', { role: 'w' });\n\
                 return [a, b];",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);
            let mut cut = journal_prefix(&events, is_completed(0));
            for event in &mut cut {
                if let RunEvent::StepSpawned { request_digest, .. } = event {
                    *request_digest = "doctored".to_string();
                }
            }
            rewrite_journal(home, &id, &cut);

            let before = child_run_dirs(home);
            let err = run_value(Some(proj.path()), &wf_resume("t", None, &id))
                .unwrap_err()
                .to_string();
            assert!(err.contains("step #0 diverged from the journal"), "{err}");
            assert_eq!(
                child_run_dirs(home),
                before,
                "a refused resume must spawn nothing"
            );
        });
    }

    /// Tamper-evident feed: an edited stdout artifact no longer matches the
    /// child's recorded `HeadlessOutput.sha256` and refuses resume, naming
    /// the step — the evidence digest does double duty as the replay anchor.
    #[cfg(unix)]
    #[test]
    fn resume_refuses_tampered_result_artifact() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const a = await agent('emit-json', { role: 'w' });\n\
                 const b = await agent('long', { role: 'w' });\n\
                 return [a, b];",
            );
            run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);
            let cut = journal_prefix(&events, is_completed(0));
            let child0 = cut
                .iter()
                .find_map(|e| match e {
                    RunEvent::StepSpawned { child_run_id, .. } => Some(child_run_id.clone()),
                    _ => None,
                })
                .unwrap();
            rewrite_journal(home, &id, &cut);

            // Tamper the persisted result bytes.
            let artifact = home.path().join("runs").join(&child0).join("stdout");
            let mut bytes = std::fs::read(&artifact).unwrap();
            bytes.extend_from_slice(b" tampered");
            std::fs::write(&artifact, &bytes).unwrap();

            let err = run_value(Some(proj.path()), &wf_resume("t", None, &id))
                .unwrap_err()
                .to_string();
            assert!(err.contains("step #0"), "{err}");
            assert!(
                err.contains("does not match the recorded output digest"),
                "{err}"
            );
        });
    }

    /// Ceiling continuity (rule 2): the engine re-counts replayed spawns
    /// against the SAME effective ceiling, so replayed + live together stay
    /// bounded by the original grant — resume never widens.
    #[cfg(unix)]
    #[test]
    fn resume_never_widens_the_agent_ceiling() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'], maxAgents: 2 };\n\
                 const a = await agent('emit-json', { role: 'w' });\n\
                 const b = await agent('long', { role: 'w' });\n\
                 let denied = false;\n\
                 try { await agent('emit-json', { role: 'w' }); } catch (e) { denied = true; }\n\
                 return denied;",
            );
            let value = run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            assert_eq!(value, serde_json::json!(true));
            let (id, events) = workflow_run_events(home);
            rewrite_journal(home, &id, &journal_prefix(&events, is_completed(0)));
            let before = child_run_dirs(home);

            let value = run_value(Some(proj.path()), &wf_resume("t", None, &id)).unwrap();
            assert_eq!(
                value,
                serde_json::json!(true),
                "the third call is refused on resume too — replayed spawns count"
            );
            let new = child_run_dirs(home).len() - before.len();
            assert_eq!(
                new, 1,
                "one live child (step 1); step 0 replayed, step 2 refused"
            );
        });
    }

    /// Fresh wall clock (the Stage D promise): a `wall_deadline` run resumes
    /// and completes — the resumed session's deadline arms at the FULL
    /// effective ceiling, not the remainder (a remainder clock would refuse
    /// the live tail's batch immediately).
    #[cfg(unix)]
    #[test]
    fn resume_arms_a_fresh_wall_clock() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'], maxWallSeconds: 1 };\n\
                 const a = await agent('sleep', { role: 'w' });\n\
                 const b = await agent('sleep', { role: 'w' });\n\
                 return [a, b];",
            );
            // The original run burns the whole ceiling and dies at the wall.
            let err = run_value(Some(proj.path()), &wf_args("t", None))
                .unwrap_err()
                .to_string();
            assert!(err.contains("wall-clock ceiling"), "{err}");
            let (id, _) = workflow_run_events(home);

            // wall_deadline is a resumable interruption; the replay is
            // instant, and the live tail (1.5s child) fits the fresh 1s
            // deadline check at its batch boundary.
            let value = run_value(Some(proj.path()), &wf_resume("t", None, &id)).unwrap();
            assert_eq!(value, serde_json::json!(["ok", "ok"]));

            // The multi-terminal log renders honestly: the resumed session's
            // `done` is the outcome, the superseded `wall_deadline` stays
            // stated as an earlier terminal.
            let report = render_workflow_report(&id).unwrap();
            assert!(report.contains("Outcome: done"), "{report}");
            assert!(
                report.contains("earlier terminal: wall_deadline"),
                "{report}"
            );
        });
    }

    /// Mid-batch straddle (per-member granularity): a batch with both spawns
    /// journaled but only one terminal replays the terminal-bearing member
    /// and re-executes the other live — the at-least-once boundary, recorded
    /// as a fresh post-marker spawn and rendered honestly.
    #[cfg(unix)]
    #[test]
    fn resume_straddles_a_half_journaled_batch() {
        workflow_fixture(|home, proj| {
            pin_and_trust(
                proj,
                SIMPLE_MANIFEST,
                "export const meta = { roles: ['w'] };\n\
                 const outs = await parallel([\n\
                   () => agent('emit-json', { role: 'w' }),\n\
                   () => agent('long', { role: 'w' }),\n\
                 ]);\n\
                 return outs;",
            );
            let uninterrupted = run_value(Some(proj.path()), &wf_args("t", None)).unwrap();
            let (id, events) = workflow_run_events(home);
            // Keep both spawns but only step 0's terminal: the crash landed
            // mid-batch (or mid-append-loop).
            let cut: Vec<RunEvent> = events
                .iter()
                .filter(|e| {
                    !matches!(
                        e,
                        RunEvent::StepCompleted { step: 1, .. }
                            | RunEvent::WorkflowCompleted { .. }
                    )
                })
                .cloned()
                .collect();
            rewrite_journal(home, &id, &cut);
            let before = child_run_dirs(home);

            let value = run_value(Some(proj.path()), &wf_resume("t", None, &id)).unwrap();
            assert_eq!(value, uninterrupted);
            let new = child_run_dirs(home).len() - before.len();
            assert_eq!(new, 1, "only the terminal-less member re-executes");

            // The re-execution is a fresh post-marker spawn; the report says
            // who replayed, who was superseded, and who ran live.
            let resumed_events = RunLog::read(&id);
            let marker_idx = resumed_events
                .iter()
                .position(|e| matches!(e, RunEvent::WorkflowResumed { .. }))
                .expect("marker recorded");
            let respawn_after_marker = resumed_events.iter().enumerate().any(|(idx, e)| {
                idx > marker_idx && matches!(e, RunEvent::StepSpawned { step: 1, .. })
            });
            assert!(
                respawn_after_marker,
                "the live re-execution follows the marker"
            );

            let report = render_workflow_report(&id).unwrap();
            assert!(report.contains("[replayed on resume]"), "{report}");
            assert!(
                report.contains("[superseded — re-executed live on resume]"),
                "{report}"
            );
            assert!(report.contains("[live, resumed session]"), "{report}");
        });
    }

    /// Progress sanitization witness: a `log()` line carrying terminal
    /// escapes reaches the terminal with the SCRIPT-controlled content
    /// stripped. (The line may still carry our own styling escapes around
    /// the bullet — launcher-authored, not script-reachable — so the
    /// assertion targets the hostile payload, not ANSI in general.)
    #[test]
    fn progress_lines_are_sanitized() {
        let line = format_progress(&Progress::Log("evil\u{1b}[2J\u{7}payload".into()));
        assert!(!line.contains("[2J"), "script CSI survived: {line:?}");
        assert!(!line.contains('\u{7}'), "script bell survived: {line:?}");
        assert!(line.contains("evilpayload"), "{line:?}");
    }

    /// `workflow list --json`: a pinned-but-not-yet-trusted entry is STILL
    /// listed (`trusted: false`) rather than refused — the whole point of
    /// this surface versus `run`'s `normalized_workflows` choke point, which
    /// bails the entire call on the first untrusted entry. Trusting the same
    /// bundle flips `trusted` to `true` with the lock status unchanged.
    #[test]
    fn list_json_surfaces_untrusted_and_trusted_states() {
        workflow_fixture(|_home, proj| {
            proj.child("workflows/main.js")
                .write_str("export const meta = { roles: ['w'] };\nreturn 1;")
                .unwrap();
            proj.child("agentstack.toml")
                .write_str(SIMPLE_MANIFEST)
                .unwrap();
            let manifest: crate::manifest::Manifest = toml::from_str(SIMPLE_MANIFEST).unwrap();
            let store = crate::store::Store::default_store();
            crate::commands::lock::record_workflow_pins(proj.path(), &manifest, &store).unwrap();

            let rows = collect_workflow_list_rows(Some(proj.path())).unwrap();
            assert_eq!(rows.len(), 1);
            let json = workflow_list_json(&rows);
            let entry = &json["workflows"][0];
            assert_eq!(entry["name"], "t");
            assert_eq!(entry["declared"], true);
            assert_eq!(
                entry["trusted"], false,
                "pinned but not yet trusted — still listed, not refused"
            );
            assert_eq!(entry["lock_status"], "matches");
            assert_eq!(entry["roles"], serde_json::json!(["w"]));
            assert_eq!(entry["max_agents"], 25);
            assert_eq!(entry["max_wall_seconds"], 1800);
            assert!(entry["checksum"].is_string());

            // Trust the same bundle: the entry flips to trusted, lock
            // status untouched (the pinned bytes never moved).
            crate::trust::trust(proj.path()).unwrap();
            let rows = collect_workflow_list_rows(Some(proj.path())).unwrap();
            let json = workflow_list_json(&rows);
            assert_eq!(json["workflows"][0]["trusted"], true);
            assert_eq!(json["workflows"][0]["lock_status"], "matches");
        });
    }
}
