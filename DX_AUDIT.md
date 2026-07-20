# DX Audit — first-time-user walkthrough (v0.14.0)

Audited 2026-07-20 against a release build, run as a newcomer would: in an empty
directory, in a minimal hand-written project, with typos, missing secrets, and
closed stdin. Honest headline first: **the happy path is already good.** The
no-args orientation screen, the `init` wizard (detects 8 CLIs, imports 11
servers, lifts plaintext tokens to `${REF}` with an explanation), did-you-mean
on typo'd commands *and* flags, `explain`'s not-found error, and doctor's
`↳ fix-command` lines are all better than most CLIs ship. The gaps below are
residual, but several of them hit exactly the moments where a newcomer forms
their first impression: the first error, the first wall of output, the first
"which command was that again?".

Ranking: within each section, highest impact first.

---

## (a) Time-to-first-success blockers

Measured happy path today: `agentstack` (orientation, 11 ms) → `agentstack init`
(wizard) → done. That is already ≤2 minutes interactively. The blockers below
are the off-happy-path entries that cost newcomers real time.

### A1. `run` prints a success banner before validating anything

```
$ agentstack run nosuchcli
▶ launching nosuchcli…
  posture: HOST / ADVISORY
  ⚠ host mode: policy is advisory — the gateway brokers MCP tool calls, but …
error: unknown harness 'nosuchcli' — see `agentstack adapters list`
```

Same with a valid harness but no manifest: "▶ launching claude…" + the
three-line posture lecture, *then* the missing-manifest error. The newcomer
reads "launching" and thinks it started. (`crates/cli/src/commands/runs.rs:32`
prints the banner before harness/manifest resolution; `sandbox.rs:415` already
fixed exactly this bug for the Docker-daemon check — same treatment needed
here.)

**A great CLI prints:** validation errors first, banner only once launch is
actually going to happen:

```
$ agentstack run nosuchcli
error: unknown harness 'nosuchcli' — see `agentstack adapters list`, or try one
of: claude-code · codex · gemini
```

### A2. A manifest without `[targets]` silently fans out to all 13 CLIs

`init` writes `[targets].default` from detection, so wizard users are safe. But
a newcomer who hand-writes the minimal manifest from the docs (or copies an
example) gets `apply`/`use`/`doctor` operating on **all 13 adapters**, creating
`.cursor/`, `.junie/`, `.agents/`, `.vscode/` directories in their repo for
CLIs they don't have, and a blocked `apply --write` reports "9 blocked" where
the user has maybe 3 real CLIs. Everything gets ~3× noisier and the failure
counts stop matching reality.

**A great CLI defaults to:** detected CLIs when `[targets]` is absent, with one
line saying so: `targets: 8 detected CLIs (no [targets] in manifest — pin with
'agentstack init' or a [targets] block)`.

### A3. Non-interactive `init` refusal is correct but buries the fix

The refusal (piped stdin, no flags) is the right call security-wise, but it's a
7-line paragraph whose three escape hatches (`--yes`, `--secrets`, `--dry-run`)
are embedded mid-sentence. Scripted first-touch (CI, dotfiles bootstrap, an
agent running it) hits this as their literal first output.

**A great CLI prints:** the what/why in one line, then the three options as a
copy-pasteable list, `--dry-run` first (the safe one).

---

## (b) Confusing errors

### B1. The missing-manifest error leaks internals and names the wrong path

The single most common newcomer error (any command before `init`):

```
$ agentstack apply
error: no manifest here (looked for /path/to/dir/agentstack.toml) — run
`agentstack init` to create .agentstack/agentstack.toml, or point at one with
--manifest-dir: No such file or directory (os error 2)
```

Three problems: (1) trailing `: No such file or directory (os error 2)` — an
io::Error context chain leaking through (`crates/core/src/manifest/load.rs:136`);
(2) it says it looked for the *legacy root* `agentstack.toml` while telling you
`init` creates `.agentstack/agentstack.toml` — the newcomer can't tell which
path is the real one; (3) it's one unbroken 40-word sentence.

**A great CLI prints:**

```
error: no manifest in this directory (or any parent)

  Start one:            agentstack init
  Or point at one:      agentstack --manifest-dir <dir> apply
```

### B2. Blocked `apply --write` says the same thing three times

```
Wrote 0 of 9 target(s); 9 blocked by unresolved secret(s) or missing fragment source(s); see ✗ above.
9 issue(s) — resolve before writing.
error: blocked write(s) on 9 target(s) — set the missing secret(s) (`agentstack secret set <NAME>`), restore missing fragment source(s), or pass --allow-unresolved for unresolved secrets
```

Three stacked summaries of one fact. The per-target ✗ lines above already name
the secret; the tail should be one line with the *specific* command
(`agentstack secret set DEMO_TOKEN` — it knows the name) rather than the
`<NAME>` placeholder and the fragment-sources clause that doesn't apply to this
run.

### B3. `use`'s ✗ lines don't carry the fix; doctor's do

`doctor` prints `✗ DEMO_TOKEN not found ↳ agentstack secret set DEMO_TOKEN`.
`use` and `apply` print `✗ unresolved secret DEMO_TOKEN (server 'demo')` with
no `↳`. Same condition, two voices; the newcomer meets the unhelpful one first.

### B4. `init` repeats identical warnings with no action

```
⚠ server 'tldraw' differs between CLIs — kept the first definition
⚠ server 'tldraw' differs between CLIs — kept the first definition
⚠ server 'tldraw' differs between CLIs — kept the first definition
```

