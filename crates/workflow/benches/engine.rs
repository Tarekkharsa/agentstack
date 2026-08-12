//! Bench A — "engine": drive one whole `WorkflowRun` in process.
//!
//! No criterion, no new dependency (workspace rule): a plain `harness = false`
//! target with `std::time::Instant`, N iterations, median reported alongside
//! the spread so a later comparison can tell a real move from noise.
//!
//! The workload is the widest shape the engine actually ships: a `parallel()`
//! fan-out of width 100, with every `SpawnRequest` answered instantly by a
//! small fixed JSON payload. The host loop therefore contributes no I/O — what
//! is timed is the engine: `Context` construction, the prelude, the untrusted
//! script's evaluation to its first suspension, the 100-request drain, and the
//! result feed that resolves 100 promises through `bridge::value_to_js`.
//!
//! Reported separately, because they move for different reasons:
//!   (a) `WorkflowRun::new`  — context + host hooks + natives + args + prelude
//!   (b) mean per-step cost  — total of the two `step` calls / 2
//!   (c) total engine wall   — (a) + every step of the width-100 run
//!
//! Run: `cargo bench -p agentstack-workflow --bench engine`

use std::time::{Duration, Instant};

use agentstack_workflow::{
    extract_meta, Grant, RuntimeLimits, StepOutcome, StepOutput, StepResult, WorkflowRun,
};

/// Iterations per measured phase. Odd, so the median is a real sample.
const ITERS: usize = 25;
/// Discarded iterations that pay for first-touch page faults and CPU ramp.
const WARMUP: usize = 3;
/// Fan-out width of the benched workload.
const WIDTH: usize = 100;

fn main() {
    println!("# Bench A — engine (width-{WIDTH} parallel fan-out)\n");
    println!("iterations: {ITERS} (plus {WARMUP} warmup), all times in microseconds\n");

    let reference = measure(&trivial_script(), 0);
    let fanout = measure(&fanout_script(WIDTH), WIDTH);

    println!("| phase | min | median | p90 | max |");
    println!("| --- | ---: | ---: | ---: | ---: |");
    reference.new_run.row("reference: `new` (trivial script)");
    reference.per_step.row("reference: per-step (trivial script)");
    reference.total.row("reference: total (trivial script)");
    fanout.new_run.row("(a) `WorkflowRun::new`");
    fanout.step_first.row("step 1 — eval to suspension, drain 100 requests");
    fanout.step_second.row("step 2 — feed 100 results, settle root");
    fanout.per_step.row("(b) mean per-step");
    fanout.total.row("(c) total engine wall");
    fanout.per_request.row("derived: step 2 / 100 (per answered request)");
}

/// The width-N workload. `meta` must be a literal (the parse-only extractor
/// refuses computed values), so the width is templated into the source.
fn fanout_script(width: usize) -> String {
    format!(
        "const meta = {{ roles: ['bench'], maxAgents: 1000 }};\n\
         const thunks = [];\n\
         for (let i = 0; i < {width}; i++) {{\n\
           thunks.push(() => agent('task-' + i, {{ role: 'bench' }}));\n\
         }}\n\
         const outs = await parallel(thunks);\n\
         return outs.length;\n"
    )
}

/// The same engine machinery with no fan-out at all: the fixed cost floor that
/// (a) and (b) sit on top of.
fn trivial_script() -> String {
    "const meta = { roles: ['bench'] };\nreturn 1;\n".to_string()
}

/// The fixed answer every request gets: small, so the boundary conversion is
/// charged honestly to bench B rather than smuggled into this one.
fn payload() -> serde_json::Value {
    serde_json::json!({ "ok": true, "text": "done" })
}

fn grant_for(script: &str) -> Grant {
    let meta = extract_meta(script).expect("bench script parses");
    Grant {
        max_agents: 1000,
        max_wall_seconds: 1800,
        admitted_roles: meta.roles,
    }
}

