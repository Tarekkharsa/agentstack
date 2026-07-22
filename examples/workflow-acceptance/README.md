# workflow-acceptance — the engine's day-one test

The benchmarked map → reduce → validation-reduce pipeline from the
2026-07-22 evidence runs, packaged so the governed workflow engine
(D7 Stage B/C, `crates/workflow`) can be pointed at it the day
`workflow run` exists. The same pipeline has already been run twice
without the engine, so the engine's first run lands between two known
bookends instead of in a vacuum.

## The bookends (recorded 2026-07-22, this exact pipeline, same machine)

| path | wall clock | verdict stage | notes |
|---|---|---|---|
| Pure Claude Code Workflow (ungoverned control) | **22.6 s** | worked | no admission, no grants, no recorded hashes |
| Interim courier path (governed workers) | **86.4 s** | worked | ~49 s of the gap is courier scaffolding, deleted by the engine |
| **Engine target** | **~25–40 s** | must work | "proof-almost-for-free" |

Admission itself is free: measured head-to-head, `agentstack run
claude-code --locked --prompt` (4.5–5.3 s) is indistinguishable from a
bare `claude -p` (5.0–6.4 s) on the same prompt. The gates are
milliseconds; the child cost is CLI start + model call. If the engine
run lands far above ~40 s, the regression is in the engine, not the
gates. (Do not benchmark children against in-process Workflow agents
(~2–2.5 s) — that measures a process spawn avoided, not overhead added.)

## Contents

- `bundle/` — a trustable project: manifest with the pinned
  `[workflows.mapreduce-acceptance]` capability (W1 schema, parses
  today), three empty-surface role profiles, and the workflow script
  under `.agentstack/workflows/`, written against the §3 v1 engine API
  (`agent(prompt, {role, label})`, text returns, no `schema`).
- `check-evidence.sh` — engine-agnostic assertions over the recorder
  output that already ships (`~/.agentstack/runs/<id>/events.jsonl`).
  It also passes against the interim courier path, so it is the
  before/after harness, not just the after.

## Running it

```bash
cp -r bundle /tmp/wf-acceptance && cd /tmp/wf-acceptance
agentstack lock
agentstack trust . --yes
ls ~/.agentstack/runs > /tmp/runs-before.txt
time agentstack workflow run mapreduce-acceptance
./check-evidence.sh /tmp/runs-before.txt /tmp/wf-acceptance
```

Adopted (Stage C) as the tracked end-to-end acceptance test: CI runs
this bundle through the real binary against a fake prompt-driven
`claude` (`crates/cli/tests/workflow_e2e.rs`), proving the admission +
drive + spawn composition. The real-model run above (with the
performance bookends) stays a manual procedure.

## Pass criteria

**Functional** — the workflow returns `pass: true`: three non-empty map
outputs, a reduce sentence, and a verdict matching
`^(CONFIRMED|REFUTED)`. **Both verdicts pass.** Semantic drift is model
variance, not an engine property; across the three recorded runs the
validation reducer caught real drift twice (hallucinated "signatures";
dropped fail-closed) with zero false refutations — the acceptance claim
is that the verifier runs governed and returns well-formed, not which
way it votes.

**Evidence** — `check-evidence.sh` passes: exactly 5 child runs, four
gates green each, exit 0 each, **5 distinct grant digests**, a genuine
3-way wall-clock overlap among the map children, and the rig's
`.mcp.json` untouched (absent) throughout.

**Performance (soft)** — total wall clock ≤ 45 s on a warm machine.
Not a hard gate; a miss means "explain where the time went", with the
bookends above as the reference.

## Honest scope

Written before the engine existed, against the approved design (§3, §12
of `docs/design/workflows-capability.md`), then adapted to the shipped
v1-C surface at Stage C: `meta.roles` added (required by the engine as
the script-internal consistency set; the manifest roles stay the
authority). The script uses no `budget` — that object is Stage D, and
nothing here stubs it. Role→harness binding is the profile's optional
`harness` field (absent here → the claude-code default). The stable
part is `check-evidence.sh`, which only reads recorder output that
v0.15.0 already produces.
