# Simplify agentstack's user surface — three changes, zero feature loss

## Context

A UX review found the product pitch ("3 commands, 60 seconds") doesn't match reality:
48 top-level commands (33 hidden from --help), ~40 user-facing concepts, and a happy
path that doesn't finish the job it promises. Three fixes were agreed:

1. **`setup` finishes the job** — materialize the active profile's skills (today only
   `use <profile> --write` does that) and tell the user to restart their agent CLI.
2. **Collapse the verb families** — onboarding 4→1 visible entry, lockfile 3→1,
   reporting 6→1, packaging grouped — without losing any functionality.
3. **Progressive disclosure** — `doctor` shows only sections relevant to the project
   (plus anything warning/erroring); `doctor --all` shows everything. README
   restructured so the everyday loop comes before governance.

Hard constraint: **no feature may be lost or weakened.** Only names, grouping, and
default visibility change. No external users exist, so no deprecation shims — but
every doc, test, and demo script referencing an old name is updated in the same change.

---

## Item 1 — `setup` finishes the job ✅ (implemented 2026-07-17)

### Key facts from exploration

- `setup.rs` never materializes skills. Its write pass calls
  `apply::write_quiet(...)` with `quiet=true`, which even suppresses apply's existing
  "skills are not rendered by apply" reminder (apply.rs:678-687 fires only when
  `!quiet`). The closing hint (setup.rs:151-163) picks `profiles.keys().next()` for
  display only.
- The only end-to-end skills materializer is `use_profile::activate()`
  (use_profile.rs:106-457): resolves profile skills (`resolve_active_skills`), calls
  `render::skills::plan/materialize` per target, records lock + state, manages the
  .gitignore block. `prepare()` + `activate()` are already reused by `session.rs` —
  that's the established seam.
- Calling `activate()` from setup introduces failure modes setup never had:
  hard bail on lock drift (`verify::ensure_activatable`), executable-pin derivation
  bails, first-time lockfile writes, redundant server re-render, and a final bail when
  targets are blocked. These must be softened, not inherited.
- `use` requires a named profile — there is no default-profile concept anywhere
  (`UseArgs.profile: String`, required). Inline `[skills.*]` without any profile are
  unreachable by design (only `lock.rs` warns about that state).
- The codebase's one existing "restart your CLI" precedent is
  `plugins.rs:1019-1025` `print_sync_guidance()` — a `"Next:"` block.

### Change

**Files: `crates/cli/src/commands/setup.rs`, `crates/cli/src/commands/apply.rs`
(reminder text), `README.md`, `docs/reference.md`**

1. **Profile selection in setup** (after the apply write pass, before doctor):
   - `--profile` given → use it.
   - Exactly one profile in manifest → select it automatically (say so).
   - Multiple profiles, interactive → prompt with the first-declared as default.
   - No profiles → skip materialization; if inline `[skills.*]` exist, print
     guidance to define a profile.
2. **Materialize via the existing seam**: call `use_profile::prepare()` +
   `activate()` (same pattern as session.rs), wrapped in a soft-fail: on `Err`,
   print the error as a warning plus the exact recovery command
   (`agentstack use <profile> --write`) and continue to doctor — setup must not
   die after configs are already written. No new materialization code; zero
   behavior change to `use` itself.
3. **Restart hint**: extend setup's closing block (setup.rs:151-163) with a
   `Next:` section modeled on `print_sync_guidance()`:
   "Restart or reopen your agent CLI(s) so they pick up the new config."
   Add the same one-liner to `apply`'s non-quiet write path when anything changed.
4. **Fix the dead-end reminder**: apply's skills reminder currently tells you to run
   `use <profile>` even when no profile exists. Reword: name the actual profile(s)
   when they exist; say "define a profile to activate them" when none do.
5. **Docs**: update README "Start in 60 seconds" + the skills caveat
   (README.md:103-110) — setup now completes skills; plain `apply` still doesn't
   (unchanged). Update docs/reference.md "Selective skills via profiles" (mirror
   the "apply (and therefore setup) compiles…" phrasing from reference.md:411-422).

