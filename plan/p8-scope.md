# P8 — scope, with evidence

Scoped 2026-08-06 against `target/release/agentstack` 0.18.0-rc.2, on macOS,
in isolated `HOME`/`AGENTSTACK_HOME` temp dirs. Read-and-plan only: nothing in
the codebase was changed to produce this. Raw commands, output and exit codes
are in the appendix.

## The headline

P8's premise does not survive contact with the binary.

The record says `run` and `workflow` "could not be exercised in an isolated
environment", and that six journey screens are "sketches, never observed". I
ran all six today. **All six rendered. None needed infrastructure this project
does not have.** Two of the three obstacles the record names were facts about
the reviewer's setup, not about the product:

- *"the debug build reports no sandbox support"* — correct, and irrelevant.
  The `sandbox` feature is off by default on purpose, but
  `.github/workflows/release.yml:115` builds published binaries with
  `--features sandbox`. A locally-built binary is the one artifact users never
  get. Building one here took **2 minutes 5 seconds**, after which
  `run --sandbox` and `run --lockdown` both ran green against this machine's
  Docker daemon.
- *"`workflow run` had nothing declared"* — correct for an empty project, and
  the repo ships `examples/workflow-acceptance/bundle`, a declared, pinned,
  trustable workflow. It ran end to end in 1.0 s against a ten-line fake
  `claude` on `PATH`. `crates/cli/tests/workflow_e2e.rs` has been doing exactly
  that in the default test suite for some time.

So P8 is not "make these verifiable". It is smaller and different: **most of
these screens are already witnessed; the remainder are cheap; and the one real
defect P8 should fix is not a screen at all — it is that the CI job guarding
the flagship security feature can pass while asserting nothing.**

## Verdict per screen

| # | Screen | Observed today? | Already witnessed? | What P8 still owes |
|---|---|---|---|---|
| 1 | `use` / `use --write` | yes, exit 0 | refusals and side effects: yes. Happy-path body: no | one screen assertion |
| 2 | `x up` | yes, exit 0 | **fully** — 6 tests assert the rendered text | nothing |
| 3 | `run` | yes, at five levels | **fully** — CI example + 5 CI sandbox e2e tests | nothing for the screen; see the CI gate below |
| 4 | `x workflow explain` | yes, exit 0 | `--json` twin only | one screen assertion |
| 5 | `x workflow report` | yes, exit 0 | **nothing at all** | one end-to-end witness |
| 6 | `x image` (plan and `--write`) | yes, both exit 0 | artifact and staged context: yes. Screen: no | one screen assertion |

"Witnessed" here means an automated assertion that fails when the thing
regresses. A screenshot is not that; where this document says *assertion* it
means an assertion.

## What the sandbox story really is

**The feature is off by default, in two crates, deliberately.**
`crates/cli/Cargo.toml:23` declares `sandbox = ["agentstack-runtime/docker",
"dep:agentstack-egress"]`; `crates/runtime/Cargo.toml` declares `default = []`
and `docker = ["dep:bollard", "dep:tokio", "dep:futures-util"]`. The stated
reason is to keep bollard and tokio out of ordinary builds. The binary is
honest about which build it is: `crates/cli/src/cli.rs:60-63` appends
`(sandbox: yes)` or `(sandbox: no)` to `--version`, and
`crates/cli/src/commands/sandbox.rs:445` carries the
`#[cfg(not(feature = "sandbox"))]` refusal that exits 1 before launching.

**Published binaries ship with it.** `.github/workflows/release.yml:115`:
`cargo build --release --locked --features sandbox --target …`. The refusal
message's claim — *"or install a published release binary — those ship with
it"* — is true. Anyone reproducing P8 against a locally-built binary is
testing a configuration no user has.

**CI already runs the sandbox suite against a real daemon.** `ci.yml` has a
dedicated `sandbox` job on `ubuntu-latest` (GitHub runners ship Docker) that
runs `cargo test -p agentstack-egress --test sidecar_image -- --include-ignored`
and then `cargo test -p agentstack --features sandbox` over
`sandbox_egress`, `sandbox_cli_e2e`, `sandbox_fs`, `sandbox_lockdown` and
`sandbox_gateway_e2e`. Four of those five drive the real `agentstack run
--sandbox` binary. This is substantially more `run` coverage than the record
credits.

