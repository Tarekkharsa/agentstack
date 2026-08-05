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
agentstack x install --locked   # fetch pinned skills; fail if the lockfile would change
agentstack doctor --ci        # exit nonzero on errors, drift, policy, or unsafe content

# Building the manifest fresh in a job? Write only the manifest, no prompts:
agentstack init --secrets skip
```

Or use the one-line GitHub Action, which wraps the same gate:

```yaml
steps:
  - uses: actions/checkout@v4
  - uses: Tarekkharsa/agentstack@v0.17.1  # pin the release tag you use, never @main
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

## The trust gate, headlessly

A fresh runner is an untrusted checkout — the grant is keyed to a path on one
machine and never travels with the repo — and an untrusted project refuses to
**deliver**: no server definitions written into a CLI's config, no skill files
materialized, no instruction fragments compiled, no hooks, no extensions (see
[trust a cloned repo](trust-a-repo.md)).

**The gate above is unaffected.** `install --locked` fetches into the store and
`doctor --ci` only reads — neither renders nor activates anything, so neither
asks for consent. Both pass on an untrusted checkout, `doctor` states
"not trusted for auto mode" as a fact rather than a failure, and the one-line
Action keeps working exactly as written.

**A job that renders needs the two-step grant.** If your pipeline goes further
— `apply --write`, `use --write`, `agentstack x session start`, or a protected
`agentstack run` — it hits the gate, and a bare `agentstack trust .` refuses in
CI because stdin is not a terminal. Present the reviewed digest back instead:

```bash
# 1. Emit the review surface as JSON and grant nothing. Keep it as a build
#    artifact — it is the record of what the runner was about to approve.
agentstack trust --preview . > surface.json

# 2. Bind the grant to those exact bytes, then render.
DIGEST=$(jq -r .surface_digest surface.json)
agentstack trust . --yes --consented-digest "$DIGEST"
agentstack apply --write
```

`--yes` is refused without a digest, and `--consented-digest` is refused on any
mismatch — so a checkout that moved between the preview and the grant fails
closed rather than approving bytes nobody saw. Derive the digest from the
surface the job just printed over a pinned checkout; taking it from anywhere
else defeats the whole mechanism, since the digest *is* the claim that this
surface was reviewed.

The preview also says whether the grant can succeed before you attempt it:
`grantable` is false, with a `blockers` array naming each item and its fix,
whenever the loadable surface is not fully pinned. Branch on that rather than
parsing an error string.

Two ordering rules carry over into automation. `agentstack lock --write`
invalidates a grant, so a job that re-locks must lock **before** it trusts,
never after. And if the job runs from a repository root while the manifest sits
in a subdirectory, `agentstack trust` honours the global `--manifest-dir <dir>`
flag — the same directory the Action's `working-directory` input points at.

**Limits.** `doctor --ci` gates config health and *declared* policy, not runtime
enforcement — it checks what the manifest declares, not what a server does once
it runs. Content scanning catches known hidden-Unicode and injection heuristics,
not all malicious content. Pin the Action to a release tag so a change to the
Action itself can't slip into your pipeline.

- [Concepts](../concepts.md) — lockfile, policy, drift, secrets
- [Reference: governance (`[policy]`)](../reference.md#governance-policy)
- [Reference: content scanning](../reference.md#content-scanning)
