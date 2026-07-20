# `add skill --write` activation — mode-aware, on `use`'s primitives (priority 3)

Status: **implemented 2026-07-20** (`a42cd4e`). Implements priority 3 of
[`skills-sh-learnings.md`](skills-sh-learnings.md) §10, completing the
one-preview/one-write transaction that
[`add-skill-source-grammar.md`](add-skill-source-grammar.md) (implemented)
deliberately stopped short of: after manifest + store + lock, the same
confirmed write now **activates** the new skills — mode-aware, skills-only,
on extracted `use` primitives. Grounded in a three-seam survey, then
hardened by a three-lens adversarial review whose findings are folded in
(both 2026-07-20) — most consequentially: the additive helper is a second
path, not a refactor of `activate()`'s (pruning) skills block; delivery
mode is captured from pre-write state so `add`'s own lock write can't
flip a fresh project to clean-at-rest; the copilot-shaped
no-project-dir case is reported instead of inheriting `use`'s silent
skip; and the `refresh_trust` hardening gains the worker barrier
`lease_open` implicitly had. All file:line references verified.

The binding constraints from the reviewed learnings doc:

- **Reuse `use`'s primitives; never invoke full profile activation.**
  Adding one skill must not rewrite unrelated server configuration.
- Mode-aware write: static → materialize; clean-at-rest → manifest+lock
  only; zero-files → manifest+lock, current lease untouched.
- Targets via the existing `resolve_targets` contract; a target that can't
  be materialized is reported, never silently skipped.
- Partial failure has defined per-target reporting.

## 0. What the survey established (the facts the design rides on)

1. **The skills path inside `activate()` is separable but NOT
   additive.** Per target, servers (`use_profile.rs:317-442`) and skills
   (`:444-498`) are independent blocks — but the skills block **prunes**
   (`plan` receives `state.managed_skills(&key)` as `previously_managed`
   and unlinks `to_remove`), records the plan's names as a **full
   replace**, and feeds one *combined* gitignore accumulation shared with
   servers/instructions. Those are load-bearing `use` behaviors
   (profile-switch deactivation). Consequence, confirmed by review: an
   additive primitive **cannot** also serve `activate()` — it is a
   second, add-only path built from the same low-level primitives
   (`skills::plan`/`materialize`, `record_skills`, `usage::bump`, the
   gitignore fns), which is exactly what the binding "reuse `use`'s
   primitives" constraint asks for. `activate()` stays untouched.
2. **The drift gate is subset-safe.** `classify_skill` is pure and
   per-skill; `ensure_activatable` takes slices and accepts an empty
   server slice (`verify.rs:141`).
3. **`record_skills` is a full overwrite** (`state.rs:205-209`,
   `entry.managed_skills = managed_skills`). Recording just the new
   skill would silently untrack every previously managed skill for that
   target — an ownership leak the next `use` perpetuates. **The union
   rule below is therefore load-bearing, not style.**
4. **No record exists of "which profile is active"** — not in state, not
   in the lock. Activation history is unknowable; the design must not
   pretend otherwise.
5. **Delivery mode is derived, never stored** — `overview::detect_mode`
   (`overview.rs:127-137`) recomputes it from rendered-artifacts /
   gateway-connected / trusted / locked signals. **The `locked` signal is
   self-poisoning for this feature**: `add --write` creates the lock, and
   `(no artifacts, no gateway, locked)` reads as CleanAtRest
   (`mode_from_signals`, `overview.rs:70-78`) — `use` documents and dodges
   this exact hazard (`use_profile.rs:546-556`). Therefore: **the mode is
   detected once, from pre-write disk state** (the dry-run preview
   already forces this), and that value is threaded into the write's
   tail; it is never recomputed after `lock.save`. (`setup.rs`'s
   `mode_switch_plan` is private and its strings differ — `add` prints
   its own §3 hint strings; no false single-source claim.)
6. **Zero-files: the trust digest covers manifest + lock**
   (`trust::digest_for`, proptest-guaranteed to change on any byte flip),
   so an `add --write` flips `Trusted → Changed` at the next check. The
   MCP loadable index is computed fresh per call (nothing cached), and a
   lease holds no manifest snapshot — "current lease untouched" is
   structural. But the gateway **caches trust per connection**
   (`ensure_project` resolves once; only `lease_open` and lease
   transitions call `refresh_trust`) — a pre-existing stale window this
   design closes in passing (§4).