**Is the gating honest, or a quiet skip? Both, in different places.**

The tests gate twice. First `#![cfg(feature = "sandbox")]` at file scope —
without the feature the file compiles to an empty test binary, which is honest
enough: it is a compile-time fact, and the `Clippy (enforcement features)` CI
step lints the gated code so it cannot rot silently. Second, at run time, a
`docker info` + `docker pull` probe:

```rust
fn docker_and_image() -> bool {
    …
    if !up { eprintln!("SKIP: no Docker daemon"); return false; }
    …
    if !pulled { eprintln!("SKIP: cannot pull {IMAGE}"); return false; }
    true
}

#[test]
fn cli_run_sandbox_blocks_denied_egress_and_records_it() {
    if !docker_and_image() { return; }
```

On a developer's Docker-less machine that is correct behaviour. **In the CI
`sandbox` job it is a hole.** There is no `AGENTSTACK_REQUIRE_DOCKER` or
equivalent anywhere in the workspace — I grepped; zero hits. So if the runner's
`docker pull` fails, and the realistic cause is Docker Hub's anonymous rate
limit on `curlimages/curl`, `busybox`, `node:22-slim` and `alpine:3`, then
every test early-returns, the job exits 0, and the job whose comment says *"a
regression that lets a sandboxed container reach a denied host … MUST fail
CI"* goes green having asserted nothing. Nothing in the workflow distinguishes
that outcome from a real pass. The same shape covers
`crates/cli/tests/packaging.rs:574` and `crates/egress/tests/sidecar_image.rs:54`.

**There is no Docker-less containment backend.** `crates/runtime/src/docker.rs`
is the sole implementation of the `Sandbox` trait
(`crates/runtime/src/sandbox.rs:33`), and every export of it in
`crates/runtime/src/lib.rs:31-42` is `#[cfg(feature = "docker")]`. No
seatbelt/`sandbox-exec`, no bubblewrap, no landlock. If the goal is kernel
containment, Docker is the only path and there is no honest substitute.

**But there is a Docker-less path that proves something real.**
`run --plan --sandbox` assembles and prints the entire sandbox plan — posture
label, workspace mount mode *and the policy reason for it*, egress mode, the
exact command — with neither the feature nor a daemon. Its own help calls this
out: *"Works without Docker or the `sandbox` feature."* It proves the decision,
not the enforcement. That is worth an assertion and worth saying plainly in
whatever P8 publishes: a plan is a claim about what would happen.

**One genuine hole found on the way.** `crates/runtime/tests/docker.rs` is
`#![cfg(feature = "docker")]`, and no CI job runs
`cargo test -p agentstack-runtime --features docker`. The `sandbox` job's
`-p agentstack --test sandbox_*` invocations build the runtime crate but do not
run *its* test targets. That file executes nowhere. Relatedly,
`crates/runtime/Cargo.toml:11-15` still says the Docker backend is
"compile-verified but not yet behavior-verified against a real daemon" — the
CI `sandbox` job has behavior-verified it, so that comment is stale and
understates what is enforced.

## What is already covered

Grepped before claiming any gap.

**`up` — fully witnessed, and the best-covered of the six.**
`crates/cli/tests/up_materializes.rs` drives the real binary over six
properties and asserts the *rendered text*: the section markers in order
(`found harnesses` → `your environment` → `rendered` → `next:`), that the
environment line states the shape it counted rather than claiming a
verification, that a nothing-to-verify run says so in words instead of printing
"verified against lock", that an unresolved `${REF}` is named with its command
and its fail-closed consequence, and that `up` owns no writing path of its own.
This is not a sketch and should be struck from P8.

**`run` — witnessed at both tiers.**
`examples/projects/locked-run/assert.sh` is a seven-property end-to-end assert
of the Protected host tier (plan mutates nothing · default run freezes a grant
and launches at the project root · `--unprotected` names every gate it skips ·
a flipped byte in the sealed artifact fails machine authentication · a
post-lock edit re-gates · a one-byte edit to a pinned executable refuses ·
`--toolset` fences the grant), and CI runs it in the example suite against the
release binary. The sandbox tier is the five CI `sandbox`-job tests above.

