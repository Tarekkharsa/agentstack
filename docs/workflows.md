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

**The full path ships today.** Declare, pin, and trust a
workflow, run it end to end with `agentstack workflow run`, render its
evidence tree with `agentstack workflow report`, and resume an interrupted
run with `--resume` (replay from the recorded journal — byte-identical
script and args, or it refuses). Every agent step runs as a governed
[locked run](reference.html). The interpreter boundary passed its
independent security review on 2026-07-23; what that settled is the
*posture*, not the enforcement — a host-tier step is still cooperative-guard
only, exactly as *Honest limits* below describes.

## Why a workflow needs governing

A workflow is **authority, multiplied**: one command spawns N agent runs,
each with tool access, filesystem reach, and token spend, with the control
flow decided at runtime by script code. That is exactly the thing a security
tool should not run on trust. So AgentStack treats the orchestration script
the same way it treats any other executable content from a repo — as
[untrusted input](enforcement.html#what-trusted-does-and-does-not-mean)
until you review and pin it.

## The security model

- **Pinned, re-gated on change.** Workflow source is pinned in the lockfile
  by a strict content digest (a symlink anywhere is a hard error). Change one
  byte and trust re-gates — you review again before it can run.
- **Untrusted means inert.** Until the bundle is trusted, a workflow never
  parses as script and its name is not even invocable. No dev-mode exception.
- **Roles can only narrow.** Each `agent()` call names a **role** — a toolset
  with its own tools, servers, folders, secrets, and egress. A workflow
  *requests* a closed set of roles; it can never grant or widen authority. A
  child step's grant is always within the workflow's, which is within your
  machine policy.
- **Every step is a locked run.** Each agent step goes through the full
  protected-run path — trust gate, lock verification, policy admission, a
  frozen grant, its own scoped MCP config, and a recorded outcome.
- **Per-child isolation.** Concurrent steps in one project each get their own
  launch-scoped tool config; they never touch your project's `.mcp.json` or
  each other's.
- **A complete evidence tree.** The run records which orchestration bytes
  ran, what authority every step had, and the full spawn tree — so you can
  audit exactly what happened.
- **Resume without re-running.** The evidence log doubles as the resume
  journal: an interrupted run replays its completed steps' results (verified
  against each step's recorded output digest) and only executes what never
  finished. Any divergence — script bytes, args, ceilings, roles, or an
  edited artifact — refuses; a completed step never runs twice, and a
  failed one is never silently retried.

## Honest limits

What AgentStack can promise here has a sharp edge, and the docs say it plainly:

| It can | It cannot |
|---|---|
| Prove which pinned script ran and what authority each step had | Make a prompt-injected step *escalate* — roles are a closed, pre-reviewed set and ceilings are frozen |
| Fence a step's network reach under `--lockdown` | Contain every tool in every posture — a host-tier step is cooperative-guard only |

Step outputs are model output — untrusted data. One step's result can flow
into a later step's *prompt* by design, so a prompt-injected step can mislead
its successors; it cannot widen any grant. The built-in **validation step**
(an independent verifier under a narrower role) is the mitigation, and the
report labels each step's posture rather than implying uniform containment.

## Writing one

A workflow is one JavaScript file with a small, familiar API — the same
`agent()` / `parallel()` / `pipeline()` vocabulary as Claude Code, with one
change: `agent()` takes a **role**, not a model, because the harness and
model are properties of the role's toolset, not something a script may choose.

```js
export const meta = {
  name: 'nightly-review',
  description: "Review the day's diff, then verify the findings",
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

### Getting structured results back

Pass a `schema` and the promise resolves with a **parsed value** instead of
text, so a later stage can index it rather than parse prose. A result that
does not satisfy the schema fails that step closed — the script sees `null`
and decides. There is deliberately no automatic re-ask: a retry would spend
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
split: skewed keys make skewed buckets, and the fix is a better key.

### Keeping wide runs in memory

A run that fans out over large outputs can ask for
`agent(prompt, { result: 'handle' })`, resolving with
`{ digest, bytes, preview }` instead of the full text. There is also a
machine ceiling on total result bytes; a run that exceeds it fails closed and
tells you to use handles. Handles cost about 620 bytes each, so they are for
stages returning kilobytes — on short results they are simply pointless.

### Before you run it

`agentstack workflow explain <name>` reports the effective ceilings, which
roles launch serially, and how many `agent()` call **sites** the pinned
script has — statically, spawning nothing. Sites are not calls: one site
inside a loop runs once per item, so real fan-out is data-dependent. The
enforced bound on total spawns is `max_agents`, refused per call.

## Where it stands

The full technical contract and security rationale live in the
[workflows capability design doc](design/workflows-capability.md). The
manifest kind, pinning, trust review, the engine, `workflow run` /
`workflow report`, negotiated ceilings, and journal-replay resume all ship,
and the interpreter boundary has passed its independent security review.
What remains before workflows leave experimental is repeated-use evidence —
running real workflows on separate occasions and confirming each is easier
to repeat than hand-rolled orchestration (`TODO.md`).

Scaling work — how the drive loop behaves at width, and what it costs — is
tracked separately in the
[workflow scaling plan](design/workflow-scaling.md), which also records two
things it deliberately did **not** build: automatic retry and straggler
speculation (nothing in the current model can prove a role is side-effect
free, and a claim the enforcement cannot back does not ship), and distributed
workers (the measured bottleneck is the latency tail, not a shortage of
machines).
