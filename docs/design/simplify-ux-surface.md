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

## Item 1 — `setup` finishes the job

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

## Item 2 — collapse the verb families

*(Blast-radius exploration still in flight — final references list to be folded in
before implementation of this item. Scope as planned:)*

- **`bootstrap` deleted** as a top-level command; its `preflight()` function stays
  (setup already calls it). README/CI examples move to `setup` and `doctor --ci`.
- **`update` + `upgrade` folded into `lock`**: `lock` (pin, as today),
  `lock --update [name]` (re-resolve git skills = today's `update`),
  `lock --upgrade [pack]` (re-resolve vendor packs = today's `upgrade`).
  Implementations move, logic unchanged.
- **Reporting under one umbrella**: `runs`, `stats`, `analyze`, `report <run-id>`
  become `agentstack report runs|usage|calls|run <id>`. `audit` keeps its
  security-scan role (different job); `proxy report` stays under `proxy` (tied to
  the proxy lifecycle). `kill` stays (paired with `runs`; possibly `report runs
  --kill <id>` NOT done — kill is an action, not a report).
- **Packaging**: `pack init` moves to `lib pack-init` (or stays hidden top-level if
  the explorer finds external references); `plugins` unchanged this pass (its own
  consolidation is a later, larger design).
- **Zero-files bridge grouped**: `connect`/`disconnect`/`mcp` become
  `agentstack gateway connect|disconnect|serve`. `trust` stays a visible top-level
  command — it's load-bearing in all security docs and is conceptually about the
  project, not the gateway.
- Update `after_help` grouping text, `docs/reference.md` "All commands"
  (docs_commands.rs enforces presence), README table, and any demo scripts/examples
  the explorer reports.

---

## Item 3 — `doctor` progressive disclosure + README restructure

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