**`workflow run` — witnessed in the default suite, no Docker.**
`crates/cli/tests/workflow_e2e.rs` runs the tracked
`examples/workflow-acceptance` bundle through the real binary with a fake
prompt-driven `claude` on `PATH`, and adds two watchdog witnesses that the
process is force-exited on overrun. It is `#![cfg(unix)]` and nothing else — it
runs in the ordinary `cargo nextest run --workspace`.

**`workflow explain` — the JSON twin only.**
`crates/cli/tests/workflows_promotion.rs:297` asserts
`workflow explain … --json` down to per-role `harness`, `model`, `effort`,
`serial` and `undeliverable`. `print_explain`, the human screen, has no
assertion.

**`image` — the artifact, not the screen.**
`crates/cli/tests/packaging.rs` asserts the plan names every pinned member,
that no secret value can reach the image or the build context, that an
unpinned member fails the build closed, that the posture label is honest, and
that a missing daemon degrades with a complete staged context and a handover
command. `a_built_image_carries_its_labels_and_refuses_to_start_without_its_secret`
performs a real `docker build` and asserts the entrypoint guard exits 78
naming the secret ref. Note it is Docker-gated but **not** feature-gated and
**not** `#[ignore]`d, so it runs in the ordinary `test` job on `ubuntu-latest`
too. The tests call `image_cmd::run` in process, so the printed screen is not
captured.

**`use` — refusals well covered, happy path not at all.**
`red_team_skills_trust_gate.rs:163` asserts "refusing to materialize skills"
and that the message names `agentstack trust`; `content_pinning.rs` and
`regate_staging.rs:448` assert the drift and re-gate wording. The live-lane
phrasing that also appears in `use`'s banner is pinned in
`use_honours_delivery.rs:170-182`, but on a **`diff` outcome struct's
`warnings` field**, not on `use`'s stdout — so it constrains the wording
without witnessing the screen. The happy-path body of `use` — the activation
header, the per-harness lines, the closing count — has no assertion anywhere.

**`workflow report` — nothing.** No test in the workspace invokes it. Grepping
`"report"` across `crates/cli/tests/*.rs` returns only `x report run|runs|usage|calls`
and an unrelated MCP tool name. Of the six, this is the only true zero.

**No screen has a golden.** The only twelve `.snap` files in
`crates/cli/tests/snapshots/` are adapter render goldens. Every screen
assertion in this repo is a `contains(...)` over harvested stdout. That is a
deliberate-looking choice — `up_materializes.rs` says so explicitly: *"Not a
snapshot of exact bytes (that would break on every wording change and teach the
next reader to update it without thinking), but the sections in order."* P8
should follow that convention rather than introduce screen goldens.

## Cost, and what each buys

Sizes assume a maintainer who knows the tree. "Assertion" means a test that
fails on regression; "observation" means someone looked once.

