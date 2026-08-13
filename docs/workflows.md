<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Governed workflows

A **workflow** is one script that fans a task out to many agent runs and
composes their results — the map/reduce shape, but every worker is a
governed agent run instead of a bare process. One review chore becomes: run
a reader over each file, synthesize the findings, then run an independent
verifier to refute the weak ones. Claude Code has an ungoverned version of
this today; AgentStack's version pins the orchestration code and gives every
step its own reviewed authority.

For the shortest practical path, start with
[Run a multi-agent workflow](howto/run-a-workflow.md). The complete loop is:

```bash
agentstack workflow declare ... --preview
agentstack workflow declare ... --write
agentstack trust .
agentstack workflow explain <name>
agentstack workflow run <name>
agentstack workflow report <run-id>
```

Declare, pin, and trust a workflow, run it end to end with
`agentstack workflow run`, render its
evidence tree with `agentstack workflow report`, and resume an interrupted
run with `--resume` (replay from the recorded journal — byte-identical
script and args, or it refuses). Every agent step runs as a governed
[protected run](reference.md#execution-posture). The interpreter boundary was
independently security-reviewed on 2026-07-23, and all six findings that review
raised are now closed, each with its own witness. `agentstack workflow` is
therefore listed one hop away. It is grouped out of the everyday `agentstack
--help` list and named under **Run** on `agentstack more`. It runs at both
`agentstack more workflow` and `agentstack workflow`.

That change is discoverability, not enforcement. Not one boundary moved
when the command became listed. What the review settled is the *posture*, and
a host-tier step is still cooperative-guard only, exactly as *Honest limits*
below describes.

## Why a workflow needs governing

One workflow command spawns N agent runs, each with tool access, filesystem
reach, and token spend. The control flow is decided at runtime by script
code. That is exactly the thing a security
tool should not run on trust. So AgentStack treats the orchestration script
the same way it treats any other executable content from a repo — as
[untrusted input](ENFORCEMENT.md#what-trusted-does-and-does-not-mean)
until you review and pin it.

## The security model

- **Pinned, re-gated on change.** Workflow source is pinned in the lockfile
  by a strict content digest (a symlink anywhere is a hard error). Change one
  byte and you review again before it can run.
- **Untrusted means inert.** A workflow's name is not even invocable, and its
  source never parses as script. No dev-mode exception.
- **Roles can only narrow.** Each `agent()` call names a **role** — a toolset
  with its own tools, servers, folders, secrets, and egress. A workflow
  *requests* a closed set of roles; it can never grant or widen authority. A
  child step's grant is always within the workflow's, which is within your
  machine policy.
- **Every step is a protected run.** Each agent step goes through the full
  protected-run path — trust gate, lock verification, policy admission, a
  frozen grant, its own scoped MCP config, and a recorded outcome.
- **Per-child isolation.** Concurrent steps in one project each get their own
  launch-scoped tool config; they never touch your project's `.mcp.json` or
  each other's.
- **A complete evidence tree.** The run records which orchestration bytes
  ran, what authority every step had, and the full spawn tree — so you can
  audit exactly what happened. Each step is a protected run with its own
  report, and a call its **toolset fence** refused (the project divides its
  capabilities into toolsets and no open lease selects one that exposes the
  named server) appears there in its own **Fence refusals** section, naming
  the server, the tool, the toolset that would have exposed it, and the
  reason. It is a `fence_refused` row in that run's `events.jsonl`, kept out
  of the Tool-calls section, because a refused call was never dispatched and
  must not inflate the count of calls the step made.
- **Resume without re-running.** The evidence log doubles as the resume
  journal. An interrupted run replays its completed steps' results (verified
  against each step's recorded output digest) and executes only what never
  finished. Any divergence — script bytes, args, ceilings, roles, or an
  edited artifact — refuses; a completed step never runs twice, and a
  failed one is never silently retried.

## Honest limits

What it can and cannot do:

| It can | It cannot |
|---|---|
| Prove which pinned script ran and what authority each step had | Make a prompt-injected step *escalate* — roles are a closed, pre-reviewed set and ceilings are frozen |
| Fence a step's network reach under `--lockdown` | Contain every tool in every posture — a host-tier step is cooperative-guard only |

Step outputs are model output — untrusted data. One step's result can flow
into a later step's *prompt* by design, so a prompt-injected step can mislead
its successors; it cannot widen any grant. The built-in **validation step**
(an independent verifier under a narrower role) is the mitigation, and the
report labels each step's posture rather than implying uniform containment.

### What the interpreter bounds, and what it does not

The orchestration script runs under host-set ceilings, and two of them are
partial. A partial bound must not read as total, so both residuals are stated
here and in the posture block `agentstack workflow report` prints verbatim:

- **There is no JS heap cap.** Every path by which *untrusted* input reaches
  the interpreter heap is bounded — the invoker's args and every child result
  cross a depth-bounded JSON boundary under the resident-result ceiling,
  progress output is capped per line and per run, and the one allocation a
  script can name directly (`ArrayBuffer` / `SharedArrayBuffer`) has an
  engine-owned ceiling. But a trusted, reviewed, pinned script that allocates
  on purpose, doubling a string in a loop, is bounded by nothing here.
  The bound covers hostile ingress, not intent.
- **The native call budget covers our natives, not Boa's built-ins.** Calls
  into the host functions AgentStack installs (`agent`, `phase`, `log`,
  `budget.*`) are charged against a run-total ceiling. Work *inside a single
  built-in* — a backtracking regex, a large `sort`, `String.prototype.repeat`
  — ticks no counter at any setting, because the engine exposes no
  instruction counter or interrupt hook to build one on.

Both residuals are defects in *reviewed* content rather than hostile-input
paths. A ceiling does not contain them; the out-of-thread watchdog does. It
force-exits the process at the wall ceiling plus grace, behind an armed no-I/O
exit so a blocked write cannot keep a runaway alive. Removing them is what the
recorded QuickJS-in-wasmtime fallback is for.

**The wall-clock budget is not an engine ceiling.** `max_wall_seconds` is inert
inside the interpreter, because the engine is clock-free; the number is
surfaced to the script through `budget` only. Enforcement lives in two places
outside it, both in the CLI:

- the **drive loop's cooperative deadline**, checked at every batch boundary.
  Once the effective ceiling has passed it refuses the *next* batch and fails
  the workflow cleanly through the normal error path (exit `1`); and
- the **out-of-thread watchdog** above, armed before the first step at the
  effective ceiling plus a fixed grace, which force-exits the process
  (exit `124`) whatever the interpreter is doing.

The clock is therefore live-run state, never replayable state, because a
`--resume` must not spuriously time out replaying a run that originally used
its full wall clock. A resumed run restarts at the full effective
ceiling. The effective ceiling itself is the narrowest of the machine
cap, the manifest request and the script's own `meta` request.

## Writing one

A workflow is one JavaScript file with a small, familiar API — the same
`agent()` / `parallel()` / `pipeline()` vocabulary as Claude Code, with one
change: `agent()` takes a **role**, not a model, because the harness and
model are properties of the role's toolset, not something a script may choose.

```js
export const meta = {
  name: 'nightly-review',
  description: "Review the day's diff, then verify the findings",
  roles: ['reader', 'writer', 'verifier'],
}

const FINDINGS = {
  type: 'object',
  required: ['findings'],
  properties: {
    findings: {
      type: 'array',
      items: { type: 'object', required: ['file', 'summary'] },
    },
  },
}

// map: one reader per file, each returning validated JSON rather than prose
const mapped = await pipeline(
  files,
  f => agent(`List issues in ${f}.`, { role: 'reader', schema: FINDINGS }),
)
const findings = mapped.filter(Boolean).flatMap(m => m.findings)

// reduce: one synthesis per key group, so no single prompt has to hold everything
const claims = await parallel(
  partition(findings, 4, f => f.file).map(group => () =>
    agent(`Synthesize and rank:\n${JSON.stringify(group)}`, { role: 'writer' })),
)

// verify: an independent refuter under a narrower role
const checked = await parallel(
  claims.filter(Boolean).map(c => () => agent(`Try to refute: ${c}`, { role: 'verifier' })),
)
return keepUnrefuted(claims, checked)
```

The script runs inside a sandboxed interpreter with no filesystem, network,
or environment access — the only thing it can do is request governed agent
runs through `agent()`. Everything else is plain computation.

### What a role brings: harness, model, effort

A role is a toolset, and a toolset may declare `model` and `effort` alongside
its `harness`:

```toml
[toolsets.reader]
harness = "codex"
model = "gpt-5.5"
effort = "high"
```

Both are delivered to the child as **launch flags on its argv** — never by
writing the harness's own settings file, which a governed run must never
touch. Which flags (or whether any exist) is the adapter's answer, not
AgentStack's: each adapter descriptor declares what its CLI calls the setting
and how to select it for one headless launch.

That means a harness can be unable to carry a value, and the run says so
rather than dropping it silently. Two different facts, kept apart:

- the harness has no notion of the dimension at all, or
- the harness has the setting but no confirmed way to select it for a
  single launch.

Either way you get a warning line per child naming the role, the harness, the
dimension and the reason, and the run proceeds on that harness's own
default — an undeliverable model is a capability gap, not a manifest error. A
value the adapter's own catalog *rejects* (an effort outside its enum) is a
manifest error, and a run refuses that child before launch.
`agentstack workflow explain <name>` reports the same facts statically,
spawning nothing.

### Named algorithms

Five helpers spell out the shapes scripts kept re-deriving by hand. All five
are **pure compositions of `parallel` / `pipeline` / `shard` / `partition`**,
and not one of them calls `agent()` — an agent run happens only when *your*
callback calls it, through the same bridge a hand-written script uses. So a
helper can never widen a role or manufacture fan-out. The `role ∈ meta.roles`
check and the `max_agents` ceiling remain the only authority path.

| Helper | Shape |
|---|---|
| `mapReduce(items, { map, reduce, partitions })` | map every item, drop the failures, shuffle survivors into `partitions` buckets, reduce each |
| `reduceByKey(items, r, keyFn, reduceFn)` | group by key first, so one reducer sees all of a key's items |
| `combine(items, per, combineFn)` | Hadoop's combiner — pre-summarize chunks so the reduce prompt holds 20 things, not 200 |
| `verify(claims, refute)` | run a refuter per claim, returning `{ claim, verdict }` rows **paired by claim** |
| `keepUnrefuted(claims, verdicts, isRefuted?)` | pure filter, no await, no agent |

They follow the house rules the existing helpers do: never throw (a failed
callback becomes `null`), deterministic, and total under junk arguments
(clamped, not thrown). An empty bucket spends no agent, and the result array
still carries one slot per bucket, so your reducer count never varies with the
data.

`keepUnrefuted` is the reason this set exists at all: the worked example above
has always called it, and until now **it was never defined in the prelude** —
copying the documented example raised a `ReferenceError`.

**`keepUnrefuted`'s default predicate is a text heuristic, not a trust
boundary.** It greps the stringified verdict for "refuted". A refuter that
phrases its finding differently ("this claim is false") reads as unrefuted, a
claim whose own text contains the word reads as refuted, and a prompt-injected
refuter can say whatever it likes. The schema section says the same: shape,
not content. Pass your own `isRefuted` — ideally over a schema-validated
verdict field — whenever the answer matters. A `null` verdict (the refuter
died) is not treated as refuted, because failing closed there would silently
delete claims whenever a child run failed.

### Getting structured results back

Pass a `schema` and the promise resolves with a **parsed value** instead of
text, so a later stage can index it rather than parse prose. A result that
does not satisfy the schema fails that step closed — the script sees `null`
and decides. There is no automatic re-ask, because a retry would spend
an agent slot your ceiling never granted.

Validation constrains **shape, not content**. A schema-validated result is
still model output, and a prompt-injected step can return perfectly
schema-valid lies. It is a parsing convenience, not a trust boundary.

### Splitting and grouping

`shard(items, { per })` and `partition(items, r, keyFn)` are plain computation
over values you already have — no agent, no tokens. `partition` returns
exactly `r` buckets (empty ones included, so your reducer count never varies
with the data) and always places the same key in the same bucket, which is
what lets one reducer see all of a file's findings. It is not a balanced
split. Skewed keys make skewed buckets, and the fix is a better key.

### Keeping wide runs in memory

A run that fans out over large outputs can ask for
`agent(prompt, { result: 'handle' })`, resolving with
`{ digest, bytes, preview }` instead of the full text. There is also a
machine ceiling on total result bytes; a run that exceeds it fails closed and
tells you to use handles. Handles cost about 620 bytes each, so they are for
stages returning kilobytes. On short results they are simply pointless.

### Before you run it

`agentstack workflow explain <name>` reports the effective ceilings, which
roles launch serially, and how many `agent()` call **sites** the pinned
script has — statically, spawning nothing. Sites are not calls. One site
inside a loop runs once per item, so real fan-out is data-dependent. The
enforced bound on total spawns is `max_agents`, refused per call.

## Status

Everything above is the guide. This last section is project status — how mature
the capability is and what its security review found, kept separate so the two
are not read as one.

The full technical contract and security rationale live in the
[workflows capability design doc](archive/design/workflows-capability.md). The
manifest kind, pinning, trust review, the engine, `workflow run` /
`workflow report`, negotiated ceilings, and journal-replay resume all ship.

**The six review findings are closed**, each with a focused witness named for
it: the watchdog's no-I/O exit path, interpreter memory bounds, host-native
re-entrancy, a run-total budget for native calls, cross-host resume
determinism, and the crate boundary. Two of those witnesses are constructions
rather than assertions — the re-entrancy witness is the reproduction itself
(a getter that re-enters `agent()` during its own argument conversion, which
spawns a second child without the guard), and the crate-boundary witness fails
if any other crate takes the interpreter dependency.

**Maturity, stated separately from the security gate.** Closing the findings
is what made the command visible; it is not a claim that workflows are
well-worn. Repeated-use evidence — running real workflows on separate
occasions and confirming each is easier to repeat than hand-rolled
orchestration — stands at **1 of 3 occasions**: the 2026-07-23 acceptance run.
That is a maturity signal for you to weigh, not a gate anything is waiting on.
Expect the rough edges of a young capability. The vocabulary (admission,
ceilings, locked child runs) is the densest in the product, and *Honest limits*
above is the section to read before you rely on it.

Scaling work — how the drive loop behaves at width, and what it costs — is
tracked separately in the
[workflow scaling plan](archive/design/workflow-scaling.md), which also records two
things it did not build: automatic retry and straggler
speculation (nothing in the current model can prove a role is side-effect
free, and a claim the enforcement cannot back does not ship), and distributed
workers (the measured bottleneck is the latency tail, not a shortage of
machines).