Three identical lines (once per extra CLI that defines it), and no way to see
what was dropped or how to pick a different definition. Dedupe to one line with
a count and a pointer: `⚠ 'tldraw' defined differently in 4 CLIs — kept Claude
Code's; compare: agentstack diff`.

### Positives worth keeping (the bar the fixes must match)

- Typo'd subcommand *and* typo'd flag both get "a similar X exists" (clap).
- `explain nosuchthing` → "no server, skill, or instruction 'nosuchthing'…
  Try `agentstack search nosuchthing`" — the model error for the whole CLI.
- `apply` with no TTY defaults to dry-run and ends "Dry run. Re-run with
  --write to apply." Fail-closed secrets block writes per target. Exit codes
  are correct (verified unpiped: doctor 0/`--ci` gates, apply-blocked 1,
  init-refusal 1).

---

## (c) Concept overload

### C1. `doctor` counts but doesn't triage — and inflates its warnings

*(Correction to the first draft of this audit: doctor DOES end with a
"N error(s), M warning(s)." line and a hidden-sections note — the original
walkthrough piped through `head` and truncated them. The real gaps are the
two below.)*

Doctor's individual lines are excellent (every ⚠/✗ has a `↳ fix`), and it
closes with honest counts. But (1) it never says which fix to start with —
"1 error(s), 22 warning(s)." leaves the newcomer scrolling back through ~45
lines across 9 sections to find the ✗; and (2) the 22 is inflated: ⚠ "Cursor
not detected" fires for every CLI the user doesn't have, which isn't a fault
at all.

Compounding it: a *project* doctor surfaces **machine-global drift** — in the
test project, 15 warnings about the user's other global CLI configs
("Claude Code would REMOVE kibana_mcp, figma, …") drowned out the two lines
that were actually about this project (untrusted, missing secret).

**A great CLI ends with:**

```
1 ✗  2 ⚠  (12 machine-wide notes hidden — agentstack doctor --all)
  first: agentstack secret set DEMO_TOKEN
```

### C2. 18 visible top-level commands; a newcomer needs ~6

`--help` already has a good one-line identity, a "Start here" footer, and hides
14 commands into grouped one-liners — genuinely better than most. But the
visible list is still 18 rows of undifferentiated table: `instructions`,
`lock`, `lib`, `adopt`, `dashboard`, `report`, `guard` sit visually equal to
`init`. The newcomer set is roughly: `init`, `add`/`search`, `apply`, `doctor`,
`run`, `secret`, `trust`. Grouping the visible list by task (Set up / Edit
stack / Protect / Run / Inspect) — the same trick the footer already uses —
would let the eye skip whole groups.

### C3. `agentstack status` doesn't exist, and did-you-mean sends you wrong

`status` is the single most-guessed command in any CLI ecosystem
(git/docker/systemctl muscle memory). Today:

```
$ agentstack status
error: unrecognized subcommand 'status'
  tip: some similar subcommands exist: 'setup', 'settings', 'trust'
```

None of the three suggestions is the answer, and the first one (`setup`) is a
hidden legacy command. The right answer — the no-args orientation screen and/or
doctor — is never mentioned. (See D1.)

### C4. Three names for the thing you run, in the same help output