| Item | Cost | Buys |
|---|---|---|
| **A. Make the CI Docker gate fail closed** — an `AGENTSTACK_REQUIRE_DOCKER=1` env var set by the `sandbox` job, read by the probe helpers, turning `SKIP` into a panic there and leaving dev machines alone | small — one helper edit per file plus two workflow lines | the flagship security suite can no longer pass while asserting nothing. This is the only item that closes a real defect rather than adding coverage |
| **B. `workflow report` end-to-end witness** — extend `workflow_e2e.rs`'s existing acceptance test to run `workflow report <run>` on the run it just produced and assert the evidence tree joins: one line per step, the child grant digests matching what the run printed, the taint mark on the verifier step, and the posture block present | small — the fixture, home and run id already exist in that test | the only wholly unwitnessed screen of the six, at near-zero marginal cost |
| **C. `workflow explain` screen assertion** — a sibling to the existing `--json` test, asserting the human text names the pinned digest, the ceilings, each role with its harness/model/effort, and the call-sites-not-calls caveat | small | closes the gap between the JSON contract and what a person reads |
| **D. `image` screen assertion** — capture stdout from a real binary invocation (`packaging.rs` currently calls in process) and assert the plan screen names each member with its digest and destination, the posture label, and the "nothing written, Docker not contacted" line | small–medium — needs a process-level invocation the file does not have yet | the plan screen is what a user judges the artifact by, and it currently has no guard |
| **E. `use --write` happy-path assertion** — assert the activation header, the per-harness lines including the live-lane explanation, and the closing count | small | the most-run of the six screens; today only its refusals are pinned |
| **F. `run --plan --sandbox` Docker-less assertion** — assert the plan screen with default features: posture, the read-only mount *with the policy reason*, the egress line | small | pins the decision layer on every machine and in the default CI job, independent of Docker |
| **G. Wire `crates/runtime/tests/docker.rs` into the `sandbox` job** — one line | trivial | a written test currently runs nowhere |
| **H. Refresh `crates/runtime/Cargo.toml:11-15`** — the comment claiming the backend is not behaviour-verified | trivial | stops a reader concluding the sandbox is less proven than it is |
| **I. Real-model workflow acceptance run** — `examples/workflow-acceptance/README.md`'s manual procedure with `check-evidence.sh` | medium, and recurring — needs a paid model and a human | model-facing semantics and the performance bookends. Buys nothing about governance, which B already covers with a fake harness. Keep it manual |

## Recommended order

1. **A** — it is the only item that fixes something broken. Everything else
   adds coverage; A stops a green check from lying.
2. **B**, then **G** and **H** — the zero-coverage screen and the two
   one-liners, all in the same sitting.
3. **F**, **C**, **E**, **D** — the four screen assertions, cheapest first.
   Each is independent; none blocks another.
4. Strike `up` from P8 entirely, and rewrite the `run` line: `run` is verified
   on the host tier by a CI example script and on the sandbox tier by five CI
   tests. What `run` lacks is a Docker-less assertion on its plan screen, which
   is item F.
5. **I** stays manual and stays off the queue.

After 1–4, P8 is closed. That is roughly one focused day, not a milestone.

## The honest risk

**Nothing here is blocked on infrastructure this project does not have.** That
is the finding, and it is worth saying plainly because the record implies the
opposite. Docker is present in CI (both the `test` and `sandbox` jobs run on
`ubuntu-latest`), the sidecar image is published to GHCR and pinned per
release, the fake-harness pattern removes the model from every governance
assertion, and the `sandbox` feature is a two-minute build.

The three risks that are real:

- **The Docker gate is advisory, not required.** Today the sandbox suite's
  green is conditional on an unstated precondition. Until item A lands, "CI is
  green" and "containment is enforced" are different statements, and a Docker
  Hub rate limit silently converts one into the other. This is the single
  most valuable thing in P8.
- **A plan is not an enforcement.** `run --plan --sandbox` renders on any
  machine and proves the decision, not the containment. If P8's output is used
  to support runtime-governance positioning, the distinction has to travel with
  it — the containment claim rests on the CI `sandbox` job, which is exactly
  why item A comes first.
- **macOS containment is unproven and unprovable as built.** Docker is the only
  backend; on macOS that means a Linux VM, so the kernel enforcing the
  read-only bind is not the user's kernel. That is fine and normal, but a claim
  of "kernel isolation on macOS" would need a seatbelt or equivalent backend
  that does not exist. P8 should not imply one.

One thing that surprised me and is not a P8 item, recorded so it is not lost:
while building a throwaway container image, the machine's own AgentStack guard
hook refused my `docker build` because the Dockerfile text contained
`/usr/local/bin/claude` — it classified a string inside a build instruction as
a host write. That is the classifier gap **G6** already names ("file-tool write
confinement covers only the fixed `WRITERS` names — build a payload-shaped
classifier"), observed firing in the false-positive direction on real work.

---

# Appendix — raw evidence

Environment for every command below unless stated otherwise:

```
HOME=<scratch>/lab/home
AGENTSTACK_HOME=<scratch>/lab/home/.agentstack
PATH=<scratch>/lab/fakebin:$PATH      # a ten-line fake `claude`
AS=target/release/agentstack           # default features
```

Exit codes were read with `$?` directly, never through a pipe.

## E0 — the binary

```
$ target/release/agentstack --version
agentstack 0.18.0-rc.2 (sandbox: no)
exit=0
```

`agentstack --help` lists exactly the fifteen verbs TODO #10 names (init,
status, add, search, apply, doctor, lock, toolset, use, yes, run, trust, undo,
adopt, secret) plus `help`. The review board's "the binary ships seventeen" is
stale. `agentstack x --help` lists the hidden set, `image` and `workflow` among
them under **Run**.