struct Measured {
    new_run: Stats,
    step_first: Stats,
    step_second: Stats,
    per_step: Stats,
    total: Stats,
    per_request: Stats,
}

/// Drive `ITERS + WARMUP` complete runs, timing each phase separately.
///
/// `width` is the number of requests expected in the first batch; 0 means the
/// script never suspends and the single step settles the root directly.
fn measure(script: &str, width: usize) -> Measured {
    let grant = grant_for(script);
    let answer = payload();

    let mut new_run = Vec::with_capacity(ITERS);
    let mut step_first = Vec::with_capacity(ITERS);
    let mut step_second = Vec::with_capacity(ITERS);
    let mut per_step = Vec::with_capacity(ITERS);
    let mut total = Vec::with_capacity(ITERS);

    for i in 0..(ITERS + WARMUP) {
        let start = Instant::now();
        let mut run = WorkflowRun::new(
            script,
            RuntimeLimits::default(),
            serde_json::Value::Null,
            grant.clone(),
        )
        .expect("bench script constructs");
        let t_new = start.elapsed();

        let start = Instant::now();
        let outcome = run.step(Vec::new());
        let t_first = start.elapsed();

        let t_second = match outcome {
            StepOutcome::Batch(batch) => {
                assert_eq!(batch.requests.len(), width, "unexpected fan-out width");
                // Building the answer vector is host work, not engine work —
                // it is deliberately outside the timed region.
                let results: Vec<StepResult> = batch
                    .requests
                    .iter()
                    .map(|r| StepResult {
                        request_id: r.id,
                        output: StepOutput::Completed(answer.clone()),
                    })
                    .collect();
                let start = Instant::now();
                let outcome = run.step(results);
                let elapsed = start.elapsed();
                match outcome {
                    StepOutcome::Done(value) => assert_eq!(value, serde_json::json!(width)),
                    other => panic!("expected Done, got {other:?}"),
                }
                elapsed
            }
            StepOutcome::Done(value) => {
                assert_eq!(width, 0, "a fan-out script settled without suspending");
                assert_eq!(value, serde_json::json!(1));
                Duration::ZERO
            }
            other => panic!("expected Batch or Done, got {other:?}"),
        };

        if i < WARMUP {
            continue;
        }
        let steps = if width == 0 { 1.0 } else { 2.0 };
        new_run.push(micros(t_new));
        step_first.push(micros(t_first));
        step_second.push(micros(t_second));
        per_step.push((micros(t_first) + micros(t_second)) / steps);
        total.push(micros(t_new) + micros(t_first) + micros(t_second));
    }

    let divisor = if width == 0 { 1.0 } else { width as f64 };
    let per_request: Vec<f64> = step_second.iter().map(|s| s / divisor).collect();

    Measured {
        new_run: Stats::from(new_run),
        step_first: Stats::from(step_first),
        step_second: Stats::from(step_second),
        per_step: Stats::from(per_step),
        total: Stats::from(total),
        per_request: Stats::from(per_request),
    }
}

fn micros(d: Duration) -> f64 {
    d.as_nanos() as f64 / 1000.0
}

/// Median plus the spread that says whether a later median move is real.
struct Stats {
    min: f64,
    median: f64,
    p90: f64,
    max: f64,
}

impl Stats {
    fn from(mut samples: Vec<f64>) -> Self {
        assert!(!samples.is_empty(), "no samples collected");
        samples.sort_by(|a, b| a.partial_cmp(b).expect("bench timings are never NaN"));
        let last = samples.len() - 1;
        let p90_index = (samples.len() * 9 / 10).min(last);
        Self {
            min: samples[0],
            median: samples[samples.len() / 2],
            p90: samples[p90_index],
            max: samples[last],
        }
    }

    fn row(&self, label: &str) {
        println!(
            "| {label} | {:.3} | {:.3} | {:.3} | {:.3} |",
            self.min, self.median, self.p90, self.max
        );
    }
}
