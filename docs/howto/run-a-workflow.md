<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Run a multi-agent workflow

A workflow is a reviewed JavaScript file that asks several governed agents to
work in parallel, then combines or verifies their answers. Each agent runs with
one existing toolset; the workflow cannot invent more access.

## Example: review a change from three angles

First create two narrow toolsets from capabilities the project already has:

```bash
agentstack toolset create reviewer --server github --skill code-review
agentstack toolset create verifier --skill code-review
```

`verifier` is deliberately the narrower of the two: no server, so the step that
challenges the findings can read them but cannot reach the repository through a
tool. A verifier that shares the reviewer's surface buys you very little.

Create `review-diff.js`:

```js
export const meta = {
  name: 'review-diff',
  description: 'Review the current diff from three angles, then verify the findings',
  roles: ['reviewer', 'verifier'],
}

const lenses = ['correctness', 'security', 'missing tests']

const reviews = await parallel(lenses.map((lens, i) => () =>
  agent(`Review the current git diff for ${lens}. Report only concrete findings.`, {
    role: 'reviewer',
    label: `review:${i + 1}`,
  })
))

const final = await agent(
  `Check these findings against the code. Remove weak claims and rank what remains:\n${JSON.stringify(reviews)}`,
  { role: 'verifier', label: 'verify' },
)

return { reviews, final }
```

Declare and pin it:

```bash
agentstack workflow declare --name review-diff --script ./review-diff.js --role reviewer --role verifier --max-agents 4 --max-wall-seconds 600 --preview
agentstack workflow declare --name review-diff --script ./review-diff.js --role reviewer --role verifier --max-agents 4 --max-wall-seconds 600 --write
```

`declare` copies the script into `.agentstack/workflows/`, adds the manifest
entry, and refreshes the lock as one reversible transaction. It deliberately
does not trust the new workflow for you.

## Review, run, inspect

```bash
agentstack workflow list
agentstack trust .
agentstack workflow explain review-diff
agentstack workflow run review-diff
agentstack workflow runs
agentstack workflow report <run-id>
```

- `list` shows whether the workflow is pinned and trusted.
- `trust .` is the human approval after reviewing the script and manifest.
- `explain` shows roles, ceilings, serial work, and call sites without spawning
  an agent.
- `run` launches three reviewers in parallel and one independent verifier.
- `runs` lists the recorded runs newest first — this is where a `<run-id>`
  comes from, and which rows are still resumable.
- `report` shows the workflow plus every child run, grant, outcome, and refusal.

If a run is interrupted, resume only the unfinished work:

```bash
agentstack workflow run review-diff --resume <run-id>
```

Completed steps are replayed from recorded evidence rather than run twice. A
changed script, arguments, role, or ceiling makes resume refuse.

## Why this is useful

The speed comes from parallel work. The reliability comes from giving each
worker a narrow role and making a separate verifier challenge the result. The
same pinned workflow can run on another machine, while that machine still uses
its own secrets, trust decision, policy ceiling, and installed CLI.

**Limits.** A role can only narrow — a workflow never grants authority it was
not already given — but a step's *containment* is whatever its tier provides. On
the host tier that is cooperative-guard only: the child still runs as you, so a
prompt-injected step can mislead the steps that read its output even though it
cannot widen a grant. Add `--sandbox --lockdown` when the step itself must be
confined. What each tier actually enforces per dimension, and what a workflow
step records, is the
[enforcement matrix](../ENFORCEMENT.md#workflows).

Next: [Detailed workflow model](../workflows.md) ·
[Name a toolset](name-a-toolset.md) · [See what happened](see-what-happened.md)
