//! Bench B — "boundary": the serde_json <-> Boa conversion, on its own.
//!
//! No criterion, no new dependency (workspace rule): a plain `harness = false`
//! target with `std::time::Instant`, N outer iterations, median reported with
//! the spread.
//!
//! Two views of the same code, because each answers a different question:
//!
//! * **Direct** (`--features bench-internals`) — `value_to_js` and
//!   `js_to_value` timed alone through the crate's bench seam. This is the
//!   number to compare after a change to `crates/workflow/src/bridge.rs`.
//! * **Public round trip** (always) — the same conversions as an unmodified
//!   caller reaches them: `WorkflowRun::new` converts `args` in, a script that
//!   returns `args` converts them back out. Coarser by roughly the cost of a
//!   `Context` plus prelude, and reported unsubtracted so nothing is hidden,
//!   but reproducible with no feature flag at all.
//!
//! Run: `cargo bench -p agentstack-workflow --bench boundary --features bench-internals`

use std::time::{Duration, Instant};

use agentstack_workflow::{extract_meta, Grant, RuntimeLimits, StepOutcome, WorkflowRun};

/// Outer iterations per measured phase. Odd, so the median is a real sample.
const ITERS: usize = 25;
/// Discarded iterations that pay for first-touch page faults and CPU ramp.
const WARMUP: usize = 3;

fn main() {
    let payloads = payloads();

    println!("# Bench B — boundary (serde_json <-> Boa)\n");
    println!("iterations: {ITERS} (plus {WARMUP} warmup), all times in microseconds\n");

    println!("## payloads\n");
    println!("| payload | JSON bytes |");
    println!("| --- | ---: |");
    for (name, value) in &payloads {
        let bytes = serde_json::to_string(value).map_or(0, |s| s.len());
        println!("| {name} | {bytes} |");
    }

    direct(&payloads);
    public_round_trip(&payloads);
}

/// The four shapes under test. `wide array` and `object` stress the per-element
/// paths (`JsArray::push`, `obj.set`, `arr.at`); the two flat strings stress the
/// single-allocation path.
fn payloads() -> Vec<(&'static str, serde_json::Value)> {
    let flat = |n: usize| serde_json::Value::String("a".repeat(n));
    let wide: Vec<serde_json::Value> = (0..10_000)
        .map(|i| serde_json::Value::String(format!("e{i}")))
        .collect();
    let mut object = serde_json::Map::new();
    for i in 0..1_000 {
        object.insert(format!("k{i}"), serde_json::Value::String(format!("v{i}")));
    }
    vec![
        ("1 KB flat string", flat(1024)),
        ("64 KB flat string", flat(64 * 1024)),
        (
            "wide array, 10,000 elements",
            serde_json::Value::Array(wide),
        ),
        ("object, 1,000 keys", serde_json::Value::Object(object)),
    ]
}

// --- direct: the converters alone -----------------------------------------

