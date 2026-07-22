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
//!    Boa). The recorded-outcome half of that story is Stage E.

use std::collections::{HashMap, HashSet};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use anyhow::{Context as AnyhowContext, Result};
use owo_colors::OwoColorize;

use agentstack_workflow::{
    extract_meta, Grant, Progress, RuntimeLimits, SpawnRequest, StepOutcome, StepOutput,
    StepResult, WorkflowRun,
};

use crate::cli::{RunArgs, WorkflowRunArgs};
use crate::commands::locked::{run_locked_child, supports_injection};
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

    // 3. Engine construction inside catch_unwind at the CLI edge.
    let grant = Grant {
        max_agents: effective_agents,
        max_wall_seconds: effective_wall,
        admitted_roles: wf.roles.clone(),
    };
    let mut run =
        contained(|| WorkflowRun::new(&script, RuntimeLimits::default(), args_value, grant))?
            .map_err(|e| anyhow::anyhow!("refusing workflow '{}': {e}", wf.name))?;

    // Effective concurrency: machine-configurable, engine-owned, never
    // script-visible. `.max(1)` so a machine cap of 0 bounds to serial
    // instead of deadlocking the drive.
    let max_concurrent = machine_policy
        .workflows
        .max_concurrent
        .unwrap_or(DEFAULT_MAX_CONCURRENT)
        .max(1) as usize;

    let narrowed = effective_agents != wf.max_agents || effective_wall != wf.max_wall_seconds;
    eprintln!(
        "{} workflow '{}' admitted: {} role(s), effective ceilings max_agents={} \
         max_wall_seconds={}{}, concurrency cap {}",
        "▶".green(),
        wf.name.bold(),
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

    // 4. The out-of-thread watchdog, armed before the first step() at the
    // EFFECTIVE (possibly script-narrowed) wall ceiling plus a fixed grace —
    // the hard backstop above the cooperative deadline below. `done_tx` lives
    // to the end of this function; every return path drops it, which wakes
    // the watchdog with Disconnected and retires it.
    let pids: crate::runs::ChildPids = Arc::new(Mutex::new(HashSet::new()));
    let done_tx = spawn_watchdog(wf.name.clone(), effective_wall, Arc::clone(&pids));

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
    let final_value = loop {
        let outcome = run.step(std::mem::take(&mut results));
        print_progress(run.take_progress());
        match outcome {
            StepOutcome::Batch(batch) => {
                if std::time::Instant::now() >= deadline {
                    drop(done_tx);
                    note_exhaustion(&run, effective_agents);
                    anyhow::bail!(
                        "workflow '{}' exceeded its effective wall-clock ceiling ({}s) — \
                         refusing the next batch and failing cleanly in-band (the out-of-thread \
                         watchdog at ceiling+{}s remains the stall backstop; recording this \
                         outcome in the run evidence lands in Stage E)",
                        wf.name,
                        effective_wall,
                        WATCHDOG_GRACE_SECS
                    );
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
                    anyhow::bail!(
                        "workflow '{}': engine-invariant breach — {} spawns handed out against \
                         a granted ceiling of {} — failing closed (this is an engine defect, \
                         not a script error; please report it)",
                        wf.name,
                        spawned_total,
                        effective_agents
                    );
                }
                results = execute_batch(
                    manifest_dir,
                    &bindings,
                    &batch.requests,
                    max_concurrent,
                    &pids,
                );
            }
            StepOutcome::Done(value) => break value,
            StepOutcome::Failed(err) => {
                drop(done_tx);
                note_exhaustion(&run, effective_agents);
                anyhow::bail!("workflow '{}' failed: {err}", wf.name);
            }
        }
    };
    drop(done_tx);
    note_exhaustion(&run, effective_agents);
    Ok(final_value)
}

/// The Stage D honesty line: if any `agent()` call was refused at the granted
/// ceiling (the engine's non-forgeable flag — a script that caught and
/// absorbed the refusal cannot hide it), say so on stderr regardless of how
/// the run ended. The recorded-report half of this honesty is Stage E.
fn note_exhaustion(run: &WorkflowRun, granted: u32) {
    if run.exhausted() {
        eprintln!(
            "{} the granted agent ceiling ({granted}) was exhausted during this run — refused \
             agent() calls failed closed (the script saw each refusal); recording this in the \
             run report lands in Stage E",
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
/// contradicts the admitted ceiling or `budget.maxWallSeconds` — best-effort
/// SIGTERMs the live child process groups, and force-exits the PROCESS — the
/// §12.2 hard backstop, deliberately not a cooperative check (a drive thread
/// stuck inside a Boa builtin slice cannot observe a deadline).
fn spawn_watchdog(
    name: String,
    effective_wall: u64,
    pids: crate::runs::ChildPids,
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
                 children receive SIGTERM; recording this outcome in the run evidence lands in \
                 Stage E.",
            );
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

/// Fan one engine batch out as locked children. Injection-capable children
/// run concurrently under the cap (each is an independent worker thread
/// re-running the FULL per-child gate sequence — trust, strict verify,
/// admission, grant freeze — via `run_locked_child`); park/swap children run
/// strictly serially afterwards, labeled `serial (config-swap)` (§12.1: the
/// one deliberate degrade, stated honestly). Results are keyed by request id,
/// so completion order is irrelevant.
fn execute_batch(
    manifest_dir: Option<&Path>,
    bindings: &HashMap<String, RoleBinding>,
    requests: &[SpawnRequest],
    max_concurrent: usize,
    pids: &crate::runs::ChildPids,
) -> Vec<StepResult> {
    let (concurrent, serial): (Vec<&SpawnRequest>, Vec<&SpawnRequest>) = requests
        .iter()
        .partition(|r| bindings.get(&r.role).map(|b| b.injectable).unwrap_or(false));

    let results: Mutex<Vec<StepResult>> = Mutex::new(Vec::with_capacity(requests.len()));
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
                let result = run_child(manifest_dir, bindings, request, pids, false);
                results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(result);
            });
        }
    });
    for request in serial {
        let result = run_child(manifest_dir, bindings, request, pids, true);
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
) -> StepResult {
    let failed = |output: StepOutput| StepResult {
        request_id: request.id,
        output,
    };
    let Some(binding) = bindings.get(&request.role) else {
        // The bridge + admission cross-check make this unreachable; refusing
        // the step (not the process) keeps the failure in-band regardless.
        eprintln!(
            "  ✗ agent #{} names unbound role '{}' — failing the step closed",
            request.id,
            sanitize_line(&request.role)
        );
        return failed(StepOutput::Failed);
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
        run_locked_child(manifest_dir, &child_args, pids)
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
            return failed(StepOutput::Failed);
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
                    failed(StepOutput::Completed(serde_json::Value::String(
                        String::from_utf8_lossy(&report.stdout).into_owned(),
                    )))
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
                    failed(StepOutput::Failed)
                }
            }
        }
        Err(e) => {
            // A gate refusal (trust, verify, admission, freeze) or launch
            // failure: recorded by the child path itself; the step fails
            // closed and the script decides severity (R1).
            eprintln!("  {} agent #{} refused: {:#}", "✗".red(), request.id, e);
            failed(StepOutput::Failed)
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
}
