# Per-CLI instruction targeting

A runnable proof that one manifest can carry instructions for **different
audiences** and deliver each one only where it belongs. Claude Code and Codex
both read a project's agent instructions, but they read them from different
files (`CLAUDE.md` vs `AGENTS.md`) and often need genuinely different guidance —
Claude-specific tool advice, Codex-specific workflow rules, plus a shared core.
AgentStack lets you author all three in one place and compiles each fragment
only into the harnesses its `targets` name.

```bash
bash assert.sh          # asserting; exits nonzero on any mismatch
```

Requires `agentstack` on `PATH` (or `AGENTSTACK_BIN=/path/to/agentstack`, or a
built `target/release/agentstack` in this repo). It runs entirely in an isolated
temp `HOME` and `AGENTSTACK_HOME` — nothing touches your real config.

## The repo

`bundle/.agentstack/agentstack.toml` targets **Claude Code and Codex** and
declares three instruction fragments:

| Fragment      | `targets`         | Marker                   | Lands in            |
|---------------|-------------------|--------------------------|---------------------|
| `shared`      | `["*"]` (default) | `SHARED-RULE`            | `CLAUDE.md` **and** `AGENTS.md` |
| `claude_only` | `["claude-code"]` | `CLAUDE-ONLY-MARKER-7A31`| `CLAUDE.md` only    |
| `codex_only`  | `["codex"]`       | `CODEX-ONLY-MARKER-9C55` | `AGENTS.md` only    |

`bundle/CLAUDE.md` ships with a block of **hand-written prose** (marked
`HANDWRITTEN-PROSE-KEEP-ME`) that a human committed before AgentStack ran. It is
there to prove the compiler only ever edits its own managed region and leaves
everything else byte-for-byte intact.

## What the proof asserts

1. **Preview before write.** `agentstack instructions` with no `--write` is a
   read-only plan that lists both native files and both per-CLI marker texts and
   writes nothing.

2. **Targeting holds in both directions.** After `--write`:
   - `CLAUDE.md` contains `SHARED-RULE` and `CLAUDE-ONLY-MARKER-7A31` but **not**
     `CODEX-ONLY-MARKER-9C55`;
   - `AGENTS.md` contains `SHARED-RULE` and `CODEX-ONLY-MARKER-9C55` but **not**
     `CLAUDE-ONLY-MARKER-7A31`.

3. **Hand-written prose survives.** The original bytes of `CLAUDE.md` are still
   present, unchanged, as the file's prefix; the compiler only appended a single
   `<!-- agentstack:start --> … <!-- agentstack:end -->` managed region. Each
   file carries exactly one such region.

4. **Edits flow through the trust gate.** Changing the `claude_only` fragment
   drifts the content lock, so the next `instructions --write` is **refused**
   until you run `agentstack lock` to accept the change — content-pinning working
   as designed. After re-locking, the managed region updates in place (no
   duplicate region, prose still intact).

A `PASS`/`FAIL` line backs every one of these claims; the script exits nonzero if
any fails, so it doubles as a CI-grade regression check.

## Probes — diagnostics on the edges (both resolved)

The script also probes two edge cases. In the v0.10.1 baseline these were
`SKIP (defect: …)` lines — content a fragment declared could disappear with no
diagnostic (issues [#12](https://github.com/Tarekkharsa/agentstack/issues/12)
and [#14](https://github.com/Tarekkharsa/agentstack/issues/14)). Both are fixed
as of v0.15.0, so both now assert `PASS`:

- **A fragment targeting `cursor`** (a valid adapter that has **no** instructions
  file) is no longer dropped silently: `instructions` still writes nothing for
  cursor, but now **warns** that the fragment targets a CLI that has no
  instructions file, so the drop is reported to the user rather than vanishing.
- **A misspelled adapter id** (`claude-kode`) in a fragment's `targets` is now
  **rejected** — instruction targets validate like servers' do, so the unknown
  adapter id is flagged as a validation error instead of silently targeting a
  harness that does not exist.

The core targeting story — the thing this example exists to demonstrate — is
fully correct, and the edges now diagnose rather than swallow.

## What it does not claim

This example is about **instruction compilation and targeting**, not MCP
servers, policy, or runtime enforcement. See `examples/one-manifest-demo/` for
server fan-out and the secret story, and `examples/malicious-repo-demo/` for the
trust gate and tool firewall.