#[cfg(feature = "bench-internals")]
fn direct(payloads: &[(&'static str, serde_json::Value)]) {
    use agentstack_workflow::BoundaryBench;

    println!("\n## direct — `value_to_js` / `js_to_value` alone\n");
    println!("| payload | direction | repeat | min | median | p90 | max |");
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: |");

    for (name, value) in payloads {
        let mut bench = BoundaryBench::new(RuntimeLimits::default()).expect("bench context");

        let repeat = calibrate(|| {
            bench.to_js(value).expect("to_js");
        });
        let stats = sample(repeat, || {
            bench.to_js(value).expect("to_js");
        });
        stats.row(name, "serde_json -> JS (`value_to_js`)", repeat);

        bench.to_js(value).expect("to_js");
        let repeat = calibrate(|| {
            bench.from_js().expect("from_js");
        });
        let stats = sample(repeat, || {
            bench.from_js().expect("from_js");
        });
        stats.row(name, "JS -> serde_json (`js_to_value`)", repeat);
    }
}

#[cfg(not(feature = "bench-internals"))]
fn direct(_payloads: &[(&'static str, serde_json::Value)]) {
    println!("\n## direct — skipped\n");
    println!(
        "`value_to_js` / `js_to_value` are `pub(crate)`. Re-run with \
         `--features bench-internals` to time them directly."
    );
}

/// Calibration target for one timed region: a 1 KB conversion is far below
/// timer noise on its own, so each sample repeats the operation until the
/// region clears this, then divides the count back out.
#[cfg(feature = "bench-internals")]
const TARGET: Duration = Duration::from_micros(500);
/// Ceiling on that repeat count, so a slow payload cannot run away.
#[cfg(feature = "bench-internals")]
const MAX_REPEAT: usize = 2000;

/// How many repeats one timed region needs to clear [`TARGET`].
#[cfg(feature = "bench-internals")]
fn calibrate(mut op: impl FnMut()) -> usize {
    let mut repeat = 1;
    loop {
        let start = Instant::now();
        for _ in 0..repeat {
            op();
        }
        if start.elapsed() >= TARGET || repeat >= MAX_REPEAT {
            return repeat;
        }
        repeat = (repeat * 2).min(MAX_REPEAT);
    }
}

/// `WARMUP + ITERS` samples of `repeat` operations each; reported per operation.
#[cfg(feature = "bench-internals")]
fn sample(repeat: usize, mut op: impl FnMut()) -> Stats {
    let mut samples = Vec::with_capacity(ITERS);
    for i in 0..(ITERS + WARMUP) {
        let start = Instant::now();
        for _ in 0..repeat {
            op();
        }
        let elapsed = start.elapsed();
        if i >= WARMUP {
            samples.push(micros(elapsed) / repeat as f64);
        }
    }
    Stats::from(samples)
}

// --- public round trip: the same conversions through the shipped API -------

/// `WorkflowRun::new(args = P)` pays `value_to_js(P)` inside a context build
/// plus prelude; a script that returns `args` pays `js_to_value(P)` inside a
/// script evaluation. Both references (`args = null`, `return null`) are
/// printed so a reader can take the difference themselves, rather than being
/// handed a subtraction dressed up as a clean measurement.
fn public_round_trip(payloads: &[(&'static str, serde_json::Value)]) {
    println!("\n## public round trip — unsubtracted, no feature flag\n");
    println!("| payload | phase | repeat | min | median | p90 | max |");
    println!("| --- | --- | ---: | ---: | ---: | ---: | ---: |");

    let return_args = "const meta = { roles: [] };\nreturn args;\n";
    let return_null = "const meta = { roles: [] };\nreturn null;\n";
    let grant = grant_for(return_args);
    let null = serde_json::Value::Null;

    sample_new(return_args, &null, &grant).row("(reference)", "`new`, args = null", 1);
    sample_step(return_null, &null, &grant).row("(reference)", "`step`, `return null`", 1);

    for (name, value) in payloads {
        sample_new(return_args, value, &grant).row(name, "`new`, args = payload", 1);
        sample_step(return_args, value, &grant).row(name, "`step`, `return args`", 1);
    }
}

/// Time `WorkflowRun::new` only. Each constructed run is parked in `keep` and
/// dropped after the loop, so no sample is charged for the previous sample's
/// `Context` teardown.
fn sample_new(script: &str, args: &serde_json::Value, grant: &Grant) -> Stats {
    let mut keep = Vec::with_capacity(ITERS + WARMUP);
    let mut samples = Vec::with_capacity(ITERS);
    for i in 0..(ITERS + WARMUP) {
        let start = Instant::now();
        let run = build(script, args, grant);
        let elapsed = start.elapsed();
        keep.push(run);
        if i >= WARMUP {
            samples.push(micros(elapsed));
        }
    }
    drop(keep);
    Stats::from(samples)
}

/// Time the single `step` that evaluates the script and settles the root. A run
/// is single-shot, so every sample needs its own; they are all built up front,
/// outside the timed regions.
fn sample_step(script: &str, args: &serde_json::Value, grant: &Grant) -> Stats {
    let mut runs: Vec<WorkflowRun> = (0..ITERS + WARMUP)
        .map(|_| build(script, args, grant))
        .collect();
    let mut samples = Vec::with_capacity(ITERS);
    for (i, run) in runs.iter_mut().enumerate() {
        let start = Instant::now();
        match run.step(Vec::new()) {
            StepOutcome::Done(_) => {}
            other => panic!("expected Done, got {other:?}"),
        }
        let elapsed = start.elapsed();
        if i >= WARMUP {
            samples.push(micros(elapsed));
        }
    }
    Stats::from(samples)
}

fn build(script: &str, args: &serde_json::Value, grant: &Grant) -> WorkflowRun {
    WorkflowRun::new(
        script,
        RuntimeLimits::default(),
        args.clone(),
        grant.clone(),
    )
    .expect("bench script constructs")
}

fn grant_for(script: &str) -> Grant {
    let meta = extract_meta(script).expect("bench script parses");
    Grant {
        max_agents: 1000,
        max_wall_seconds: 1800,
        admitted_roles: meta.roles,
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

    fn row(&self, payload: &str, phase: &str, repeat: usize) {
        println!(
            "| {payload} | {phase} | {repeat} | {:.4} | {:.4} | {:.4} | {:.4} |",
            self.min, self.median, self.p90, self.max
        );
    }
}
