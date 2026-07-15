# Behavioral contract: `agentstack run <harness> --locked`

- Date: 2026-07-15
- Status: **Approved for implementation** (revision 4; review rounds 1–3 rulings incorporated). Approved 2026-07-15; supersedes the draft-for-review status.
- Phase: 0A (Contract). See [`STRATEGY.md` §Phase 0A](../../STRATEGY.md#phase-0a--prove-the-canonical-no-docker-activation-path) and [`TODO.md` Phase 0A Contract](../../TODO.md).
- Security-sensitive: **yes** — this contract binds trust, lock verification, policy compilation, secret grant, and evidence into one host activation path. Every section marked 🔒 needs line-by-line review.

> This document specifies *behavior and guarantees only*. It intentionally
> writes no Rust and mandates no internal type layout beyond naming the existing
> seams it must reuse and the two authority structures the evidence model needs.
> Implementation is the next, separate task and starts only after this contract
> is approved.

### Rulings incorporated

- **Round 1:** `--locked` opt-in for Phase 0A (§2.1); D3 Option A — pin repo-local
  executable payloads (§8); skill-cache bypass is a hard prerequisite (§3 step 4);
  frozen field list expanded (§6).
- **Round 2:** empty gateway is a valid state, not a failure (§3 step 6); recorder
  uses `AttemptStarted` → `GrantFrozen` with a **checked** append (§3 step 2, §9);
  the grant digest is defined over a separate `AuthorityGrant`, wrapped by a
  `RunEnvelope` (§6); caller argv is never recorded verbatim (§4, §6, §9); D3
  covers transitive repo-local imports via declared content roots (§8); local
  executables are an **integrity** dimension, not a policy-admission one, and
  admission blocks unclassifiable hosts (§3 steps 4–5); D2 sequencing fixed (§7);
  STRATEGY §12-A reworded (done).
- **Round 3:** argv settled as one identity — exact-stored, keyed-committed in the
  digest, redacted in every view, no unkeyed fallback (§4, §6.1); integrity roots
  need recursive symlink handling — `dir_digest` skips symlinks and is not reusable
  as-is (§8); "successfully appended" means checked write success, not `fsync`, and
  a failure to record a refusal is surfaced alongside the refusal without launching
  (§3 step 2, §9).

---

## 0. Naming note before anything else

The strategy and TODO name the gateway seam `Gateway::from_plan`. **That symbol
does not exist in the code.** The shipped constructor is
`Gateway::from_frozen` (`crates/cli/src/gateway.rs:533`); `from_plan` appears
only in aspirational doc comments. This contract uses the real name. Renaming
`from_frozen` → `from_plan` is a **separate mechanical rename** — but not purely
cosmetic: `from_frozen` names narrower semantics (it consumes an already-frozen
server set and ruleset) than a general "from the plan" would imply, so the
eventual name still deserves review. Do not conflate that rename with this work.

`ExecutionPlan` exists today **only for the `--sandbox` path**
(`crates/cli/src/commands/sandbox.rs:202`). It is not a general run-plan type,
and — as §7 details — it is **not yet an enforcing grant builder**: it *records*
trust without enforcing it and runs only partial lock verification. D2 is the
work of growing it into the single, enforcing grant every delivery path
consumes.

This revision introduces two named authority structures the evidence model
depends on (§6): **`AuthorityGrant`** (the projection that is digested and
printed) and **`RunEnvelope`** (run identity + recorder identity +
`digest(AuthorityGrant)`). They are contract-level concepts; the implementing
types may be named differently but must preserve the projection boundary.

---

## 1. What `--locked` is

`--locked` is a **fail-closed modifier** on the existing `run` command. It does
not change the command's shape, add a positional, or consume an argument. It
promotes the plain host run — which today does **no** trust, lock, or policy
enforcement (`crates/cli/src/commands/runs.rs:17-48`, banner literally prints
`posture: HOST / ADVISORY`) — into the **Protected activation** tier: explicit
trust, verified locked inputs, policy compiled under the machine ceiling,
cooperative host guards, and recorded evidence — **without Docker and without the
`sandbox` feature.**

The one-sentence guarantee:

> `agentstack run <harness> --locked` refuses to launch the harness unless the
> current directory's agent configuration is explicitly trusted, **every
> activation input in the declared integrity surface has a required pin and
> matches it**, and the repository's statically-declared capability requests fit
> under the machine policy ceiling — and it records what it decided, including
> refusals.

What it is **not**: not kernel isolation, not a sandbox, not network
confinement. Those remain the separate `--sandbox` / `--lockdown` maximum-
assurance tier (§5). `--locked` is honest, meaningful, pre-launch gating on the
host; it does not contain a harness that has already launched, and its evidence
is a cooperative local audit trail, not tamper-proof attestation (§3.1).

---

## 2. CLI surface and composition 🔒

```text
agentstack run <harness> [--locked] [--profile NAME] [--scope project|global]
                         [--keep] [--plan] [--sandbox] [--lockdown] [-- ARGS…]
```

`<harness>` stays the required positional (`crates/cli/src/cli.rs:428`).
Trailing harness args keep their current `trailing_var_arg` behavior
(`cli.rs:469-475`) and are forwarded verbatim to launch, but are caller-supplied
and never recorded verbatim (§4).

### 2.1 Flag interactions — the decision table

| Combination | Behavior |
|---|---|
| `run H` (today, unchanged) | Advisory host launch. No trust/lock/policy gate. Prints `HOST / ADVISORY`. **Kept as-is** so `--locked` is a visible, opt-in promotion. |
| `run H --locked` | Protected host activation. The strict gate sequence (§3) runs, then the harness launches on the host. Posture label: **`HOST / PROTECTED`** (new). |
| `run H --locked --profile P` | The strict gate runs against the profile-fenced capability set. **Profile application does NOT behave "as today":** today `--profile` mutates native state before spawning (`runs.rs:152-164`). Under `--locked`, (a) no profile mutation before the gate passes; (b) `--plan` never applies the profile; (c) any live application renders from the frozen `AuthorityGrant` (§7). A `--profile` absent from the manifest remains a hard error (`resolve.rs:340-346`). `--scope`/`--keep` otherwise unchanged. |
| `run H --locked --plan` | Non-mutating dry run. Evaluate everything, **collect all blockers**, print and digest a **complete proposed `AuthorityGrant`** (§6) — **without inventing a run id or recorder** — and **exit nonzero if a live launch would be refused**. Applies no profile, resolves no secret, writes no recorder log. See §2.2. |
| `run H --sandbox` / `--lockdown` (today) | Maximum-assurance container path, **not a superset of `--locked`:** on absent trust it **warns and continues** (`sandbox.rs:408-418`) and does partial lock verification. Container topology is the differentiator; trust/lock gating is currently weaker than `--locked`. |
| `run H --locked --sandbox` | **`--locked` strengthens `--sandbox`.** The strict gate (§3) runs and must pass before the container executes; the container then adds topology confinement. The intended strong combination. Whether plain `--sandbox` should adopt the strict gate by default is a separate open item (§12-D). |

**Default posture (ruling 1):** plain `run H` stays advisory for Phase 0A;
`--locked` is opt-in. The exit gate reconsiders making it the default (with an
explicit `--unlocked`/`--advisory` escape). STRATEGY's assurance table is now
worded to match (§12-A, done).

### 2.2 `--plan` behavior and the current gap 🔒

Today `--plan` is only read inside the sandbox path (`sandbox.rs:403`);
`run H --plan` with neither `--sandbox` nor `--lockdown` **silently ignores
`--plan` and launches the harness for real**. That is unacceptable for a locked
contract.

`--locked --plan` is a **non-mutating, secret-free evaluation** that runs the
same checks as a live locked run **but**: (a) applies no profile and writes no
native config; (b) resolves no secret value; (c) creates no recorder log and
**invents no run id** — it produces an `AuthorityGrant` and its digest, never a
`RunEnvelope` (§6); (d) **aggregates all blockers** across trust, lock, and
admission rather than stopping at the first; (e) prints the complete proposed
`AuthorityGrant` with its honest limits (§3.1); and (f) exits **nonzero** when a
live launch would be refused, zero otherwise. This is the deliberate exception to
the live run's "fail at the first violation" behavior (§3).

---

## 3. The no-Docker guarantee, step by step 🔒

A **live** `run H --locked` performs these steps in order and **fails closed at
the first blocking gate**, before the harness binary is spawned. Run identity and
the recorder are created **before** the gates so a refusal is itself recorded
evidence. (`--plan` instead follows §2.2.)

1. **Resolve project state without executing repository code.** Load the
   manifest, machine `[instructions]` layer, adapter registry, and the secret
   *resolver* (not resolved values) via `commands::load`
   (`crates/cli/src/commands/mod.rs:98`). No repository-controlled hook, script,
   or MCP server runs during resolution.

2. **Open the recorder and record an attempt (before any gate).** Generate the
   run id (`runs::gen_id`) and create the per-run flight recorder
   (`recorder::RunLog::create`); failure to create it refuses the run ("refusing
   to run unobserved", cf. `sandbox.rs:1068-1074`). Emit **`AttemptStarted`**
   (run id, invocation identity — **no grant digest yet**, because the grant is
   not frozen until step 6). **Material lifecycle and gate events must use a
   checked append that can fail the run:** `RunLog::append` currently swallows
   all write failures (`recorder/src/lib.rs:403`), which is fine for best-effort
   telemetry but **insufficient for contractual refusal evidence**. "Successfully
   appended" here means a **checked write success** (the append returned without
   error), **not** crash-durable `fsync`/`sync_data` — crash durability is not
   required by this contract, and a checked append that can fail the run is
   enough. If a material event (attempt, gate decision, `GrantFrozen`, terminal
   outcome) cannot be successfully appended, the run is refused rather than
   proceeding unrecorded.

   **When recording a refusal itself fails (round-3 correction 3):** the command
   must surface **both** the original gate refusal **and** the recorder failure,
   and must still **never launch**. It must not claim the missing event exists —
   the contract's promise is "a refusal is recorded *or* the failure to record it
   is itself surfaced and the run still does not launch," never a silent gap.

3. **Require explicit trust (enforced, not merely recorded).** Call
   `agentstack_trust::check(project_root)` (`crates/trust/src/lib.rs:138`).
   Anything other than `Trusted` **aborts** (this is where `--locked` diverges
   from today's sandbox path, which warns and continues):
   - `Untrusted` → "run `agentstack trust .` after reviewing" + a trust preview.
   - `Changed` → "configuration changed since you trusted it; re-review" + diff
     pointer.
   Record the trust decision (observed `TrustState`, consent digest — never
   secret values) via the checked append; on abort emit a terminal outcome event
   (which naturally has no grant digest — the grant was never frozen). Trust is
   bound to the digest over `agentstack.toml` + `agentstack.local.toml` +
   `agentstack.lock` (`trust/src/lib.rs:121-135`); it is the content identity,
   not a signature — `--locked` does not require `agentstack sign`.

4. **Strictly verify locked inputs; missing pins block.** The existing verifier
   is not strict enough: `verify.rs` maps `MissingLockEntry` to
   `Verdict::Unpinned`, which is *permitted* (`verify.rs:91`). `--locked` requires
   a **strict verifier (or explicit strict mode)** in which, for every input in
   the declared integrity surface, a missing pin, checksum drift, unreadable lock,
   or unavailable offline pin all **block** and name the offender, directing to
   `agentstack lock`. The integrity surface is:
   - skills, instructions, and servers (existing pins);
   - **repository-local executable payloads** (D3, §8): the repo-relative stdio
     `command`, local interpreter-script `args`, and their declared content roots
     — pinned by current bytes, **cache-free**, with canonical-path/symlink
     handling (traversal or symlink escape out of the project is a hard error).
   Local executable integrity lives **here**, under strict verification — it is
   **not** a policy-admission dimension (there is no machine-policy executable
   ceiling, and this contract does not introduce one; §3 step 5).

   Additionally, `resolve::frozen_runtime_servers` currently **preserves per-server
   pin failures for later skipping** rather than failing the run
   (`resolve.rs:330`). Locked assembly must **aggregate those errors and reject
   the run** — a server that fails its pin must not be silently dropped.

   🔒 **Hard prerequisite (ruling 3):** skill content verification today routes
   through the stat-fingerprint digest cache (`store.rs::dir_digest_cached_with`),
   while servers/instructions hash current bytes. `docs/ARCHITECTURE.md:119`
   forbids authoritative verification from using that cache. This bypass must be
   fixed as a **separate correctness change with its own regression test**, and
   `--locked` **cannot merge or ship** while authoritative verification can
   consume a cached skill digest.

5. **Compile policy, then check the enumerable admission surface.** Compilation
   and admission are **distinct**: `agentstack_policy::compile`
   (`policy/src/compile.rs:25`) returns a *narrowed ruleset*, not a
   denied-request verdict. After compiling the effective ruleset under the machine
   ceiling (`render::ruleset_for` / `gateway.rs:701`; machine load is fail-closed
   with last-known-good, D1), evaluate the repository's **statically-declared
   requests** against it and refuse when one falls outside the ceiling. The
   **admission surface** is:
   - declared MCP/egress **hosts** — a host the machine egress layer denies →
     refuse (machine layer named); **an unclassifiable host — e.g. a `${REF}` in
     the host portion, or any host that cannot be resolved to a concrete name for
     classification — blocks admission**, because the contract cannot otherwise
     establish the host fits under the machine egress ceiling (mirrors the D4
     gateway-only classifier's fail-on-unclassifiable);
   - declared **secret references** — a `${REF}` the machine secret layer denies →
     refuse (by reference name only, never a resolved value);
   - (Phase 1 placeholder) declared **workspace roots** overlapping a deny mask.

   **Tool allowlists are constraints, not requests:** the ruleset narrows which
   tools are callable at call time; an empty tool intersection is a runtime
   constraint, not a pre-launch admission failure. **Local executables are not
   here** — they are integrity inputs (step 4). The proptest invariant
   `effective ⊆ machine` (`policy/src/lib.rs:139`) still guarantees the ruleset
   can only narrow. Record a policy-admission event (ruleset compiled; whether any
   declared request was refused and the rule text — never secrets); on refusal
   emit a terminal outcome event.

6. **Freeze the `AuthorityGrant`, record `GrantFrozen`, then build the gateway.**
   Freeze the `AuthorityGrant` (§6) once and emit **`GrantFrozen { grant_digest }`**
   (checked append). Build the in-process gateway with `Gateway::from_frozen` from
   **that** grant's ruleset + frozen servers — never re-resolved or re-compiled.

   **Empty gateway is a valid state, corrected (round-2 blocker 1):** a trusted
   profile may legitimately declare **zero** MCP servers, and the existing sandbox
   code already distinguishes that case (`sandbox.rs:843`). A **zero-upstream
   gateway is valid**; ambient native MCP entries are still shadowed (step 7).
   What must fail the run is not emptiness but an **explicit unsafe construction
   status** — trust not `Trusted` at construction (`gateway.rs:566`) or a frozen
   input that dropped out between the gate and construction. The gateway
   constructor must therefore return a **fallible result / explicit admission
   status** distinguishing "intentionally empty, trusted, zero servers declared"
   (proceed) from "empty because unsafe" (abort). `--locked` proceeds only on the
   former.

7. **Launch through launch-scoped configuration with cooperative host guards.**
   Spawn the harness on the host (existing `runs::spawn_child`) carrying the
   grant: host-guard hooks where the client supports them, `AGENTSTACK_RUN_ID`
   set, and MCP traffic pointed **only** at the synthetic gateway entry. The MCP
   claim holds only if the harness's live configuration exposes the synthetic
   gateway and nothing else, so `--locked` requires **launch-scoped configuration**
   for the run's lifetime, with an explicit rule for **pre-existing ambient native
   MCP entries**: they are shadowed/neutralized (as the sandbox path does via
   `shadow_native_config`, `sandbox.rs:843-859`), not left reachable around the
   gateway — **including when the gateway is validly empty.** Where a client
   cannot enforce a capability, the run states so honestly in the plan and report
   (advisory, not silently dropped).

8. **Record outcome and clean up per artifact mode.** On harness exit, record the
   lifecycle outcome (exit status, duration, and token/cost evidence or explicit
   `unavailable`, §9). Revert an applied profile unless `--keep`
   (`runs.rs:204-211`); restore any shadowed native MCP config; honor static /
   clean-at-rest / zero-files modes for generated gateway/native config.

### 3.1 Honest non-isolation limits (must appear in `--plan`, trust preview, and report)

- The harness runs as the invoking user on the host. `--locked` does **not**
  confine filesystem, process, or network access at the kernel level.
- **The harness runs as the same user who can modify the trust store, machine
  policy, and local recorder.** Recorded evidence is a cooperative local audit
  trail, **not tamper-proof attestation** — a process running as that user can
  alter `~/.agentstack/trust.json`, the machine policy source, and run logs.
  `--locked` detects accidental drift and enforces policy against repository
  configuration; it does not defend against a host-compromised user process.
- **The resolved harness executable itself is an external, unpinned `$PATH`
  binary** (`claude`, `codex`, `node`, `python`, …). `--locked` pins
  repository-controlled executable content (§8), never the interpreter or harness
  binary the OS resolves.
- Host-guard hooks are **cooperative**: they enforce only for clients that honor
  them and only for the operations those hooks intercept.
- MCP brokering applies to declared MCP servers routed through the synthetic
  entry. It does not intercept arbitrary network calls the harness or a spawned
  tool makes directly — that is the `--lockdown` tier's job.
- The no-Docker tier proves *content trust and policy admissibility before
  launch*. Runtime containment is `--sandbox`/`--lockdown` only.

---

## 4. Trailing arguments and the harness slot 🔒

Remote acquisition must **not** be overloaded into the harness positional
(`STRATEGY.md:314-318`). `--locked` takes no source argument; the project is
always the current working directory for this release.

Trailing args after `--` are **caller-supplied**: they are **not** content-
verified and **not** policy-constrained — the human operator's own argv, outside
the repository trust boundary. They can contain tokens or passwords AgentStack
cannot reliably recognize or redact, so the argv model is settled as one identity
with three views (round-3 correction 1):

- **Storage:** the live in-memory `AuthorityGrant` holds **exact argv** as a
  single **sensitive field** — it must, because the harness is launched with it.
  There is no second "the grant sometimes has argv, sometimes doesn't" identity.
- **Digest:** the canonical `digest(AuthorityGrant)` (§6.1) **binds the exact
  invocation** by encoding argv through a **mandatory keyed commitment** (a keyed
  MAC over the exact argv), so the digest is invocation-binding without being a
  brute-forceable unkeyed hash of a low-entropy secret.
- **Display/record:** every durable or printed view — recorder evidence, `--plan`
  output, the trust preview — shows a **redacted (non-correlating) representation**
  and never the verbatim argv.

**No unkeyed fallback is permitted for locked evidence.** The recorder's existing
keyed argument hash is **not** sufficient as-is: when its key is unavailable it
explicitly falls back to an **unkeyed** digest (`recorder/src/lib.rs:107`). Under
`--locked`, if the commitment key is unavailable, behavior is path-specific and
never a silent downgrade to an unkeyed commitment:

- **Live locked run:** grant construction **fails and the run never launches** —
  a locked run must not proceed without an invocation-binding keyed commitment.
- **`--plan`:** report the missing-key condition as a **blocker**, show **only
  redacted argv**, produce **no valid invocation-binding digest**, and **exit
  nonzero** (consistent with §2.2's "nonzero when a live launch would be refused").

---

## 5. Maximum-assurance guarantee, stated separately

`--sandbox` and `--lockdown` are a distinct tier. This contract does not change
their topology guarantees, and does **not** describe the current sandbox path as
a superset of `--locked`.

| Mode | Topology guarantee | Trust/lock gating today | Limit |
|---|---|---|---|
| `--locked` (this contract) | None (host process). | **Strict, enforced** (§3): abort on untrusted/changed, strict lock verify, admission check. | Not kernel isolation. Post-launch containment advisory; evidence not tamper-proof (§3.1). |
| `--sandbox` (today) | Container mounts project as workspace, points HTTPS at the policy proxy. | **Weaker than `--locked`:** warns and continues on absent trust (`sandbox.rs:408-418`); partial lock verification. | Ordinary bridge still permits direct connections bypassing the proxy (`cli.rs:443-450`). Requires `--features sandbox` + Docker. |
| `--lockdown` (implies `--sandbox`) | Internal Docker network, no host route; only peer is the egress-proxy sidecar; declared MCP hosts direct-denied on all ports; literal-IP/non-TLS tunnels refused; unsafe fallback fails closed (D4). | Same as `--sandbox` today. | Platform-specific; Docker/kernel are trusted computing base. |
| `--locked --sandbox` (intended) | Container topology of `--sandbox`. | **Strict gate of `--locked` applied first.** | Both tiers' guarantees; both limits still apply. |

§12-D tracks whether plain `--sandbox` should adopt `--locked`'s strict gate by
default so it stops being weaker than the host tier.

---

## 6. The authority model: `AuthorityGrant` and `RunEnvelope` 🔒

Round-2 blocker 3: the revision-1/2 field list put the grant digest *inside* the
grant and again inside evidence identity — circular and underspecified. The fix is
two structures with a clean projection boundary.

### 6.1 `AuthorityGrant` — the thing that is digested and printed

The complete, backend-neutral authority projection. `digest(AuthorityGrant)` is a
**canonical digest over exactly these fields** (deterministic ordering, the
`indexmap`/length-framed discipline the trust and lock digests already use), with
the **exact invocation bound through a mandatory keyed commitment** (§4) so the
digest is invocation-binding without hashing low-entropy caller secrets in the
clear. It contains **no run id, no recorder identity, and no digest-of-itself.**
`--plan` prints (redacted) and digests exactly this, without inventing a run.

| Field group | Content | Present today? | Source seam |
|---|---|---|---|
| **Grant schema/version** | The grant format version (so a later backend narrows a known shape). | Absent | new |
| **Project/content identity** | Project identity + trusted **consent digest** (the trust digest consent was pinned to). | Partial (`TrustState`) | `trust::digest_for` |
| **Exact invocation** | Harness/adapter identity, resolved executable path (external `$PATH` binary, §3.1), argv (**stored exact as a sensitive field; keyed-committed in the digest; redacted in every display/record view**, §4), cwd, `--profile`, `--scope`. | Partial — `ExecutionPlan.spec.command` and `manifest_dir` already carry command/argv and directory **partially** (no resolved exec identity, no scope, no keyed commitment, no redaction) | `SandboxSpec.command`, `manifest_dir`, `AdapterDescriptor` |
| **Resolved inputs + verified pins** | Skills, instructions, servers, **repo-local executable payloads + declared content roots** (§8), and relevant runtime/image identity. | Partial (`frozen_servers` only) | `resolve::FrozenServer`, lock model, D3 |
| **Effective policy + provenance** | Compiled ruleset + policy-input identity/provenance (which machine + project policy produced it). | Partial (ruleset in `spec.ruleset`; no provenance) | `CompiledRuleset`, `machine_policy` |
| **Secret authorization** | Secrets authorized **by reference name and capability scope, with lifetime** — never resolved values. | Absent | resolved per-call in `Gateway::build` today |
| **Posture + effects** | Confinement posture, egress mode, workspace roots (placeholder), artifact mode, cleanup/mutation intent. | Partial (`posture()`, `lockdown`) | `Posture` |

### 6.2 `RunEnvelope` — evidence identity around a specific grant

Wraps one live run: **run id**, **recorder identity**, and
**`digest(AuthorityGrant)`** — the single place the grant digest lives. Every
material recorder event carries the `RunEnvelope`'s grant digest (available from
`GrantFrozen` onward, §3 step 6); gate refusals before the freeze have no grant
digest by construction (§9). `--plan` produces an `AuthorityGrant` but **no**
`RunEnvelope`.

**Rule (D2):** freezing this list is a naming/plumbing exercise, not new
enforcement. Do **not** implement Workspace Grants, hosted adapters, or secret-
lifetime enforcement in Phase 0A — reserve the fields. The one exception the
rulings pull *into* Phase 0A is D3 local-executable pins (§8), a real input.

---

## 7. D2 — one enforcing grant across delivery paths 🔒

Four independent authority reconstructions exist, and **none is yet a sufficient,
enforcing grant builder**:

1. `ExecutionPlan::build` (sandbox) — *records* trust but does **not** enforce it
   (execute path warns and continues, `sandbox.rs:408-418`), and runs only
   **partial** lock verification (pin failures preserved for skipping,
   `resolve.rs:330`). The **seam to extend**, not a sufficient grant.
2. `Gateway::from_frozen` — consumes a pre-compiled ruleset + frozen servers
   verbatim (the right "one resolution, one ruleset" shape) and re-checks trust,
   but returns **empty-gateway, not run-abort**, on untrusted (`gateway.rs:566`).
   Correct as a gateway-local control; the whole-run abort is the caller's job
   (§3 step 6), and emptiness alone is **not** the signal (a valid zero-server
   profile is also empty — `sandbox.rs:843`).
3. `Gateway::from_manifest` / `from_manifest_lease` — no hard trust gate at this
   layer; recompiles from disk.
4. Native render / `apply` / `session` — no trust check, no pin verification,
   recompiles the ruleset fresh each call (`render/apply.rs`).

**D2 sequencing (round-2 ruling 7 — resolves the earlier contradiction):**

- **Every render / session / lease path a locked run touches lands in the Phase 0A
  `--locked` implementation.** When a locked run applies a profile, that
  render/session path consumes the same frozen `AuthorityGrant` — trust enforced
  and strict lock verify **before** any native write, rendering from the frozen
  resolved set rather than a fresh re-resolve/re-compile. A lease established under
  a locked run is built fail-closed with a hard trust gate consuming the grant.
- **Standalone-command unification** (plain `agentstack apply` / `session` / MCP
  lease invoked outside a locked run) **must complete before the Phase 0A exit
  gate** — not merely be recorded as debt.
- **Proof obligation (Phase 0A gate):** a test showing the locked host run, the
  **`--locked --sandbox`** run, the gateway, and any render/lease path a locked
  run touches all consume one `AuthorityGrant`, and no path can widen authority
  relative to it. (Proof references say `--locked --sandbox`, not the ambiguous
  plain "sandbox run," unless §12-D is separately adopted.)

---

## 8. D3 — local executable integrity 🔒 (ruling: Option A, with content roots)

**Current state:** local executables are **not** integrity-pinned. A stdio
server's `command = "./scripts/foo.sh"` pins the *declaration text*
(`resolve.rs:217-241` hashes the `Server` table), never the file's bytes. Editing
the script post-trust changes behavior and re-gates nothing.

**Ruling (Option A):** repository-local executable payloads are **content-pinned
before the first claim-bearing release/demo**, under strict verification (§3
step 4). The declared integrity surface includes:

- a **repository-relative stdio `command`** (resolves to a path inside the
  project) — pinned by the target file's current bytes;
- **local interpreter-script arguments** — e.g. `command = "python",
  args = ["./tools/agent.py"]` or `command = "node", args = ["scripts/run.js"]` —
  where an `args` entry resolves to a repo-relative file, that file's bytes are
  pinned;
- **declared content roots/bundles for interpreted payloads (round-2 blocker 5).**
  Pinning only the entry script is insufficient: a stable `agent.py` can
  `import payload.py`, a shell script can `source` a changed file, a Node entry
  can `require` a changed module. So a manifest that declares an interpreted
  local payload must declare its **integrity root(s)** — a directory subtree
  pinned by content digest, cache-free — and a one-byte change **anywhere in a
  declared root** fails verification and re-gates.

  🔒 **`dir_digest` cannot be reused unchanged for this (round-3 correction 2).**
  The existing `agentstack_core::digest::dir_digest` deliberately **skips every
  symlink** (`crates/core/src/digest.rs:80`); an interpreter can still follow a
  symlink inside a declared root and execute unpinned content the digest never
  covered. Integrity-root digesting therefore requires **recursive symlink
  handling**: either **reject all symlinks** within a declared root as a hard
  error, or **resolve them and include only targets canonically contained within
  the project** (a link escaping the project root is a hard error, per the
  canonical-path rule below). A dedicated root-digest routine implements this; the
  skip-symlinks `dir_digest` is not authoritative for integrity roots.

Behavior and honest boundaries:

- The lock gains content pins for these inputs; the trust preview displays them;
  `doctor` warns on executable-but-unpinned local code.
- A one-byte edit to a pinned file or declared root fails strict verification
  (§3 step 4) and re-gates review (re-pin changes the lockfile bytes → the trust
  digest flips — the existing re-gate chain).
- **Canonical-path/symlink handling:** paths are canonicalized; traversal or a
  symlink that escapes the project root is a hard error, not a silently-followed
  link.
- **Explicitly unbound, and labeled as such:** the resolved harness/interpreter
  binary itself (`python`, `node`, `/usr/bin/env`, `claude`) is an external
  `$PATH` binary and is **not** pinned (§3.1). Transitive imports **outside** a
  declared integrity root are **unbound** and must be labeled as such in `--plan`
  and the trust preview — the guarantee is "declared repository-local executable
  content is content-bound," never "every byte the harness can execute is pinned."

The distinction that must not blur: the pin binds **repository-controlled
executable content** (the clone-as-consent threat); it cannot bind trusted system
interpreters or code outside a declared root.

---

## 9. Recorder events this contract introduces 🔒

The locked run must record what it decided — including refusals — and the recorder
must be able to **fail the run** when a material event cannot be successfully
appended (round-2 blocker 2): `RunLog::append` swallows write failures today
(`recorder/src/lib.rs:403`), which does not satisfy a contractual refusal-evidence
guarantee. Material events use a **checked append**; best-effort telemetry may keep
the swallowing append.

Event model (resolves the round-2 sequencing contradiction — the run-started event
must not require a grant digest that does not exist yet):

- **`AttemptStarted`** — run id + invocation identity, **no grant digest**. Emitted
  in §3 step 2, before any gate.
- **Trust decision** and **policy-admission** events — the observed decision and,
  on refusal, a **terminal outcome** event. These occur **before** the grant is
  frozen and therefore carry **no grant digest** — a gate failure naturally has
  none.
- **`GrantFrozen { grant_digest }`** — emitted in §3 step 6 once the
  `AuthorityGrant` is frozen. From here on, every material event carries the
  `RunEnvelope`'s grant digest (§6.2).
- **Lifecycle outcome** — exit status, duration, and **token/cost evidence with
  honest provenance**: a host run does not observe the provider's usage
  accounting, so the field carries either evidence AgentStack actually observed
  (provenance noted) or an explicit **`unavailable`** state — never a fabricated
  value or a zero standing in for "unknown."
- **Evidence redaction:** any recorded argv/argument representation is keyed-
  digested or redacted, never verbatim (§4).

Whether these are new `RunEvent` variants or extensions of existing ones is an
implementation choice; the *contract* is: refusals are recorded (or a failure to
record a refusal is itself surfaced alongside the refusal, §3 step 2), the run
aborts — never launches — if a material event cannot be successfully appended
(checked write success, not `fsync`), grant digests appear only from `GrantFrozen`
onward, secrets and raw argv never appear, and token/cost is never fabricated.

---

## 10. Demos this contract must make provable (Phase 0A gate)

1. **Safe repo, no Docker:** `run H --locked` on a trusted repo resolves, opens the
   recorder (`AttemptStarted`), trust-checks, strictly lock-verifies, compiles
   policy + admits, freezes the `AuthorityGrant` (`GrantFrozen`), launches with
   host guards and launch-scoped MCP config, records the outcome, cleans up.
2. **Policy admission failure, no Docker:** the repo declares a request that is
   actually rejectable before launch — a **declared MCP/egress host** (or a
   **declared secret reference**) the machine policy **denies**, or an
   **unclassifiable host** (a `${REF}` in the host portion) → blocked before
   launch, machine layer named, refusal recorded. (A tool allowlist is a
   constraint, not a rejectable request; a local executable is an integrity input,
   not an admission request.)
3. **Drift, no Docker:** a pinned input changes → strict verification blocks before
   launch, offender named, directed to `agentstack lock`, refusal recorded.
4. **Maximum assurance (separate, Docker):** `--sandbox` and `--lockdown` behavior
   demonstrated apart from the quickstart (unchanged from today).

Invariant/regression witnesses:
- one-grant consumption across the locked run, the **`--locked --sandbox`** run,
  the gateway, and any render/lease path a locked run touches (§7);
- authoritative-hash-not-cache on the locked skill-verification path, as its own
  regression test (§3 step 4, ruling 3);
- a one-byte edit to a pinned repository-relative executable **or anywhere inside a
  declared integrity root** re-gates (§8);
- a **symlink inside a declared integrity root** is either rejected or resolved to a
  canonically-contained target, and a symlink escaping the project root is a hard
  error — the skip-symlinks `dir_digest` is not used for integrity roots (§8);
- a **valid zero-server** trusted profile yields a valid (non-abort) run with
  ambient native MCP entries still shadowed (§3 step 6);
- a refusal (trust or admission) produces a terminal outcome event with **no** grant
  digest, while post-freeze events carry one (§9);
- a run aborts (never launches) if a material recorder event cannot be successfully
  appended, and a failure to record a refusal is surfaced alongside the refusal (§9);
- the exact invocation is keyed-committed in the grant digest, and recorded argv is
  redacted/non-correlating — never verbatim, never an unkeyed fallback (§4);
- `--locked --plan` mutates nothing, invents no run id, prints+digests a complete
  `AuthorityGrant`, and exits nonzero when a live launch would be refused (§2.2).

---

## 11. Summary of what this contract commits to

- `--locked` is an opt-in, fail-closed **promotion of the host run**:
  recorder-open (`AttemptStarted`) → trust (enforced) → strict lock verify →
  policy admission → freeze `AuthorityGrant` (`GrantFrozen`) → build gateway
  (fallible, zero-server-valid) → launch-scoped MCP config → recorded outcome, no
  Docker.
- It reuses the trust, verify, policy-compile, freeze, and `Gateway::from_frozen`
  seams by **extending them into strict/enforcing behavior**; no parallel authority
  path.
- Corrections in this revision: empty gateway is valid (fail on unsafe status, not
  emptiness); recorder uses `AttemptStarted` → `GrantFrozen` with a checked append;
  the grant digest lives once, in `RunEnvelope`, over a separate `AuthorityGrant`;
  caller argv is never recorded verbatim; local executables are an integrity
  dimension (not admission); admission blocks unclassifiable hosts; D3 covers
  transitive imports via declared content roots; D2 sequencing is fixed.
- Per rulings: D3 local-executable pinning (Option A + content roots) ships before
  the first claim-bearing demo; the skill-cache bypass is a hard prerequisite; the
  frozen grant carries full backend-neutral identity.

---

## 12. Open items and follow-on sequencing (not blockers to approving this contract)

- **§12-A — done.** STRATEGY.md's assurance table now reads "opt-in Protected
  quickstart for Phase 0A (a candidate default at the Phase 0A exit gate)."
- **§12-D — open (approved to remain open).** Whether plain `--sandbox` should adopt
  `--locked`'s strict gate by default. Related to D5's sandbox-posture decision.
- **§12-E — resolved.** D2 sequencing is settled in §7: locked-run-touched paths
  land in the Phase 0A implementation; standalone-command unification completes
  before the exit gate.