## E1 — `init` refuses without a terminal (unprompted)

```
$ agentstack init
error: refusing to init without a terminal: a flagless `agentstack init` imports
your CLI configs and can lift live token values into files, so it never runs
without a prompt or an explicit flag

  preview only (writes nothing):  agentstack init --dry-run
  import without prompts:         agentstack init --yes   (secrets → keychain)
  choose the secret store:        agentstack init --secrets <env|keychain|skip>

$ agentstack init --yes --secrets skip
… "Found 5 coding tools and their native configs" … "Import complete."
exit=0
```

## E2 — the fixture projects

`lab/wf` is a copy of `examples/workflow-acceptance/bundle` (one declared
`[workflows.mapreduce-acceptance]`, three empty-surface role toolsets).

`lab/app` is hand-written: one `stdio` server (`filesystem`), one path skill
(`greet`), one toolset (`backend`). Note for anyone reproducing: `[servers.X]`
requires `type` — omitting it fails with
`manifest does not match the expected schema: missing field 'type'`, exit 1.

Both were prepared with:

```
$ agentstack lock --write                    # exit 0
$ consent=$(agentstack trust . --preview | sed -n 's/.*"surface_digest": "\([^"]*\)".*/\1/p')
$ agentstack trust . --yes --consented-digest "$consent"    # exit 0
```

The trust card for `lab/wf` labels the workflow correctly:
`workflows (ORCHESTRATION CODE — spawns agent runs under the declared roles;
agentstack executes this, gated and sandboxed)`.

## E3 — `workflow explain` (screen 4)

Untrusted first, to check the gate:

```
$ agentstack x workflow explain mapreduce-acceptance
error: refusing to normalize workflows: <…>/lab/wf is not trusted — nothing from
an untrusted bundle normalizes or is invocable; review and grant with
`agentstack trust .`
exit=1
```

Trusted:

```
$ agentstack x workflow explain mapreduce-acceptance
workflow  mapreduce-acceptance
pinned    d9ffcc15f3bba4625688137da0afa280eb889873e657dc3decb324e0c2f5f5b6

ceilings  max_agents=6  max_wall_seconds=300  concurrency=4

roles
  mapper               concurrent
                       harness=claude-code  model=(harness default)  effort=(harness default)
  reducer              concurrent
                       harness=claude-code  model=(harness default)  effort=(harness default)
  verifier             concurrent
                       harness=claude-code  model=(harness default)  effort=(harness default)

3 agent() call sites in the pinned source.
Sites, not calls: one site inside a loop or .map() runs once per item, so the actual
fan-out is data-dependent. The enforced bound on TOTAL spawns is max_agents=6 — the
engine refuses the call past it, per call.
exit=0
```

## E4 — `workflow run`, end to end, no Docker

```
$ agentstack x workflow run mapreduce-acceptance
▶ workflow 'mapreduce-acceptance' admitted: run w-6c3667eb0b, 3 roles,
  effective ceilings max_agents=6 max_wall_seconds=300, concurrency cap 4
◆ Map
  ▶ agent #0 [map:1] role=mapper harness=claude-code
  ▶ agent #1 [map:2] role=mapper harness=claude-code
  ▶ agent #2 [map:3] role=mapper harness=claude-code
  ✓ agent #0 completed (run r-b27bdc872a, grant sha256:11a396cf…)
  ✓ agent #2 completed (run r-33028305a2, grant sha256:ec6d0c4a…)
  ✓ agent #1 completed (run r-675d43609b, grant sha256:fae7d748…)
◆ Reduce
  ✓ agent #3 completed (run r-d6cc8057c1, grant sha256:4940ed46…)
◆ Verify
  ✓ agent #4 completed (run r-4b9790ed1e, grant sha256:b611a0ee…)
{ "pass": true, "mapOutputs": [ … ], "reduced": "…", "verdict": "…" }
exit=0
```