7. **Clean-at-rest sessions re-prepare from the manifest at every
   `session start`** — a manifest+lock-only write is picked up next
   session automatically; an *active* session will not see it
   (`session::active` is a cheap check).
8. **Skills were never part of history/undo** — `use` explicitly captures
   no `FileChange` for skill materialization ("additive and reverted by
   `session end`", `use_profile.rs:303-306`). A skills-only path plugs
   into `usage::bump` and the managed-`.gitignore` block, not history.

## 1. The additive helper

New function in `use_profile.rs` (it owns the state/gitignore idioms) —
an **add-only second path** reusing the low-level primitives;
`activate()`'s pruning skills block is deliberately untouched (fact 1):

```rust
/// Materialize `skills` into each target's skills dir — additive only:
/// plan() runs with previously_managed = &[] (an add NEVER prunes), and
/// state records the UNION of the prior managed set and what this call
/// materialized (record_skills is a full overwrite; recording less would
/// untrack live symlinks). Skills-only by construction: no server, hook,
/// settings, or instruction path is touched.
pub(crate) fn materialize_skills_additive(
    ctx: &super::Context,
    scope: Scope,
    target_ids: &[String],
    skills: &[(String, PathBuf)],     // (manifest name, resolved source dir)
    no_gitignore: bool,
) -> Result<SkillsActivation>          // per-target outcomes for the caller to print

pub(crate) struct SkillsActivation {
    pub written: Vec<(String, PathBuf)>,   // target id, skills dir
    pub conflicts: Vec<(String, String)>,  // target id, skill name (user-owned dir kept)
    pub unsupported: Vec<String>,          // targets with no skills support
    pub failed: Vec<(String, String)>,     // target id, error (sanitized)
}
```

Per target: `skills_dir_for(scope, dir)`; when absent, **both** absent
cases report into `unsupported` — `desc.skills.is_none()` ("skills not
supported by this CLI", `use`'s wording) **and** the copilot-cli-shaped
case (`skills` present, no `project_dir` at project scope → "no
project-scope skills dir"). Review corrected the draft here: `use`
*silently skips* the second case today, which violates the binding
"reported, never silently skipped" decision — this helper closes that
gap for its own path (fixing `use`'s copy of the quirk stays a named
follow-up). Then `plan(dir, strategy, skills, &[])` → conflicts reported
(`⚠ … left as is`; **conflicted names are excluded from the recorded
union** — `plan.managed_names()` already excludes them, so a user-owned
dir is never claimed as managed) → `materialize` →
`state.record_skills(key, union(state.managed_skills(key),
plan.managed_names()))` → `usage::bump`. Project scope additionally runs
the same `Managed{skills: …}` → `managed_entries` → `ensure_block`
gitignore sequence `use` runs — without it the new symlink dir shows up
as git-dirty. (This helper writes a skills-only block; it never touches
`use`'s combined servers+instructions accumulation.)

When `written` comes back empty and only `unsupported` is populated, the
command prints a warning naming every unsupported target plus the
`use --target … --write` follow-up, and **exits zero** — the skill is
declared and pinned, which is a successful add; the warning keeps it from
reading as an activation.

**Failure semantics** (the learnings promise, made concrete): each target
is all-or-nothing (state recorded only after its `materialize` succeeds);
a failing target lands in `failed` with a sanitized error and the loop
continues; the caller prints per-target `✓`/`⚠`/`✗` lines and exits
non-zero if anything failed, naming exactly what did and didn't happen.
Nothing needs rolling back across targets — materialization is additive
symlinks/copies, and the manifest+lock write already happened and stays
(the skill is *declared and pinned*; a failed target is a materialization
problem `use --write` can retry, and the error says so).

## 2. When `add skill --write` activates — the ambiguity rule

Static mode only (see §3), and only when the new skills are
**unambiguously part of the default activation** — fact 4 means we cannot
know which profile is active, and materializing a skill from a non-active
profile would be exactly the ambient-skill fencing leak the learnings doc
rejects (§3 there):

| Manifest state | `add skill --write` |
|---|---|
| Zero profiles (implicit default) | **materialize** — the implicit default activates every inline skill by definition |
| One profile, skill enrolled (automatic per P2) | **materialize** — the only profile there is; if it was never activated anywhere, the result is a benign head start `use --write` completes later |
| Several profiles (P2 already required choosing one) | **manifest+lock only** — which profile is live per target is unknowable; print `· activate with \`agentstack use <chosen> --write\`` |

Stated explicitly, because it reads as a contradiction otherwise: the
several-profiles row **deliberately narrows** the learnings doc's binding
"static → manifest + lock + activation" — the delivery-mode rule is
intersected with the profile-fencing rule, and fencing wins when the
active profile is ambiguous. Ambient activation of a possibly-inactive
profile's skill is precisely what the learnings §3 rejected.

Targets: `resolve_targets(&manifest, &registry, &[])` — the exact
`use`/`apply` chain (`[targets].default` → detected adapters → all).
Scope: `Scope::default_for(&ctx.dir)` — project manifests materialize at
project scope, the machine manifest at global. **Known limitation,
accepted:** this is the *default* activation surface, not activation
history — a user who previously activated the sole profile narrowly
(`use <p> --target claude-code`) gets the new skill in every default
target anyway. Gating on recorded managed-state would make a first-ever
activation via `add` impossible, which is worse; the preview names the
target count before `--write`, and `use --target` remains the precision
tool. **No new flags in v1**: `add skill` stays flagless about placement
(`--target`/`--scope` mirrors of `UseArgs` are a listed follow-up if real
usage wants them, not silent scope creep).

The subset drift gate runs before materializing: `classify_skill` +
`ensure_activatable` over just the new skills (empty server slice). After
this command's own lock write they match by construction — the gate is
defense in depth against a concurrent edit between the lock save and the
materialize, and it is nearly free (fact 2).

## 3. Mode awareness

`overview::detect_mode(ctx, target_ids)` is computed **once, from
pre-write disk state** (fact 5's self-poisoning rule: `add`'s own
`lock.save` must never flip the detected mode), and the value is threaded
into the write's tail. The preview names what the write will do before
`--write` is given; the footer matrix, complete:

| | Unambiguous (zero profiles / one enrolled) | Ambiguous (several profiles) |
|---|---|---|
| **Static** | `→ will materialize into <n> target(s)` (write: per-target `✓`/`⚠`/`✗` lines) | `· activate with \`agentstack use <chosen> --write\`` |
| **Clean-at-rest** | `· next session picks this up: agentstack session start <enrolled-or-chosen profile>` | same, with the `--profile`-chosen name |
| **Zero-files** | `· trust re-gates on this edit: run \`agentstack trust .\` to re-consent` | same |

Per mode:

- **Static** → §2's rule; the preview's footer says
  `→ will materialize into <n> target(s)` (or the ambiguity hint).
- **Clean-at-rest** → manifest+lock only (already the P2 behavior).
  Hint: `· next session picks this up: agentstack session start <profile>`
  — the `mode_switch_plan` text, not the wrong `use --write` (which would
  render a *persistent* activation in a repo the user keeps pristine).
  If `session::active(&ctx.dir)` — say so:
  `⚠ a session is active; it won't see '<name>' until the next session start`.
- **Zero-files** → manifest+lock only, **lease untouched** (fact 6 makes
  this structural — nothing to do, and the design says why rather than
  claiming credit). Hint:
  `· trust re-gates on this edit: run \`agentstack trust .\` to re-consent`
  — because the digest now differs, and the next connection/lease serves
  control-plane-only until re-trust. Omitting this hint strands the user
  with a silently dead gateway.

Mode detection is a read-only pass over existing signals; when signals are
mixed the same priority order `detect_mode` already uses applies
(rendered artifacts win → Static).

## 4. In-passing hardening: close the stale-trust load window

Fact 6's tail is a pre-existing gap this feature would widen in practice:
an open gateway connection keeps its `Trusted` snapshot until a lease
transition, so a skill added-and-pinned mid-connection becomes loadable
under stale trust even though the on-disk digest already reads `Changed`.
Before P2, the only agent-reachable add path (`agentstack_add_skill`,
manifest-only, no lock) left inline skills unpinned and therefore
load-refused — fail-closed by accident. A pinning CLI add makes the
window real.

Fix, small but with two corrections the review forced on the draft's
"exactly like `lease_open`" framing:

- `agentstack_load` and `agentstack_session_start` call
  `auto.refresh_trust()` before consulting `trust_note()` — **behind the
  same worker barrier `lease_open` enjoys**. `lease_open` is an
  `is_lease_mutation` request, so `workers.join_all()` runs before its
  dispatch (`mcp_server.rs:251`); load/session_start are not, and
  `refresh_trust`'s trust-flip branch tears down the code-mode runtime
  and swaps the gateway (`:681-685`) — racing in-flight workers exactly
  when the feature matters. The design therefore extends the barrier
  (join before refresh on these tool names), not just the refresh.
- The refresh **supplements** the existing transparent-mode handling —
  `notify_if_gateway_appears` still runs for these calls in transparent
  auto mode; the new call must not short-circuit that arm.

Scope of the claim, corrected: this closes the stale window for the
**auto-project gateway** (the persistent, globally-registered connection
this feature targets). The eager `agentstack mcp` loop passes no trust
note at all and is out of scope here — per-launch, pre-selected project,
a different posture. Cost: one three-file digest hash per load call.
**Security-sensitive: flagged for line-by-line review** — it touches the
trust check path, though it only *adds* recomputation plus a barrier,
never skips a check.

The MCP `agentstack_add_skill` tool stays manifest-only and un-gated
(commit-safe text edit, nothing executes; its unpinned inline skills stay
load-refused — the existing fail-closed shape). Its description already
says activation is a human's `use --write`; still true for that tool.

## 5. Tests

- **Union-rule witness** (the fact-3 trap): seed `state.json` with a
  previously-managed skill for the target key, `add skill --write` a new
  one, assert state now records **both** and a subsequent
  `use --write` does not prune the first. This is the regression that
  silently orphans user state; it gets the explicit test.
- **Activation witness**: zero-profile project manifest +
  `[targets] default=["claude-code"]` → after one `add skill … --write`,
  `<proj>/.claude/skills/<name>/SKILL.md` exists (project scope by
  default), the managed `.gitignore` block contains the skills dir, and
  no server config file was created or modified (the skills-only claim,
  asserted, not assumed).
- **Ambiguity witness**: two-profile manifest → `add … --profile a
  --write` writes manifest+lock but materializes nothing.
- **Existing witness updated**: `add_skill.rs`'s
  `write_lands_…_then_use_materializes` currently documents the P2 split;
  it becomes the activation witness above, and a separate case keeps
  proving `use --write` still works over an `add`-produced manifest
  (different scope, exercising the union rule from the other side).
- The `refresh_trust` change: one focused test if the existing MCP test
  harness reaches it cheaply; otherwise noted as covered by the
  lease-open path's existing behavior (same function, new call site).

## 6. Scope line and touched files

Out of scope, named: `--target`/`--scope` flags on `add skill`; skills
undo/history (doesn't exist for `use` either); the copilot-cli
project-scope reporting quirk (pre-existing, mirrored not fixed);
`add server`/`add from` activation (different surfaces, own designs);
any change to `session`/lease mechanics.

Named follow-ups: fixing `use`'s own silent copilot-cli skip;
`--target`/`--scope` flags on `add skill` if wanted.

| Area | Modified |
|---|---|
| Helper | `use_profile.rs` (new add-only `materialize_skills_additive` from the low-level primitives; `activate()` untouched) |
| Verb | `commands/add.rs` (pre-write `detect_mode` capture, tail fork, activation call, the §3 hint strings) |
| Gateway | `mcp_server.rs` (`refresh_trust` + worker barrier before load/session-start trust checks, transparent-arm preserved) |
| Docs | `docs/reference.md` `add skill` section: the activation behavior + mode table |
| Tests | `tests/add_skill.rs` (updated + union/ambiguity witnesses; the activation witness asserts materialization actually happened — the mode-self-poisoning regression trap) |

No new dependencies, no new unsafe, no changes outside the `cli` crate.
One session.