### Tests

- Extract the new setup phase as a testable helper (e.g.
  `setup::materialize_profile(ctx, profile)`); integration test in
  `crates/cli/tests/setup.rs`: manifest with one profile + one local skill →
  helper materializes skills into the target's skills dir and pins the lock.
  (setup::run's confirm prompt returns false non-interactively, so the helper is
  the testable seam — same approach the existing tests take with preflight.)
- Existing `render/skills.rs` + `use_profile.rs` unit tests are the safety net for
  the primitives; unchanged.

---

## Item 2 — collapse the verb families ✅ (implemented 2026-07-17; visible surface now 14 commands)

### Key facts from exploration (blast radius verified)

- **No alias mechanism exists** in the codebase (zero `visible_alias`/`alias` hits);
  the only progressive-disclosure pattern is `#[command(hide = true)]`
  (cli.rs:44-51). Dispatch matches by enum variant, so regrouping is display+enum
  work, not behavior work.
- `docs_commands.rs` is **one-directional and substring-weak**: stale docs naming a
  deleted command will NOT fail CI. The docs sweep must be done by hand.
- `after_help` (cli.rs:18-31) lists 27 commands in 5 groups and already has a bug:
  `analyze` and `proxy` are not hidden, so `--help` prints them twice.
- **Critical compat finding**: `connect::bridge_server` (connect.rs:250) hardcodes
  `args = ["mcp", ...]` and writes it into every harness config on disk
  (`~/.claude.json`, Codex config); `locked.rs:503-513` reuses the same helpers for
  `run --locked` grants; `mcp_lease.rs`, `doctor_ci_gate.rs` fixture,
  `trust-gate-demo.sh`, and the lease example all invoke literal `agentstack mcp`
  (~70 references). **Renaming `mcp` breaks every existing installation.**
- Dashboard seams that must keep working: `analyze::collect` + `stats::collect`
  (dashboard/snapshot.rs:596-598), `crate::runs::list/kill` (server.rs:165,216),
  `crate::proxy::aggregate` (snapshot.rs:97), `connect::has_bridge_entry`
  (snapshot.rs:575, doctor.rs:233). All are module functions, untouched by CLI
  regrouping.
- `upgrade.rs` (704 lines) imports internals from `add`/`remove`/`install`;
  `update` has no file of its own (`install::run_update`); `lock.rs` resolves via
  `use_profile`, not `install` — three commands, one `Lock` data structure,
  implementations stay where they are.
- `pack.rs` is 92 self-contained lines (scaffold only, one sub-verb, 3 references
  repo-wide, no shared code with `lib`).

### Change

**Files: `crates/cli/src/cli.rs`, `crates/cli/src/main.rs`, small moves in
`crates/cli/src/commands/{bootstrap,setup,init,lock,pack,lib,optimize,sandbox}.rs`,
plus the docs/examples sweep listed below.**

1. **Delete `bootstrap`** (enum variant + `bootstrap::run`); move `preflight()` into
   `setup.rs` (its only caller). Update hint strings: init.rs:503, setup.rs:38
   (non-interactive path now suggests `agentstack apply --write`). Docs: README
   (3 sites), catalog skill `using-agentstack`, reference.md. Demo:
   `examples/sandbox/demo-firstrun.sh:68` and `tools/make-term-svgs.py:219` switch
   to the new scripted loop (`init → apply --write → use`), regenerate
   `docs/firstrun.svg`.
2. **`lock` absorbs `update` + `upgrade`**: delete both enum variants; `LockArgs`
   gains `--update [NAME]` (dispatches to existing `install::run_update`) and
   `--upgrade <NAME>|--all` plus the pass-through flags upgrade needs
   (`--yes`, `--with-instructions`, `--write`), dispatching to existing
   `upgrade::run`. `install.rs`/`upgrade.rs` implementations unchanged. Literal
   `agentstack update`/`agentstack upgrade` have ~0 doc hits; doctor's 11
   `agentstack lock` hints stay valid unchanged.