Five distinct grant digests, three concurrent map children, 1.0 s wall.

The record's "nothing declared" case reproduces exactly as expected in a
project without a workflow:

```
$ agentstack x workflow run anything
error: no workflow named 'anything' — declared and admitted: (none)
exit=1

$ agentstack x workflow list
(no workflows declared)
exit=0
```

## E5 — `workflow report` (screen 5)

```
$ agentstack x workflow runs
RUN            WORKFLOW                 OUTCOME      AGO   DURATION  STEPS  RESUMABLE
w-6c3667eb0b   mapreduce-acceptance     completed    11s   1.0s      5      false
exit=0

$ agentstack x workflow report w-6c3667eb0b
Workflow run w-6c3667eb0b: 'mapreduce-acceptance'
  script digest   d9ffcc15f3bba46…
  grant digest    da858a942191ddb…
  args digest     29d57ca0ce5c11d…
  effective ceilings  max_agents=6 max_wall_seconds=300

Steps:
  #0 role=mapper [map:1] child=r-b27bdc872a
     child: grant=sha256:11a396cf… posture=host-protected outcome=completed (exit 0) usage=unavailable
     step:  completed
  …
  #4 role=verifier [verify] child=r-4b9790ed1e — taint: prompt embeds output of #3
     child: grant=sha256:b611a0ee… posture=host-protected outcome=completed (exit 0) usage=unavailable
     step:  completed

Outcome: done (968 ms)

Honest posture (§12.2, verbatim):
  [the full BOUNDED / NOT BOUNDED block, printed verbatim]
exit=0
```

The taint mark on step #4 and the verbatim posture block are the two things
worth asserting in item B.

## E6 — `use` (screen 1)

Preview:

```
$ agentstack use backend
Activating toolset 'backend' (scope: project) — 1 server, 1 skill
[Claude Code]  · MCP servers are planned live (not connected), not written …
               → 1 skill to symlink into …/.claude/skills
[Codex CLI]    → 1 skill to symlink into …/.agents/skills
[GitHub Copilot CLI]  · (no skills dir at this scope for this CLI — 1 skill not materialized)
[OpenCode]     · (skills not supported by this CLI — 1 skill not materialized)
[Pi]           → 1 skill to symlink into …/.pi/skills
ℹ MCP servers for … are routed to the live lane — `use` does not write them.
  · nothing is being served yet — … have no bridge registered.
  → register the bridge: agentstack x gateway connect --all --write
Dry run. Re-run with --write to apply.
exit=0
```

Write:

```
$ agentstack use backend --write
… ✓ activated 'backend' — wrote skills to 3 locations; no server configs changed.
exit=0

$ find . -type l -o -type f
./.agentstack/agentstack.lock
./.agents/skills/greet
./.claude/skills/greet
./.pi/skills/greet
./.agentstack/skills/greet/SKILL.md
```

## E7 — `up` (screen 2)

Trusted:

```
$ agentstack x up
found harnesses     Claude Code · Codex CLI · GitHub Copilot CLI · OpenCode · Pi
  ✓ greet cached (path)
✓ lockfile up to date.
your environment    1 toolset · 1 skill · 1 server · 1 skill source verified against lock
rendered
Scope: project
… per-harness lines …
  rendering stopped early — nothing was delivered: every capability here is routed
  to the live lane and no bridge is registered …
next: agentstack x gateway connect --all --write
exit=0
```

Same checkout, a genuinely fresh `HOME` (so the project is untrusted): byte
for byte the same body, and the closing line correctly becomes
`next: agentstack trust .`, exit 0. Nothing was delivered in either case
because every capability is live-lane, so the zero exit is right.

## E8 — `run` (screen 3), four levels without the feature