The CLI's own help calls the same object a **CLI** (orientation: "8 detected"),
a **harness** (`run <HARNESS>`), an **adapter** (`adapters list`, "Harness/
adapter id"), and a **target** (`apply --target`, "Applied to 1 target(s)").
Doctor headlines "Adapters & CLIs". Each name is locally defensible (target =
adapter × scope, harness = the running process) but a newcomer meets all four
in their first ten minutes with no glossary line anywhere. One primary
user-facing word (suggest: **CLI** everywhere a human reads it, keeping
`--target` as the flag) plus one glossary line in `--help` would collapse this.

*(Docs-side terminology findings: see section (e).)*

---

## (d) Missing affordances

### D1. A `status` command (alias territory)

The no-args orientation screen *is* a status screen — it just isn't reachable
by name, so nobody typing `agentstack status` (or being in a script, where
no-args can't be used as "status") finds it. Wire `status` as a visible alias
of the orientation view, extended with the doctor tail-line from C1 (trusted?
secrets ok? drift?). One screen: what's configured, what's trusted, what's out
of sync, fix command per warning — which is the screen newcomers actually want
hourly, whereas full `doctor` is the weekly deep-scan.

### D2. Successful writes never mention `restore`

`apply --write` ends with "Applied to 1 target(s). → Restart or reopen your
agent CLI(s)…" — good, but the single best trust-builder a config-mutating tool
can print is the undo: `undo: agentstack restore`. The restore machinery
exists and is solid (13 snapshots listed in the test run); it's just never
advertised at the moment of mutation. Same for `use --write` and `init`'s
final screen.

### D3. Warnings never say when they're ignorable

Doctor ⚠s for undetected CLIs ("Antigravity not detected") read as problems.
A trailing `(ok if you don't use it)` — or demoting not-detected to the
`--all` view since detection status isn't actionable — would cut perceived
warning count by more than half on a typical machine.

### D4. `doctor` vs hidden `setup` overlap

`setup` (hidden) prints a near-subset of doctor's Adapters section. It only
surfaces via the wrong did-you-mean (C3). Fold it into doctor/status or mark
it deprecated-hidden so suggestion machinery skips it.

---

## (e) Docs & terminology

From a sweep of README.md, docs/reference.md, docs/ARCHITECTURE.md, the HTML
docs pages, and the clap strings in `crates/cli/src/cli.rs`.

### E1. Four names for the thing you run — evenly split, no glossary

Confirming C4 across the docs: reference.md uses **harness** (42×), **CLI**
(23×), **target** (22×), and **adapter** (22×) with no dominant term and no
sentence explaining the distinction. The collision reaches single doc comments:
`cli.rs:744-746` has the `--target` flag with `value_name = "CLI"` and a
description ending "Unknown **adapter** ids are an error." The distinction is
real (target = manifest fan-out key, adapter = the compiler, CLI/harness = the
program) but nowhere stated. Fix: one primary human-facing word (**CLI**), keep
`--target`/`adapters` as technical identifiers, add a one-line glossary to
`--help` and reference.md.

### E2. "Gateway" in the docs, "bridge" in the tool's own output

The command is `agentstack gateway connect`, README and reference.md teach
"gateway" — but the runtime output the user actually sees says **bridge**:
`✓ bridge registered (agentstack mcp --auto-project)` (`connect.rs:90`), and
doctor's section is "Zero-files bridge". A newcomer who read the docs greps
their terminal for "gateway" and finds nothing. Also: the reference.md section
anchor is singular "zero-**file** bridge" (`reference.md:57,1068`) while every
other occurrence is "zero-**files**". Pick one compound noun and use it in
both prose and printed output.

### E3. "Policy presets" exists only on the marketing pages

The same `examples/policies/` artifact is "Starter machine policies"
(README:247), "Ready-to-use machine policies" (reference.md:954), and "Policy
presets" (start.html, docs.html, index.html, examples.html). "Preset" never
appears in the README or reference — a term coined only on the HTML pages.

### E4. README concept load: ~29 concepts before the end

A top-to-bottom README read introduces roughly 29 distinct concepts (manifest,
trust, firewall, capability, machine policy, lockfile, guard, profile, gateway,
the target/adapter/CLI triple, 3 delivery modes, 2 scopes, 6 capability kinds,
`${REF}` secrets, drift, adopt, locked runs, posture, packs, wire proxy,
sandbox, lockdown, …), most in a single dense sentence each, no glossary. The
"Try it in 60 seconds" block itself is fine (install → `init` → `doctor`, 2
commands, matching the CLI's own recommendation); the overload is everything
after it.

### E5. Docs teach hand-editing where a command exists

reference.md's "Native settings" section (:670-676) teaches editing
`[settings.<cli>]` blocks by hand and never mentions `agentstack settings
set/unset` — the command that exists precisely to avoid that. It only appears
in the generated all-commands appendix. Similarly the README's manifest tour
has no forward pointer to `agentstack add server` for readers extending by
hand.

### E6. Clean bills of health (no fix needed)

- No nonexistent commands/flags anywhere in the docs — everything
  cross-checked against the clap definitions matches.
- manifest vs config is used *consistently* (manifest = agentstack.toml,
  native config = rendered output).
- profile is never confused with preset.
- ARCHITECTURE.md's "bundle" vs README's "manifest" is a mild split — one
  bridge sentence in ARCHITECTURE.md would close it.

---

## Walkthrough log (evidence)

| # | Command | Context | Result quality |
|---|---------|---------|----------------|
| 1 | `agentstack` | empty dir | ✅ orientation + one next step, 11 ms |
| 2 | `--help` | — | ✅ identity line, Start-here, grouped hidden cmds; 18 visible rows (C2) |
| 3–4 | `apply` / `doctor` | no manifest | ⚠ right next-step, but B1 (os-error leak, wrong path) |
| 5–6 | `aply` / `--wirte` | typos | ✅ did-you-mean both |
| 7 | `init` | stdin closed | ✅ safe refusal, exit 1; dense wording (A3) |
| 8 | `status` | — | ❌ C3/D1 |
| 9 | `trust .` | no manifest | ✅ clean error, next step |
| 10 | `run claude` | no manifest | ❌ A1 banner-before-error |
| 12 | `init --dry-run` | real machine | ✅ detection/import/secret-lift excellent; B4 dup warnings |
| 13 | `apply` | manifest, no `[targets]` | ⚠ A2 13-target fan-out; ✗ lines lack fix (B3) |
| 14 | `doctor` | same | ⚠ C1 45 lines, no verdict, global drift drowns project state |
| 28 | `apply --write` | unresolved secret | ✅ fail-closed per target; B2 triple summary; exit 1 |
| 29 | `use` | dry-run | ✅ "Re-run with --write"; B3 |
| 30 | `explain nosuchthing` | — | ✅ model error message |
| 33 | `run nosuchcli` | — | ❌ A1 |
| — | `apply --write` | clean manifest | ✅ diff → ✓ → restart hint; D2 no undo hint |

---

# Independent second pass — additional findings

This pass was run after the audit above, specifically to look for things the
first pass missed. It uses the committed `HEAD` at `8ee6469` from an isolated
clone, not the changing uncommitted worktree, and a fresh `HOME` plus disposable
project directories. `cargo build --release -p agentstack` succeeds for that
commit. A sandbox-enabled build (`--features sandbox`) also succeeds.

The active worktree gained uncommitted CLI changes while this audit was running
and temporarily did not compile. Those edits are user-owned work in progress;
they were not changed, and that transient failure is not ranked as a shipped
AgentStack defect.

These findings supplement the earlier ranking. Where this section says an
earlier observation is stale, this section is authoritative for `8ee6469`.

## (a) Additional time-to-first-success blockers

### A0. Semantic manifest errors make `apply --write` report success

A syntactically valid but semantically invalid manifest is correctly kept off
disk, but the command exits 0 and ends with an impossible instruction. Example:

```toml
version = 1

[servers.demo]
type = "http"
# url is missing
```

Current output, abbreviated only by removing nine repeated target previews:

```text
$ agentstack apply --write
✗ server 'demo' is type=http but has no `url`

✗ manifest has validation errors — not writing. Fix them first.
...
9 target(s) would change. Re-run with --write to write.
$ echo $?
0
```

No native files were written, which is the correct fail-closed behavior. The DX
failure is that scripts and humans are told the failed operation succeeded, and
the prescribed next command is the command they just ran. Dry-run `apply` also
exits 0 and previews malformed native config (`{}` for several adapters).

**A great CLI prints:**

```text
error: manifest is invalid; nothing was written

  server 'demo' uses HTTP but has no URL
  fix: add `url = "<SERVER_URL>"` under [servers.demo] in agentstack.toml

Then run: agentstack apply --write
```

Exit 1 before rendering target previews. `doctor` may continue to show all
diagnostics, but a write command must not have a successful exit status after
refusing the write.

### A1. Bare `run --plan` launches the CLI instead of planning

`--help` describes `--plan` as printing a plan and exiting **without running
anything**. In reality it is only consulted inside the `--locked` and
`--sandbox` branches. Used by itself, it is silently ignored and host launch
continues:

```text
$ agentstack run --plan claude-code
⚠ host mode: policy is advisory ...
▶ launching claude-code…
  posture: HOST / ADVISORY
Error: Input must be provided either through stdin or as a prompt argument when using --print
$ echo $?
0
```

The final error is from the launched Claude binary, and AgentStack still exits
0 for this observed child result. This violates the strongest promise made by
the flag and can cause an unintended process launch during a read-only review.

**A great CLI either makes bare `--plan` useful or rejects it before launch:**

```text
error: --plan needs a run mode; nothing was launched

  Protected host plan: agentstack run --locked --plan claude-code
  Sandbox plan:        agentstack run --sandbox --plan claude-code
```

### A2. A mistyped target is a successful no-op, despite help promising an error

Current `apply --help` says: “Unknown adapter ids are an error.” Actual output:

```text
$ agentstack apply --target codx
Scope: project
⚠ unknown adapter 'codx' — skipping

0 target(s) would change. Re-run with --write to write.
1 issue(s) — resolve before writing.
$ echo $?
0
```

This is especially costly in automation: a typo can skip the intended CLI and
still pass CI. Command and flag typos have good Clap suggestions; value typos do
not.

**A great CLI prints:**

```text
error: unknown CLI 'codx'
  did you mean 'codex'?

Fix: agentstack apply --target codex
```

### A3. The orientation screen and renderer disagree about zero targets

For a hand-written manifest with no `[targets]` block:

```text
$ agentstack
Manifest  .../agentstack.toml — 1 server(s) → 0 target(s)
```

But `agentstack apply` immediately fans out across all 13 adapters (nine
project-scope previews and four “no project scope, skipping” lines). This is a
second symptom of the earlier A2 default-fan-out issue: even the one-screen
orientation view cannot predict what the next command will do.

**A great CLI uses one target resolver everywhere:** detected CLIs by default,
with the same count and the same explanatory line in overview, apply, use, and
doctor.

## (b) Additional confusing errors

### B5. Missing Docker asks a question but gives no fix command

From the sandbox-enabled binary with an unavailable Docker socket:

```text
error: cannot reach Docker (sandbox backend: Socket not found: .../no-docker.sock) — is the daemon running?
```

This correctly validates before printing a sandbox launch banner, but it does
not meet the CLI's own “every warning names its fix” standard.

**A great CLI prints:**

```text
error: cannot reach Docker; nothing was launched

  Start Docker Desktop (or your Docker daemon), then verify: docker info
  Retry: agentstack run --sandbox claude-code
```

### B6. Keychain failures duplicate internals and omit alternatives

With no default macOS keychain:

```text
$ agentstack secret get DEMO_TOKEN
error: reading secret 'DEMO_TOKEN' from keychain: Platform secure storage failure: A default keychain could not be found.: A default keychain could not be found.
```

The OS cause is repeated, and there is no command showing the supported
project-`.env` fallback.

**A great CLI prints:**

```text
error: DEMO_TOKEN is not available from the OS keychain

  Store it in the keychain: agentstack secret set DEMO_TOKEN
  Or in this project's .env: agentstack secret set DEMO_TOKEN --env-file
```

### B7. Mixed stdout/stderr can put remediation after the error

In non-interactive `trust .`, buffered normal output and unbuffered error output
interleave so the command prints the final machine-policy explanation *after*
the refusal:

```text
error: refusing to trust: stdin is not a terminal ...
  machine policy ceiling: ... the repo can only narrow it, never loosen it
```

The same stream split can put the host-mode warning above the “launching” line.
User-facing narrative output should use one ordered stream, reserving stderr for
the final error if necessary.

## (c) Additional concept overload

### C5. `--help --all` is not progressive disclosure; it is identical to `--help`

Both commands exit 0 and print the same 18 visible commands plus grouped names
for hidden commands. There is no full detailed command inventory reachable by
the requested spelling.

Also, the first help line says what AgentStack does but does not include the one
start command; `agentstack init` appears much later in a footer. The desired
contract is explicit and testable:

```text
AgentStack manages agent CLI capabilities from one manifest. Start: agentstack init
```

Default help should show only the beginner set, grouped by task, and
`--help --all` should show every command with its summary. Snapshot-test that
the two outputs differ.

### C6. Empty-machine `init` asks five decisions and introduces ~20 concepts

From install to full “Setup complete” is two shell commands, but the empty-home
wizard requires these decisions:

1. confirm import/write;
2. choose `static`, `clean-at-rest`, or `zero-files`;
3. confirm apply (even when “0 target(s) would change”);
4. accept/decline a machine-global Guard install;
5. accept/decline machine-global House rules.

Before completion, a newcomer meets CLI, config, token, `${REF}`, manifest,
adapter, skill, secret, delivery mode, static, clean-at-rest, session,
zero-files, gateway, trust, target, scope, policy, guard, hook, machine
manifest, and `CLAUDE.md`/`AGENTS.md`.

The final summary and undo command are excellent. The overload is in making two
machine-global products part of every project init after the selected setup has
already applied zero targets. Move Guard and House rules behind one optional
“Set up machine-wide protection too?” step, or make them the next recommended
command after project success.

## (d) Additional missing affordances

### D5. Validation errors have no edit affordance

`doctor` can identify the exact bad field, but the user must hand-edit TOML and
already know its shape. `add server` refuses a missing URL with only:

```text
error: http server needs --url
```

At minimum, print the complete retry skeleton:

```text
Fix: agentstack add server <NAME> --url <URL> --write
```

Longer term, an idempotent `agentstack set server ...` verb (with `add` retained
as an alias where compatible) would give validation errors a safe,
copy-pasteable repair path.

## (e) Additional docs and terminology contradictions

### E7. The README's security headline contradicts runtime and the enforcement matrix

README opening:

```text
Nothing runs until it's trusted, and nothing trusted runs unobserved.
```

Observed behavior: `agentstack run claude-code` launches from an untrusted
project in `HOST / ADVISORY`. The authoritative enforcement matrix says host
audit/recording is `unsupported`, and explicitly says an untrusted project does
not block a manual host run or explicit static `apply`.

This is not merely tone; it makes a security promise the product deliberately
does not enforce. Keep the strong but precise claim already used in
`ENFORCEMENT.md`, for example:

```text
Untrusted project declarations are not auto-activated. Governed gateway and
contained runs enforce and record the controls their printed posture names.
```

### E8. README says mode choice happens before any write; the wizard writes first

README Step 1 says the wizard asks for a delivery mode “before it writes
anything.” Actual order:

```text
Import now? ... [y/N] y
✅ Wrote .../.agentstack/agentstack.toml
✓ Manifest validates
...
? Pick a delivery mode
```

Either defer the manifest write until after the mode choice, or correct the
docs to distinguish the confirmed import write from later native-config writes.

### E9. The 60-second path is interactive-only but is written as universal

README's quick start annotates `agentstack init` as “previewed and applied.”
With closed stdin, flagless init refuses; the promptless
`init --secrets skip` path only writes the manifest and ends with “Next: review
the manifest, then `agentstack apply`.” The docs should label the two-command
snippet “interactive” and put the safe scripted equivalent immediately below
it.

## Corrections to the earlier audit

- Earlier C1 (“doctor has no verdict”) is stale for `8ee6469`: default doctor
  hides unused sections and ends with `N error(s), N warning(s)`. Keep that.
- Earlier B3 is partly stale: current `apply` missing-secret lines include the
  specific `↳ run agentstack secret set DEMO_TOKEN, then re-run` fix. The final
  blocked-write summary still regresses to `<NAME>` and repeats itself (B2).
- Clean committed `HEAD` builds. The active worktree build failure observed
  during this pass belongs to uncommitted concurrent work and is not a baseline
  product finding.

## Second-pass walkthrough additions

| Command | Context | Result quality |
|---|---|---|
| `cargo build --release -p agentstack` | isolated `HEAD` clone | ✅ succeeds |
| `cargo build --release -p agentstack --features sandbox` | isolated `HEAD` clone | ✅ succeeds |
| `apply --write` | semantic-invalid manifest | ❌ refuses write but exits 0 and says retry `--write` |
| `apply --target codx` | valid manifest | ❌ successful no-op; no value suggestion |
| `run --plan claude-code` | valid, untrusted manifest | ❌ launches despite “without running anything” promise |
| `run --sandbox claude-code` | Docker unavailable | ⚠ validates early, but no exact fix command |
| `--help --all` | empty directory | ❌ byte-for-byte same content as abbreviated help |
| `init` | empty HOME, interactive | ⚠ completes with undo; five decisions, two global upsells |
| `agentstack` then `apply` | no `[targets]` | ❌ overview says 0 targets; apply fans out to all 13 |
| `secret get DEMO_TOKEN` | no default keychain | ❌ duplicated platform error, no fallback command |

---

# Before / after — the fix round (2026-07-20)

All ten planned fixes (P1–P10) landed, plus four of the second-pass findings
(A0, bare `run --plan`, the `--target` typo no-op, the Docker fix hint). The
newcomer walkthrough was re-run on the rebuilt binary; every block below is
actual output.

## First-contact error (B1) — before → after

```
error: no manifest here (looked for /path/to/dir/agentstack.toml) — run `agentstack init` to create .agentstack/agentstack.toml, or point at one with --manifest-dir: No such file or directory (os error 2)
```

```
error: no agentstack manifest in /path/to/dir
(a project keeps one at .agentstack/agentstack.toml, or agentstack.toml at the repo root)

  create one here:   agentstack init
  or point at one:   agentstack --manifest-dir <dir> <command>
```

## `run` validation (A1) — before → after

Before: `▶ launching nosuchcli…` + 3-line posture banner, then the error.
After — nothing "launches" until validation passes, and the error carries the
valid ids:

```
error: unknown CLI 'nosuchcli' — valid ids: antigravity · claude-code · claude-desktop · codex · copilot-cli · cursor · gemini · junie · kiro · opencode · pi · vscode · windsurf (details: `agentstack adapters list`)
```

## `doctor` triage (C1/D3) — before → after

Before: `1 error(s), 22 warning(s).` with no starting point; "not detected"
warned for 8 absent CLIs; machine-global drift printed as 15 project warnings.
After — same project, same machine:

```
1 error(s), 3 warning(s).
  start with: agentstack secret set DEMO_TOKEN
```

"Not detected" rows and foreign-manifest drift are now dimmed `·` info lines
(shown, counted in neither total — nothing is hidden, it just stops shouting).

## `status` (C3/D1) — new

`agentstack status` now exists: the orientation screen by name, plus a Secrets
line (`✗ DEMO_TOKEN not set   fix: agentstack secret set DEMO_TOKEN`) and the
`Deep check: agentstack doctor` pointer. The did-you-mean dead end is gone.

## Target fan-out (A2 + second-pass A3) — before → after

Before: no `[targets]` → all 13 adapters, `.cursor/`/`.junie/` dirs created,
"9 blocked"; orientation said "→ 0 target(s)" while apply fanned out to 13.
After — one resolver everywhere:

```
Scope: project
Targets: 8 detected CLI(s) — no [targets] in the manifest; pin the list with `agentstack init` or a [targets] block
```
```
Manifest  …/agentstack.toml — 1 server(s) → 8 detected CLI(s), no [targets] pinned
```

## Blocked write (B2/B3/B6-adjacent) — before → after

Before: three stacked summaries ending in a `<NAME>` placeholder.
After — per-target ✗ lines carry `↳ agentstack secret set DEMO_TOKEN`, and the
tail is two lines, the second naming the exact command:

```
Wrote 0 of 5 target(s); 5 blocked by unresolved secret(s) or missing fragment source(s); see ✗ above.
error: blocked write(s) on 5 target(s) — fix: agentstack secret set DEMO_TOKEN (or pass --allow-unresolved)
```

## Undo affordance (D2) — after

Every successful `apply --write` / `use --write` now ends with
`undo: agentstack restore` — and `use --write` now actually records restore
history for the server configs it writes (it previously recorded nothing, so
the hint would have been a lie).

## init (B4/A3) — before → after

The triple `⚠ server 'tldraw' differs between CLIs` collapsed to one line with
a count; the non-TTY refusal is now one reason line plus three copy-pasteable
options (`--dry-run` first).

## Help surface (C2/C4/E1/E2) — before → after

`--help` now shows 9 visible commands (init · status · add · search · apply ·
doctor · use · run · trust) and a complete task-grouped map of every command
(Set up / Edit / Render / Protect / Run / Inspect) plus a one-line glossary
(CLI vs adapter vs [targets]). Printed output now says **gateway** (matching
the `gateway` command and the docs) — `✓ gateway registered`, doctor section
"Zero-files gateway" — and **CLI** where "harness" was a plain synonym. Docs:
reference.md gained the glossary, the zero-file(s) anchor is unified, "Policy
presets" on the HTML pages became "Starter machine policies", the settings
section teaches `settings set`, and the README points at `add server`.

## Second-pass fixes verified

```
$ agentstack apply --write        # semantically invalid manifest
5 target(s) would change — fix the ✗ validation error(s) above before writing.
error: manifest has validation errors — nothing was written; fix the ✗ above, then re-run `agentstack apply --write`
(exit 1; dry-run keeps exit 0 but no longer says "Re-run with --write")

$ agentstack apply --target codx
error: unknown CLI 'codx' — did you mean 'codex'? (`agentstack adapters list` shows all ids)   (exit 1)

$ agentstack run --plan claude-code
error: --plan needs a run mode — nothing was launched
  protected host plan:  agentstack run --locked --plan claude-code
  sandbox plan:         agentstack run --sandbox --plan claude-code   (exit 1)
```

Docker-unreachable now prints "nothing was launched" + `docker info` + retry
guidance (B5).

## Remainder round (same day, after the second-pass report)

- **B6 fixed** — `secret get` on a broken keychain now names the cause once
  and both stores: `agentstack secret set NAME` and `agentstack secret set
  NAME --env-file` (the flag already existed; the audit's wished-for fallback
  was real). The low-level keychain read also stopped double-printing its
  platform cause.
- **B7 fixed** — `main` flushes stdout before printing the final error, so
  piped narrative can no longer land after the refusal it explains.
- **C5 fixed** — `agentstack --help --all` is now a real, different view:
  every command (hidden and nested included) with its one-line summary,
  advertised from the short help's footer. Snapshot-style test asserts the
  two outputs differ and that hidden commands appear.
- **E8/E9 fixed** — README now says the mode choice comes after the confirmed
  import writes the manifest but before any native config; the 60-second block
  is labeled interactive with the scripted `init --secrets skip` +
  `apply --write` path directly below it.

## Decision round (maintainer approved all three)

- **E7 fixed** — README and index.html headline now makes the precise claim:
  untrusted declarations are never auto-activated, and governed gateway /
  contained runs enforce and record exactly what their printed posture names.
  No more implied host-mode enforcement.
- **D5 fixed** — `agentstack set server <name> …` (hidden, in the help map's
  Edit group) is the idempotent create-or-update: same flags as `add server`,
  rewrites an existing entry in place. `add server` on an existing name now
  points at it, and the missing-`--url`/`--command` validation errors carry
  the complete retry skeleton.
- **C6 fixed** — the wizard's two machine-global upsells (Guard, House rules)
  collapsed into ONE optional "machine-wide protection" step after the
  project's own setup finishes, naming only what's still missing; declining
  prints each manual command. The "Apply this setup?" confirm is skipped when
  zero targets would change ("Configs already match the manifest").

## Verification

`cargo fmt --check` clean · `cargo clippy --workspace --all-targets -D
warnings` clean · **812 tests pass** (cli + core, including new witnesses:
missing-manifest error shape, doctor `first_fix`/Info levels, help surface +
`status` parse, `--target` typo suggestion) · sandbox-feature build compiles ·
docs command inventory regenerated (`self docs --write`).

---

# Third pass — fresh newcomer walkthrough at HEAD `6570db2` (2026-07-20)

Run against a debug build of the committed HEAD, with a fresh `HOME`, an empty
directory, a hand-written minimal project, an invalid manifest, typos, and
closed stdin. Environment caveat: the fresh `HOME` makes the macOS keychain
*unreadable* ("A default keychain could not be found"), so secret failures in
this pass are read-failures, not not-found — noted where it changes a verdict.

**Headline: the fix rounds held.** Every fix the before/after section claims
was re-verified live on HEAD: the missing-manifest error (all commands,
consistent, exit 1), the init refusal layout, `--plan` guard, `--target codx`
did-you-mean, semantic-invalid `apply --write` exit 1, `secret get` fallback
listing both stores, `status` by name with the Secrets fix line, one target
resolver in orientation *and* apply, `--help` at 9 visible commands with the
task map + glossary, a genuinely different `--help --all`, doctor's
`start with:` triage line, `trust --yes` → `run --locked --plan` →
"✓ live launch would proceed". The findings below are what's left.

## Ranked findings

### T1. Broken pipe = Rust panic: exit 101 + panic spew (b: confusing errors)

The single worst remaining first-session experience. Any command with more
output than the terminal consumes — `agentstack diff | head`, `doctor | less`
(quit early), any script piping to `grep -m1` — dies as:

```
$ agentstack diff | head -3
…3 lines…
thread 'main' (92101784) panicked at …/library/std/src/io/stdio.rs:1165:9:
failed printing to stdout: Broken pipe (os error 32)
note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
$ echo $?   # of agentstack
101
```

A security tool printing a panic backtrace note on a routine pipe reads as
"this tool crashes." **A great CLI:** exits silently on SIGPIPE like every
Unix tool (ripgrep, fd, git). Fix in `cli/src/sys.rs` (the designated libc
call-site module — `SigintGuard` already lives there): restore `SIG_DFL` for
`SIGPIPE` at the top of `main`.

### T2. `secret set` without a TTY leaks a raw OS error (b)

```
$ agentstack secret set DEMO_TOKEN </dev/null
error: reading secret from prompt: Device not configured (os error 6)
```

CI and scripts hit this on their first secret. The flags that solve it
(`--value`, `--env-file`) exist but are never mentioned. **A great CLI**
detects the missing TTY before prompting:

```
error: secret set needs a terminal to prompt for the value

  pass it inline:            agentstack secret set DEMO_TOKEN --value <VALUE>
  (…lands in shell history — prefer the prompt or an env-managed store)
```

### T3. Doctor's `start with:` recommends a command that will refuse to run (b)

With an invalid manifest (`[servers.demo]` type=http, no url):

```
Manifest
  ✗ server 'demo' is type=http but has no `url`
Drift
  ⚠ Claude Code    1 change(s) pending ↳ agentstack apply --write
…
1 error(s), 4 warning(s).
  start with: agentstack apply --write
```

`apply --write` refuses while the manifest is invalid — the triage line sends
the newcomer into a wall. Two gaps: the validation ✗ carries no `↳ fix` (so
it can't win), and the first-fix picker doesn't prefer errors over warnings.
**A great CLI prints:** `start with: agentstack set server demo --url <URL>
--write` (the repair skeleton `add server` already prints).

### T4. Non-TTY `init` advice misleads when a manifest already exists (d)

Interactive `init` in an existing project correctly *resumes* the wizard
(import is skipped — `setup.rs` checks `manifest_path.exists()`). But the
non-TTY refusal prints the same three options regardless of state, and in an
existing project two of them mislead: `init --yes` hits `error: …
agentstack.toml already exists — use --force to overwrite`, and `init
--dry-run` previews a **from-scratch import** — in the test project the
preview showed an empty `[servers]`, silently dropping the existing server,
with no hint that writing it would need `--force` and would replace the file.
**A great CLI:** when a manifest exists, the non-TTY refusal recommends the
actual scripted next steps (`agentstack apply --write`, `agentstack use
<profile> --write`), and `init --dry-run` opens with `existing manifest at …
— this preview shows a fresh re-import (writing it requires --force and
replaces the file)`.

### T5. Per-target secret read-failures stack three layers of context (b)

`apply`/`use` ✗ lines on a broken keychain:

```
✗ secret read failed DEMO_TOKEN (server 'demo') — keychain read failed: reading secret 'DEMO_TOKEN' from keychain: A default keychain could not be found. ↳ run `agentstack secret set DEMO_TOKEN`, then re-run
```

The name appears twice and "keychain read failed: reading secret … from
keychain" says the same thing twice ("`secret get`'s cause was deduplicated
in the remainder round; this sibling path wasn't"). One cause is enough:
`✗ DEMO_TOKEN unreadable (server 'demo') — a default keychain could not be
found ↳ …`.

### T6. `--help --all` leaks internal ticket ids (c)

`setup            Hidden alias of interactive `init` (P27: one front-door
verb)` — P-numbers are audit-internal shorthand, meaningless to users. Sweep
user-visible clap strings for `P\d+`.

### T7. Blocked-write tail goes vague exactly when the keychain breaks (b, nit)

The tail names exact commands for *missing* secrets, but read-*failures* fall
back to "each ✗ above names the blocker" even when every ✗ names the same
one command. Naming commands whenever the distinct fix set is small (regardless
of failure kind) would keep the last line copy-pasteable in both cases.

## Third-pass walkthrough log

| # | Command | Context | Result |
|---|---------|---------|--------|
| 1–2 | no-args / `status` | empty dir, fresh HOME | ✅ orientation, one next step |
| 3–4 | `--help` / `--help --all` | — | ✅ distinct views, task map, glossary; T6 P27 leak |
| 5–6, 9–10, 14, 16 | `apply` `doctor` `run` `explain` `add server` | no manifest | ✅ one consistent error, exit 1 |
| 7–8 | `aply` / `--wirte` | typos | ✅ did-you-mean |
| 11 | `run --plan` | bare | ✅ refuses, names both modes |
| 12–13 | `trust .` / `init` | non-TTY | ✅ ordered refusals |
| 15 | `secret get` | broken keychain | ✅ both stores named |
| 17–18 | orientation / `apply` | manifest, no `[targets]` | ✅ same resolver, same count |
| 19 | `apply --write` | unreadable secret | ✅ fail-closed, exit 1; T5, T7 |
| 20 | `apply --target codx` | typo | ✅ did-you-mean, exit 1 |
| 21 | `apply --write` | invalid manifest | ✅ exit 1, nothing written |
| 22 | `doctor` | invalid manifest | ❌ T3 `start with: apply --write` |
| 25–26 | `add server` | missing url / dup name | ✅ retry skeleton, points at `set server` |
| 29, 31, 43 | `init` variants | existing manifest | ⚠ T4 (interactive resume ✅, `--yes` walled, `--dry-run` misleads) |
| 33, 46 | `trust .` / `--yes` → plan | manifest | ✅ declarations → refusal / "live launch would proceed" |
| 35–36 | `restore` / `remove nosuch` | — | ✅ friendly |
| 37 | `secret set` | non-TTY | ❌ T2 raw os error 6 |
| 40–41 | `diff \| head` | — | ❌ T1 panic, exit 101 |

---

# Before / after — the third-pass fix round (2026-07-20, same day)

All seven findings (T1–T7) fixed and re-verified on the rebuilt binary; every
block below is actual output from the same scenarios that produced the
findings.

## T1 — broken pipe

Before: 3 lines of output, then `thread 'main' panicked … Broken pipe (os
error 32)` + the RUST_BACKTRACE note, exit 101. After — `agentstack diff |
head -3` prints 3 lines, empty stderr, silent exit (the Unix default).
`main` restores `SIGPIPE`'s default disposition via `sys.rs` — the designated
libc module — and an integration test pre-closes the pipe before spawn so the
first write deterministically hits it.

## T2 — `secret set` without a terminal

```
error: secret set needs a terminal to prompt for the value

  pass it inline:  agentstack secret set DEMO_TOKEN --value <VALUE>
  (inline values can land in shell history — prefer the prompt when you can)
```

(was: `error: reading secret from prompt: Device not configured (os error 6)`)

## T3 — doctor triage on an invalid manifest

```
Manifest
  ✗ server 'demo' is type=http but has no `url` ↳ agentstack set server demo --url <URL> --write
…
1 error(s), 4 warning(s).
  start with: agentstack set server demo --url <URL> --write
```

(was: `start with: agentstack apply --write` — a command the error itself
blocks.) Validation issues now carry a `fix` field printed in the `↳` voice by
doctor, `apply`, and the wizard; and `first_fix` never falls through to a
warning's fix while an error is outstanding — no triage line beats a
misleading one.

## T4 — scripted `init` in an initialized project

Both the flagless non-TTY path and `init --yes` now land on one adapted
refusal (was: generic advice whose options walked into the `--force` wall):

```
error: …/.agentstack/agentstack.toml already exists — init has nothing left to do here

  render it into your CLIs:  agentstack apply --write
  activate a profile:        agentstack use <profile> --write
  re-import from scratch:    agentstack init --force   (replaces the manifest)

(in a terminal, plain `agentstack init` resumes the wizard: preview, apply, verify)
```

And `init --dry-run` opens with the banner that was missing:

```
⚠ existing manifest at … — this preview shows a fresh re-import, not the file
on disk; writing it takes `agentstack init --force` and replaces the manifest
```

## T5 — secret read-failure context, said once

```
✗ secret read failed DEMO_TOKEN (server 'demo') — keychain read failed: A default keychain could not be found. ↳ run `agentstack secret set DEMO_TOKEN`, then re-run
```

(was: `— keychain read failed: reading secret 'DEMO_TOKEN' from keychain: A
default keychain could not be found.` — name twice, store thrice.) The fix is
structural: `keychain::get` keeps its context and the platform root as two
error layers instead of flattening them into one string, so every downstream
`root_cause()` gets the bare platform sentence. `secret get`'s parenthetical
dropped its duplicate the same way.

## T6 — help hygiene

`--help --all` now contains zero P-numbers (`setup  Hidden alias of
interactive `init` — same guided wizard, older name`), and the inventory test
asserts no `P<digit>` ever returns to either help view.

## T7 — blocked-write tail on a broken keychain

```
error: blocked write(s) on 4 target(s) — fix: agentstack secret set DEMO_TOKEN (or pass --allow-unresolved)
```

(was: `— each ✗ above names the blocker`.) Read-failures now feed the same
fix-command set as missing secrets in both `apply` and `use`.

## Verification

`cargo fmt --check` clean · `cargo clippy --workspace --all-targets -- -D
warnings` clean · **736 cli-crate tests pass**, including the new witnesses:
pre-closed-pipe no-panic, `secret set` non-TTY error shape, `first_fix`
no-fall-through, scripted-init adapted refusal (flagless + `--yes`, manifest
untouched), P-number help guard · docs command inventory regenerated
(`self docs --write`).
