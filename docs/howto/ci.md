<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Use it in CI

For anyone gating a repo's agent setup in continuous integration.
Prerequisite: a committed [manifest](../concepts.md) and
[lockfile](../concepts.md) under `.agentstack/` (see
[share a setup with your team](team-setup.md)).

```bash
# Reproducible install, then a gate that fails on any problem
agentstack install --locked   # fetch pinned skills; fail if the lockfile would change
agentstack doctor --ci        # exit nonzero on errors, drift, policy, or unsafe content

# Building the manifest fresh in a job? Write only the manifest, no prompts:
agentstack init --secrets skip
```

Or use the one-line GitHub Action, which wraps the same gate:

```yaml
steps:
  - uses: actions/checkout@v4
  - uses: Tarekkharsa/agentstack@v0.15.0  # pin a release tag, not @main
```

`install --locked` fetches skill sources into the store and **fails if resolving
would change the lockfile** — so CI installs the exact pinned bytes or stops.
`doctor --ci` runs every check, prints the full report, and exits nonzero if
anything fails: a check **error**, **drift** between the manifest and the
rendered config, a `[policy]` violation (a `require`/`forbid` capability or an
`allowed_sources` breach), or **unsafe content** — `--ci` always runs the deep
supply-chain scan, so a high-severity hidden-Unicode or prompt-injection finding
fails the gate. `init --secrets skip` writes only the manifest and `${REF}`
placeholders — no prompts, no token values — for jobs that reverse-engineer a
manifest from what's on disk.

**Limits.** `doctor --ci` gates config health and *declared* policy, not runtime
enforcement — it checks what the manifest declares, not what a server does once
it runs. Content scanning catches known hidden-Unicode and injection heuristics,
not all malicious content. Pin the Action to a release tag so a change to the
Action itself can't slip into your pipeline.

- [Concepts](../concepts.md) — lockfile, policy, drift, secrets
- [Reference: governance (`[policy]`)](../reference.md#governance-policy)
- [Reference: content scanning](../reference.md#content-scanning)