3. **`report` becomes the one reporting umbrella** (visible):
   `report run <id>` (today's `report <id>`), `report runs` (today's `runs`),
   `report usage` (today's `stats`), `report calls` (today's `analyze`).
   Delete top-level `Runs`/`Stats`/`Analyze` variants; `kill` stays hidden
   top-level (it's an action, not a report). `audit` keeps its security-scan role;
   `proxy report` stays under `proxy` (different data source, tied to proxy
   lifecycle). Implementation files stay; only dispatch changes — dashboard seams
   untouched. Hint-string sweep: sandbox.rs (6 sites `agentstack report <id>` →
   `report run <id>`), optimize.rs:548 (`stats --live` → `report usage --live`),
   explain.rs:278, proxy.rs:12, footprint.rs:9, recorder/src/lib.rs:263,
   docs/ENFORCEMENT.md:309,400, docs/ARCHITECTURE.md:388, docs/dashboard.md:50,101,
   README.md:425, catalog skills (analyze-usage, orchestrate-workflow,
   using-agentstack), `examples/sandbox/demo-lockdown.sh:110`
   (`as report "$run_id"` → `as report run "$run_id"`). Fix the duplicate
   `analyze` rows in reference.md's All-commands list while there.
4. **`pack init` → `lib pack-init`**: move the 92-line scaffold under `LibCmd`,
   delete the `Pack` variant. reference.md:543 updated. (The unrelated
   vendor-pack terminology collision is noted but out of scope.)
5. **`gateway` groups the human-facing bridge commands**:
   `gateway connect` / `gateway disconnect` (move `ConnectArgs`/`DisconnectArgs`
   under a `GatewayCmd`; implementations stay in connect.rs — `locked.rs`,
   dashboard, and doctor call module functions, not the CLI). **`mcp` stays a
   hidden top-level command, unchanged** — it is the machine-invoked entrypoint
   written into on-disk harness configs (like `guard check`); renaming it would
   break every existing registration for zero UX gain. `trust` stays top-level
   (load-bearing in all security docs; it's about the project, not the gateway).
   Hint sweep: README:137,374, doctor.rs:244, init.rs:369, self_cmd.rs:59,
   mcp_server.rs:215, reference.md, catalog skill orchestrate-workflow.
6. **Rewrite `after_help`** (cli.rs:18-31) for the new surface and fix the
   analyze/proxy double-listing. Visible command set after this change (~14):
   setup, init, add, search, apply, use, run, doctor, report, trust, guard,
   secret, dashboard, instructions. Everything else hidden-but-functional,
   grouped in after_help.
7. **Docs sweep is manual** (docs_commands.rs won't catch stale names): README
   table + prose, docs/reference.md All-commands list and per-feature sections.

### Tests

- `docs_commands.rs` passes with the updated reference.md (new subcommand names
  present).
- Existing tests unaffected by design: `mcp_lease.rs` and the `doctor_ci_gate.rs`
  fixture invoke `mcp`, which doesn't move; vendor-pack and upgrade tests call
  implementation functions, not CLI names. `bootstrap.rs`'s one unit test moves
  with `preflight` into setup.rs.
- One new smoke test: `Cli::command().debug_assert()` already runs via clap on
  every parse; add a test asserting the new subcommand tree parses
  (`lock --update`, `report run <id>`, `gateway connect`, `lib pack-init`).

---

## Item 3 — `doctor` progressive disclosure + README restructure ✅ (implemented 2026-07-17; posture section kept always-visible per its in-code security rationale)

### Key facts from exploration

- `doctor.rs` builds `Report { sections: Vec<Section> }`; `Section.lines` carry
  `"ok"|"warn"|"error"` tags (doctor.rs:30-41). Counters increment independently of
  printing (`quiet` only guards `println!`), so hiding sections **cannot** affect the
  `--ci` gate (`if args.ci && report.errors > 0`, doctor.rs:130-132).
- 14 sections; "Central library" and "Policy" are already conditionally created —
  precedent exists. Each section already loads the data needed for a cheap
  "relevant to this project" signal.
- No test asserts full section lists/order/snapshots; all doctor tests look up
  sections by title in fixtures that actively use the feature they assert on.
  `collect()` (dashboard/tests JSON entry, doctor.rs:139) must keep returning all
  sections.

### Change

**Files: `crates/cli/src/commands/doctor.rs`, `crates/cli/src/cli.rs` (DoctorArgs),
`README.md`, `docs/reference.md`**

1. Add `--all` to `DoctorArgs` (display-only; `--ci` and `collect()` imply all).
2. Checks always run exactly as today (counters unchanged). Tag each section
   `relevant: bool` at creation from signals already in hand:
   - Zero-files bridge → any adapter has a bridge entry (`connected > 0`) or trust
     state isn't untouched
   - Drift / Quirks → `manifest.servers` non-empty
   - Skills → manifest skill refs non-empty OR broken symlinks found (host signal)
   - Content scan → any scannable content (skills/servers non-empty)
   - Reproducibility → any lockable content (profiles/instructions/extensions)
   - Plugin recipes → `manifest.plugins` non-empty
   - Machine policy posture → one compressed line when unconfigured ("machine
     policy: open — doctor --all for details"), full section when configured/--all
     (preserves the "open is worth stating out loud" intent, doctor.rs:984-987)
   - Secrets / Instructions / Central library / Policy → already self-gated; keep
   - Adapters & CLIs → always shown (baseline)
3. Display rule: print iff `relevant || section has warn/error || args.all || args.ci`.
   Closing line when anything hidden:
   `"N sections not relevant here hidden — agentstack doctor --all shows everything."`
4. `Report::to_json()`/`collect()` return all sections plus the new `relevant` flag
   (dashboard can group later). No behavior change for `--ci/--fix/--live/--deep`.

### README restructure

Today: Why (trust/guard-heavy) → Install → Start in 60s → Trust gate → Guardrails →
One manifest → Everyday commands → … New order:

1. Why (tightened; forward-links instead of inline enforcement detail)
2. Install → Start in 60 seconds (updated for Item 1; restart hint shown)
3. One manifest, 13 CLIs → Everyday commands (the complete everyday loop)
4. Governance chapter: Trust gate → Guardrails → (pointer to policy/sandbox)
5. Proxy/library/plugins/team/modes/Going further as today, names updated per Item 2

All content is kept — sections move, nothing is deleted.

### Tests

- One new test (by-title JSON idiom): near-empty manifest → default run hides
  irrelevant sections; `report.errors` identical to `--all` run; `--all` shows all.
- Existing doctor tests unchanged (they use `collect()`).

---

## Sequencing

Three commits, in order of risk (lowest first), each leaving the tree green:

1. Item 1 (setup) — small, self-contained, highest user value.
2. Item 3 (doctor + README) — additive display logic + prose moves.
3. Item 2 (CLI consolidation) — largest blast radius; lands only with the full
   reference sweep (tests, docs, examples, demo scripts, action.yml) in one commit.

## Verification

- `cargo fmt --check` && `cargo clippy --workspace --all-targets -- -D warnings`
- Focused tests: `cargo nextest run -p <cli crate>` — setup, doctor_*,
  docs_commands, apply_instructions, gitignore_lifecycle
- End-to-end in a scratch project:
  - `agentstack setup` with a one-profile manifest containing a local skill →
    skills dir populated, lock pinned, restart hint printed
  - `agentstack doctor` → only relevant sections; `--all` → all 14;
    `--ci` exit code identical to pre-change on an erroring fixture
  - After Item 2: every renamed command runs; `docs_commands.rs` passes; demo
    scripts in examples/ still execute