```
$ agentstack run claude-code --toolset backend --plan
→ plan for `run claude-code --locked` (nothing will be mutated)
  posture: HOST / PROTECTED
  ℹ protected host run: content trust, strict lock verification, and policy
    admission are enforced BEFORE launch … Not kernel isolation …
  ✓ no ambient user/global-scope MCP entries for this harness …
  ✓ toolset fence: 'backend' …
  ✓ trust: explicitly trusted
  ✓ locked inputs: 1 skill, 0 instructions, 1 server, 0 executable pins, 0 extensions verified
  ✓ policy: declared requests fit under the machine ceiling
  proposed grant: … digest: sha256:077c01c9…
✓ live launch would proceed
exit=0

$ agentstack run claude-code --toolset backend --prompt "In 6 words say the rule"
▶ launching claude-code with --locked…
  ✓ headless: prompt delivered as one argv element (no shell) …
  ✓ authority grant frozen: sha256:0e0561ae…
  ✓ run grant handed to the gateway (…/runs/r-34e62935a0/grant.json)
  ✓ per-run MCP config injected via harness flags …; the shared project config is untouched
Nothing runs until it is trusted
See what happened: `agentstack x report run r-34e62935a0`
exit=0

$ agentstack run claude-code --toolset backend --sandbox
error: this build has no sandbox support — nothing was launched
  it was compiled without the optional `sandbox` feature …
  rebuild it with:  cargo build --features sandbox
  or install a published release binary — those ship with it
  either way, a sandbox run also needs a running Docker daemon
exit=1
```

`--lockdown` gives the identical refusal, exit 1.

The Docker-less plan of a sandbox run, on the `(sandbox: no)` binary:

```
$ agentstack run claude-code --toolset backend --sandbox --plan
▶ sandboxing claude-code (run r-c062169ef7) — bundle trusted
  posture: SANDBOX / PROXIED · DIRECT ROUTE OPEN
  workspace: <proj> → /workspace read-only — no [policy.filesystem] write scope
    covers the workspace (sandbox workspace writes are deny-by-default)
  🛡 egress is routed through the AgentStack proxy; review it after with
    `agentstack x report run r-c062169ef7`.
  command: claude
exit=0
```

Run evidence for one of the workflow children, showing the four gates:

```
$ agentstack x report run r-33028305a2
Run r-33028305a2
  Locked run  claude-code · HOST / PROTECTED
    ✓ trust  (sha256:bbec7e2f…)
    ✓ locked-verify
    ✓ rendered-verify
    ✓ policy-admission
    ✓ grant frozen: sha256:ec6d0c4a…
    ✓ headless output: 33 bytes · sha256:831289e4…
    ✓ completed · exit 0 · 869ms
exit=0
```

## E9 — the sandbox feature build, and the real thing

Docker on this machine: `/usr/local/bin/docker`, `docker info` exit 0.

Cold build into an isolated `CARGO_TARGET_DIR` (so as not to contend with
other work in `target/`):

```
$ cargo build -p agentstack --features sandbox --release
   Compiling bollard v0.21.0
   Compiling agentstack-runtime v0.17.0
   …
    Finished `release` profile [optimized] target(s) in 2m 05s
exit=0

$ <sbx>/release/agentstack --version
agentstack 0.18.0-rc.2 (sandbox: yes)
```

First attempt, against `alpine:3` as the runner — the container is created and
the gateway and proxy come up; it fails only because alpine has no `claude`:

```
$ AGENTSTACK_SANDBOX_IMAGE=alpine:3 agentstack run claude-code --toolset backend --sandbox
▶ sandboxing claude-code (run r-c939d2b659) — bundle trusted
  posture: SANDBOX / PROXIED · DIRECT ROUTE OPEN
  workspace: <proj> → /workspace read-only …
gateway: proxying 1 frozen server from the run plan
  ✓ MCP tool calls routed through the gateway (tool policy enforced, calls recorded)
error: running the sandbox container (image `alpine:3`) … exec: "claude":
executable file not found in $PATH
```

With a three-line runner image carrying a fake `claude` that tries to write to
the workspace:

```
$ AGENTSTACK_SANDBOX_IMAGE=agentstack-p8/runner:fake agentstack run claude-code --toolset backend --sandbox
▶ sandboxing claude-code (run r-f1a87cb3ee) — bundle trusted
  posture: SANDBOX / PROXIED · DIRECT ROUTE OPEN
  workspace: <proj> → /workspace read-only — no [policy.filesystem] write scope covers the workspace
  🛡 egress is routed through the AgentStack proxy …
gateway: proxying 1 frozen server from the run plan
  ✓ MCP tool calls routed through the gateway (tool policy enforced, calls recorded)
fake harness running inside the sandbox container
workspace contents:
write test:
touch: /workspace/pwned: Read-only file system
workspace write REFUSED by the kernel (read-only bind)

✓ sandbox exited cleanly.
exit=0
```

And `--lockdown`, which pulled the published sidecar
(`ghcr.io/tarekkharsa/agentstack-egress-proxy:v0.18.0-rc.2`, already local):

```
$ AGENTSTACK_SANDBOX_IMAGE=agentstack-p8/runner:fake agentstack run claude-code --toolset backend --lockdown
  🔒 lockdown: no host route, no internet — the container's only peer is the egress sidecar.
  … same read-only refusal … ✓ sandbox exited cleanly.
exit=0

$ agentstack x report run r-0484d73852
Run r-0484d73852
  Posture   LOCKDOWN / ENFORCED · NO DIRECT ROUTE
  Sandbox   agentstack-p8/runner:fake   workspace <proj>
  Exit      0
exit=0
```

## E10 — `image` (screen 6), plan and real build

```
$ agentstack x image --toolset backend --harness claude-code
  Image  toolset backend for Claude Code → agentstack/backend:latest
  · from agentstack/sandbox:latest
  skill    greet       856fcd7554f1  /agentstack/home/.claude/skills/greet
  server   filesystem  3b986135ee09  /agentstack/servers/filesystem.json (carried)
  · no secrets are required at run time
  · posture SANDBOX / PROXIED · DIRECT ROUTE OPEN
  · posture belongs to the run, not the image — a bare `docker run` gets the
    container boundary only: no egress proxy, no allowlist, no run log
  · nothing has been written and Docker has not been contacted —
    `agentstack x image --write` builds it.
exit=0
```

A real build, on the **default-feature** binary — `image --write` shells out to
`docker build` and does not need the `sandbox` feature:

```
$ agentstack x image --toolset backend --harness claude-code \
    --from alpine:3 --tag agentstack-p8/backend:witness --write
… Step 1/11 … Step 11/11 : CMD ["claude"]
Successfully tagged agentstack-p8/backend:witness
  ✓ built agentstack-p8/backend:witness — one toolset, 2 pinned members
  · run it under the sandbox contract:
    AGENTSTACK_SANDBOX_IMAGE=agentstack-p8/backend:witness agentstack run claude-code --sandbox
exit=0

$ docker run --rm agentstack-p8/backend:witness /bin/sh -c \
    'ls -la /agentstack/home/.claude/skills/greet && cat /agentstack/image.json'
… SKILL.md, .agentstack-managed …
{ "image": { "toolset": "backend", "harness": "claude-code",
  "posture": { "slug": "sandbox", "label": "SANDBOX / PROXIED · DIRECT ROUTE OPEN",
    "established_by": "run", "caveat": "posture belongs to the run, not the image …" },
  "members": [ { "kind": "skill", "name": "greet", "digest": "856fcd75…",
    "provenance": "path:./skills/greet", "dest": "…", "compiled": true }, … ] } }
exit=0
```

## E11 — the CI Docker gate, as read

`.github/workflows/ci.yml`, `sandbox` job:

```yaml
  sandbox:
    runs-on: ubuntu-latest
    steps:
      - name: Sidecar image + policy enforcement (Docker)
        run: cargo test -p agentstack-egress --test sidecar_image -- --include-ignored --nocapture
      - name: Sandbox egress + lockdown end-to-end (Docker)
        run: |
          cargo test -p agentstack --features sandbox \
            --test sandbox_egress --test sandbox_cli_e2e --test sandbox_fs \
            --test sandbox_lockdown --test sandbox_gateway_e2e -- --nocapture
```

The probe every one of those tests sits behind, and the early return it
produces, are quoted in the body above. `grep -rn "REQUIRE_DOCKER" crates` →
no matches. The images the probes pull are `curlimages/curl:latest`,
`busybox:latest`, `node:22-slim` and (in `packaging.rs`) `alpine:3`, all from
Docker Hub, all anonymously.
