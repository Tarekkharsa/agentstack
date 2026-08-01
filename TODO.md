# AgentStack execution roadmap

> **Purpose:** the only ordered product-wide work queue
>
> **Strategy:** [`STRATEGY.md`](STRATEGY.md)
>
> **Updated:** 2026-08-01
>
> **Rule:** finish the current stage gate before starting a later product stage

## How to use this file

- Work from top to bottom.
- Keep one item in implementation or review at a time when it touches a
  security boundary.
- A checked item means implemented and verified, not merely designed.
- Security findings can interrupt the product sequence; speculative features
  cannot.
- Closed implementation history belongs in `CHANGELOG.md` or commit history,
  not in this queue.
- Design documents explain decisions. They do not independently authorize
  roadmap work.

## Current objective

Make AgentStack’s everyday value obvious:

> **Import one agent setup, use it across every coding CLI, switch it by task,
> and recover safely when configuration changes.**

The current sequence is:

```text
confirmed fixes
      ↓
first-value journey in t3code + CLI
      ↓
profiles and sessions
      ↓
lifecycle confidence
      ↓
sharing evidence
      ↓
advanced expansion only if earned
```

### Launch status (2026-07-29)

The public-launch path — `install → init → apply → doctor → restore` — is
shipped and public. **v0.17.0 is the published release (2026-07-29): the
binary the study installs, carrying the dry-run fixes, the gateway
fail-closed fence, and `edit-profile`. Verified post-publish from the public
tap on a clean setup — the full journey runs green and doctor no longer
contradicts a toolset switch.** What separates here from a validated launch:

- **The §1.6 activation study** (the Stage 1 gate), then fixing the three most
  common blockers it finds. It is the one planned activity that can falsify
  the product thesis rather than confirm it.
- Workflows ship visible and honestly labeled experimental; their promotion
  gate (under "Experimental workflows") is separate and does not block launch.

### Strategy v2 — Phase 0 (Instrument), runs alongside the study

The adopted [`STRATEGY.md`](STRATEGY.md) (v2, 2026-07-31) opened with Phase 0
— complete as of v0.17.1 (all four items below shipped). Per the 2026-07-31
amendment in `STRATEGY.md`, Phases 1–4 now build in order on maintainer
acceptance; the activation study is the **v0.18.0 release gate** — the
release does not publish until the study passes against the completed v2
journey. Phases 1, 2, and 3 are complete (blocks below); **next is Phase 4,
Anywhere** — gated on the §1.6 study, which remains the v0.18.0 release gate.

- [x] **P0.1 — study instrumentation.** Add the v2 observation prompts to the
  §1.6 protocol (where does a tester reach for "drop a file" and stall; can
  they restate what they consented to) and baseline the four metrics: TTLC,
  concepts-before-value, review comprehension, recovery time. Opt-in
  observation only — the F19 constraint applies unchanged. (2026-07-31:
  additions-only in `docs/design/activation-study.md` §5 + new §9; protocol
  and pass condition byte-identical.)
- [ ] **P0.2 — trust-store mutation recording.** Grants, re-grants, and
  revocations become recorder events (the planned mitigation already named in
  `docs/ENFORCEMENT.md`). Adds events, never gates. Touches the trust seam:
  supervised, line-by-line review per `CLAUDE.md`. (2026-07-31: merged to
  main — `crates/trust/src/lib.rs` `record_mutation` appends to
  `~/.agentstack/audit/trust.jsonl` inside the store lock, after the store
  write succeeds. Phase 1's provenance clock and its event-parity witness
  both read this log, so it is now load-bearing, not just observational.)
- [x] **P0.3 — structural lint.** Every policy dimension keeps its
  `ENFORCEMENT.md` row; every capability kind has manifest table + lock
  pinning + doctor probe + witness test + an explicit honest enforcement
  statement (the `[hooks.*]` statement landed with this task). (2026-07-31:
  `tools/check-structure.py` + CI job, three adversarial rounds; the 14
  honestly-baselined gaps live in `tools/check-structure-baseline.txt` —
  shrinking that file is follow-on work, settings and the hooks lock pin
  being the substantive entries.)
- [x] **P0.4 — files-you-create-first docs restructure.** First-contact pages
  lead with the tree the user touches; mechanism nouns move to the moment
  they become relevant. Six-rung ladder unchanged. (2026-07-31: README.md and
  `docs/concepts.md` first, then `docs/start.html`, `docs/index.html` and
  `docs/tutorial/` — each now shows the `.agentstack/` tree `init` leaves
  before naming the manifest. The read-deny blocking those three was the
  wildcard `Read(docs/*.html)`; it is now an explicit list of exactly the
  pages that have a `.md` twin plus the three oversized hand-authored ones,
  so twinless pages stay editable. Ladder and `#install`/`#start` anchors
  byte-identical.)

### Strategy v2 — Phase 1 (The funnel) — COMPLETE 2026-07-31

Dropping a file into the project is now a supported, first-class way to author
a capability, and for content you demonstrably wrote yourself it ends in one
confirmation. Two slices, each merged on green witnesses.

- [x] **P1.A — intake detection, provenance, and the offer** (`f5d68ea`).
  Undeclared content in `.agentstack/skills/`/`.agentstack/instructions/` is
  noticed at command time by `status`, `doctor`, `use`, `lock`, and `adopt`,
  and offered with a preview; `adopt` writes the declaration through the same
  format-preserving insertion path `add` uses, with `--to-library` for
  cross-project reuse. Provenance: inside a git work tree tracking decides;
  outside one, the clock is the last recorded grant in `trust.jsonl` (P0.2).
  Adversarial review of the diff found four real defects, all fixed with
  witnesses before merge — git rewriting mtime on checkout promoted pulled
  content to "your own work"; a symlinked `SKILL.md` was followed; a
  same-named drop could replace a pinned git/library declaration behind a
  preview that called it an addition; an adopted skill joined no toolset.
- [x] **P1.B — single-action activation** (`b1bd459`). `agentstack yes`
  performs declare → lock → trust → render behind one review and one
  confirmation, through the same functions and the same `grant_gated` gate
  the explicit path uses. Witnesses: event parity with the explicit sequence
  (same actions, order, and digests); declining leaves the project
  byte-identical with no recorded event and nothing materialized; content
  failing provenance or colliding on name is structurally absent from the
  compressed path; one yes delivers skills *and* instruction fragments. Also
  closed the symlinked-ancestor gap slice A left open.

Scope held: servers still require declaration, hooks and extensions are
excluded entirely, and **re-gates of changed content are not compressed** —
they stay explicit until Phase 2's diff card exists.

Carried into Phase 2, written down rather than acted on:

- **The collision case wants the diff card.** Today a dropped file whose name
  is already declared is refused with an explanation. "This name is declared
  as `git@abc123`; the drop would replace it with repo bytes" is exactly the
  review-card shape, and is the right home for it.
- **Phase 1's gate is now countable.** "Zero consent-surprise incidents over
  the recorded trust-mutation events" is what the decline witness protects:
  every path that could leave a change the user did not agree to has to keep
  failing that test.

### Strategy v2 — Phase 2 (One yes: the review card) — COMPLETE 2026-07-31

The yes is now glanceable, plain-language, and diff-aware, and repeated review
of identical content is cheaper without being weaker. Contract:
[`docs/design/consent-card.md`](docs/design/consent-card.md). Merged to main as
`phase2/slice-b`; every witness tamper-tested.

- [x] **P2.A — the review card + undo-in-preview.** Two to five plain lines
  composed from the walked surface, then the unabridged detail. Fixed a real
  consent gap found while building it: `[hooks.*]` and `[settings.*]` re-gated
  the trust digest when edited but were disclosed **nowhere** on the review
  screen, so grants before this approved hooks the user was never shown. The
  `:review` requirement in `tools/check-structure.py` is what makes the fix a
  property rather than a fact about one commit. Undo is named before the first
  byte changes in `adopt`, `apply`, `use --write`, and `session start`.
- [x] **P2.B — prior bytes.** `~/.agentstack/store/content/` already was the
  content-addressed store; the gaps were a missing pin pointer on `SurfaceItem`
  and no deposit for path sources. Both closed, with the deposit inside the
  pinning act (`Store::pin`, `Store::pin_instruction`) so "the approved bytes
  were captured" is a property of pinning rather than of call-site discipline.
- [x] **P2.B.5 — the three-way answer.** accept / keep-pinned / block, staged
  behind one commit point; blocked exclusion at delivery with a standing status
  line; the collision refusal upgraded to the informed card; instruction
  deposits and their diff card; regrant event parity.
- [x] **P2.C — recognition.** Machine-local approvals index keyed by content
  digest. Shortens the card's body and nothing else — witnessed
  outcome-invariant, event-invariant, and content-free.
- [x] **P2.D — the panel contract.** `trust-review-card-v1` ships; the preview
  gained hooks, settings, policy, and the machine ceiling, and
  `ui_contract.rs`'s false "full reviewed surface" claim is now true.

**Carried forward by name, so none of it is rediscovered as a bug:**

- **The two deliberate preview omissions.** (a) A library server whose live
  definition has drifted from its lock pin stays redacted in the JSON, so an
  external UI cannot bind consent to bytes the digest does not cover — the
  terminal review deliberately does the opposite. (b) The JSON's blockers cover
  servers and local executables only; the full per-kind set needs resolution
  steps that reach git worktree materialization, which must not happen on a
  read-only command. Documented in `crates/cli/src/ui_contract.rs` and
  `docs/design/consent-card.md` (§Panel). **Future work only if the panel
  actually needs them**, and only with the write question answered first.
- **Machine-level standing approval** ("always allow this exact content
  anywhere on this machine") is **unbuilt by design**. It is a widening of the
  gate rather than a presentation of it, and gets its own review.
- **t3code fork rendering against `trust-review-card-v1`** — follow-up in the
  other repo. The CLI contract it was blocked on now exists; the fork was not
  opened this phase.
- **Two historical under-disclosure entries are in `CHANGELOG.md`** and must
  both survive editing: the terminal review omitted hooks and settings before
  Slice A, and the machine-readable preview omitted them before P2.D.
- **A refactor hazard.** `PendingAnswer.blocker_ix` is a positional index into
  one global `blockers` vector. Any future decomposition of the walk must key
  blockers by `(kind, name)` first — misaligning it would let accepting one
  item clear an unrelated item's blocker, emptying the vector so the bail never
  fires and an unpinned surface is trusted. Silent, and the most dangerous
  shape in this area.

**Gate to Phase 3:** review-comprehension metric improves against baseline, and
zero instances — counted over recorded grant events — of a user saying yes to
something the card did not surface. Phase 3 starts in a fresh session.

### Strategy v2 — Phase 3 (Four ideas) — COMPLETE 2026-08-01

The visible product is Setup, Toolset, Status, Undo, plus the yes — and the
seatbelt explains itself. Merged to main as `phase3/four-ideas`; every witness
tamper-tested. No release cut: Phase 4 and the §1.6 study remain ahead.

- [x] **P3.1 — status as one next action, and green that means verified.**
  `doctor` ends with exactly one recommended command, always; `next_action` in
  the JSON is no longer nullable, because `state` already answers "is anything
  wrong?". The inherited false-ready bug is fixed the honest way: a check that
  examined nothing says so in words at a new `Level::Unchecked`, rather than
  reporting a green pass. Six sibling emptiness branches moved with it. The
  class guard — no green line anywhere may claim a pass over nothing examined —
  is what makes it a property rather than a fact about one check.
- [x] **P3.2 — seatbelt legibility.** All four denial families produce one
  plain sentence (what, why, safe next step) with a per-family reassurance
  clause. The two families that recorded nothing — host-path egress and
  secret-scope — now do, following P0.2 exactly: additive, identity-shaped,
  best-effort, never gating. One recorder addition, `RunEvent::SecretDenied`.
  `docs/ENFORCEMENT.md` gained the per-family honesty table in the same change:
  recorded is not prevented, and the host-path egress row stays `coarse`.
  The invariant is structural — `seatbelt::refuse` returns `()`, so composing a
  denial cannot hand a call site permission. Fixed while capturing transcripts:
  a policy refusal was being given the missing-value's advice ("set it with
  `agentstack secret set`"), which is work that cannot help.
- [x] **P3.3 — the undo timeline.** `agentstack undo` is the interactive face
  of `restore`: recent changes newest-first, pick a point, revert to it. No new
  destructive machinery (a witness asserts it contains no filesystem call);
  nothing unrecorded is ever offered. `history::undo_recorded` captures the
  pre-undo bytes so the revert is an ordinary ledger row — before this, the one
  action that changed the user's files was the one action the ledger did not
  contain.
- [x] **P3.4 — vocabulary completion.** Default `--help` and `status` speak the
  four ideas; the mechanism glossary moved to `--help --all`, which now opens
  by defining it. Nothing was renamed — `cli_surface` and `t3code_parity` are
  green unmodified except the deliberate `undo` addition below, and
  `git diff main -- README.md` is empty, so the six-rung ladder is
  byte-identical. The witness asserts both directions: no mechanism noun on
  first contact, AND all four ideas named (a help screen that said nothing
  would pass a subtraction-only check).
- [x] **P3.5 — kind convergence.** `explain` covered 3 of 7 kinds and told the
  other three they did not exist, while the review card listed all seven —
  `explain` disagreed with the surface the user had already said yes on. Now
  every in-scope kind answers the same four questions in the same order, with
  empty rows stated rather than omitted. Hooks and extensions share the
  presentation shape and keep the full ceremony; the "what it asks" row is
  where they visibly differ. `status` counts every declared kind (was 2 of 7);
  `[settings.*]` gained the `doctor` section it never had. Presentation only —
  a witness asserts the seven-kind manifest shape is untouched. Structural lint
  baseline shrank 10 → 9; no entry added.
- [x] **P3.6 — dynamic-default preconditions assessed, not flipped.** All three
  unmet, as expected. (1) Open question #1 (where the yes lives in zero-files
  mode) is still open and still self-declares as the blocker. (2) The
  `ENFORCEMENT.md` lease column does not exist — the matrix is still four
  columns. (3) First-run friction is unmeasurable until §1.6 runs. The flip
  stays off; Phase 4 inherits this assessment unchanged.

**Deferred by name, so none of it is rediscovered as a bug:**

- **The server and skill explain lenses are not on the shared row shape.** They
  predate it and are the trust lens the consent card composes from, so
  converging them is its own change. Tracked by
  `servers_and_skills_are_not_yet_on_the_shared_row_shape`, a
  **failing-when-fixed** test: it passes today and fails the day someone adds
  the rows, which is exactly when the follow-up needs attention.
- **Authoring paths for the executable kinds.** `add` covers 2 of 7 kinds and
  extensions have none at all (`add.rs` bails outright). Adding one is a
  consent-surface change: hooks and extensions keep the full ceremony, so any
  attempt gets a line-by-line review of the gate, not just the authoring code.
  No schema change was required for Phase 3's presentation work, so this is a
  scope decision rather than the "stop and report" trigger firing.
- **`undo` was added to the visible command surface deliberately.** Undo is one
  of the four ideas and the one a person reaches for while something is already
  wrong — it must be findable without knowing that `restore` is its other name.
  `cli_surface`'s visible list and the full map's new `Undo` group both record
  the reasoning.
- **The locked-run fifth denial family** (`locked.rs:191,217`, `GateDecision`)
  is real and out of Phase 3's stated four. It is the only denial recording
  that is fail-closed rather than best-effort, which is a property worth
  keeping in mind before anyone "unifies" it with the seatbelt seam. Its lane
  is the locked-run work, not the seatbelt.
- **The history ledger caps at 40 entries and `session end` silently consumes
  them** (`session.rs:460`). The undo timeline inherits both. Its lane is the
  ledger, not the timeline.

### P3.7 — status contract honesty (opened by Phase 3, not fixed by it)

`doctor` reports `state: "ready"` for an untrusted, never-activated project:
zero errors and zero warnings is true, and "ready" is not — nothing it declares
is active. This is the false-ready family again, this time inside a declared
contract with panel consumers, which is why Phase 3 left it alone. **It is the
same bug the E2E's F1 Ready-mislabel finding saw from the fork side.**

Three symptoms, one item, because they are one question — does a machine-readable
status surface say only what the evidence supports?

- `state: "ready"` over an untrusted / never-activated project (`doctor.rs`
  `Report::state`).
- `snapshot.rs:804` emits a plural `nextActions` array where `status` and
  `doctor` both now emit exactly one — a list where a decision belongs.
- `doctor` prints `✓ <REF> resolved from env` for a ref that every server's
  `[policy.secrets]` refuses. Technically true, misleading in context, and the
  same shape as the vacuous green P3.1 removed.

**Do it as a contract revision, versioned** the way `trust-review-card-v1` was
— a new version consumers opt into. Do **not** mutate `status-v1` semantics in
place: a panel that renders "Ready" from today's field would silently change
meaning under its users, which is the failure this item exists to remove.

### P3.8 — the silent pin-verification skip

`gateway.rs:766` drops a server when pin/lock verification fails, with a bare
`eprintln!` and no recorded event. It is fail-closed and correct; it is also
**the most security-relevant unrecorded refusal in the product** — the content-
pinning refusal, the one that fires when delivered bytes do not match what was
reviewed, and the one a user most needs to be able to look up afterwards.

Phase 3 recorded the other two host-path silences and deliberately did not
touch this one, because it sits on the verification path rather than beside it.

- Give it the seatbelt one-liner (what / why / safe next step) and a recorded
  event, using the **same additive, identity-shaped, best-effort, never-gating
  pattern** P3.2 used — never a gating change, and never a path where an
  unrecordable refusal becomes an allow.
- **Its own review**, line by line, because the diff touches verification.
- The existing `seatbelt::refuse` seam already returns `()`, so the "legible
  denial is still a denial" invariant is inherited rather than re-argued.

## Open review findings — what two product reviews left standing

Both 2026-07-27 reviews are closed except the items below. Everything else they
raised is fixed and shipped in v0.16.0; `CHANGELOG.md` is the record of which.
These are kept here, short, because a finding that is neither fixed nor written
down is a finding that gets rediscovered by a stranger.

Ordered by what evidence they are waiting on, not by severity:

- [x] **F01 — one release truth is public.** v0.16.0 is GitHub's latest
  release with provenance-verified archives, checksums, the corrected formula,
  and the public Homebrew tap; installer and `brew install` verified 2026-07-28.
- [ ] **C2 / F04 — the activation study.** Five developers with 2+ supported
  CLIs, observed without command coaching; pass is 4/5 unaided and a median
  under five minutes. Detail in §1.6 and the Stage 1 gate below. Both reviews
  concluded this is now cheaper *after* a public release than before it — the
  gate had been blocking indefinitely, and real installs are where the five
  come from. It is also the only planned activity that could falsify the
  product thesis rather than confirm it. **Study-watch CLOSED (2026-07-31,
  v0.17.1):** the project-scope gap this item was watching for was confirmed
  deterministically by the §8.1 isolated pilot before any participant met it —
  and it failed harder than "import misses some servers": `status` reported no
  CLIs, `init` wrote an empty manifest, and `doctor` then reported 0 errors
  over it. The "fix after the study confirms" condition was therefore met, and
  it is fixed and published in v0.17.1. Participants install the latest
  release, so the study now tests the fixed journey; §8.1 keeps the original
  transcript as the before-state.
- [ ] **M1 / F08 — extract the authority data path.** 82% of the workspace's
  Rust lives in `crates/cli`, so `grant.rs` (authority construction) sits in
  the same crate as `lib.rs` (library management) with no compiler-enforced
  boundary. The contract comes first — the existing item under "Engineering
  foundation track" below is the live one. (Sizing note, 2026-07-29:
  `grant.rs` is ~83% tests; the production authority code is roughly 500
  lines, so the move is smaller than the raw line count suggests.)
- [ ] **F09 — one versioned docs system.** Pages now carry a banner naming the
  build they describe, which was the load-bearing half. Full per-release
  versioning, global search, and one content source remain.
- [x] **F15 — browser and accessibility checks at release grade** (2026-07-28:
  phone + desktop widths, landmarks, keyboard skip, themes, reduced motion,
  axe WCAG A/AA; smoke self-test and full local run green).
- [x] **F18 — migration recipes** (2026-07-28: one generated page covering
  Claude + Codex, Cursor + Gemini, dotfiles, teams without shared secrets, and
  complete removal; link/sitemap/a11y checks green).
- [ ] **F20 — the settings kind is unpinned.** `[settings.*]` values merge
  into native configs with no lock pin, no doctor probe, no witness, and — until
  this session — no ENFORCEMENT statement. Surfaced by the P0.3 structural lint
  and honestly baselined there (`kind:settings:lock` / `:doctor` / `:witness`
  remain in `tools/check-structure-baseline.txt`; the `:enforcement` gap is now
  closed by ENFORCEMENT.md's "Settings" section, which states the gap in the
  open). Fixing it is a behavior change, not documentation: pin settings values
  in the lock, re-gate on change, add a doctor probe and a witness test.
  Schedule it as its own supervised item — not inside any P0 or P1 lane.
- [ ] **F19 — a privacy-respecting learning loop.** Local-only by default, with
  an inspectable export of outcomes — never paths, commands, or content.
  Interviews and diary studies before any telemetry.
- [x] **M12 — closed-item narratives belong in `CHANGELOG.md`** (2026-07-29:
  executed — every checked item in this file is now one line plus a date or
  commit ref; the narratives live in `CHANGELOG.md` and commit history. Stale
  "uncommitted" annotations were corrected against git: the scaling lane,
  Lanes C1/C2, and the F13/F14 fixes all shipped in v0.16.0.)
- [x] **F13 — governance trigger met by the planned public launch** (2026-07-28:
  Code of Conduct, succession/funding posture, support routes published and
  linked).

Post-v0.17.0 re-review findings (2026-07-29, line-by-line review of the launch
sprint). None blocks the study; queue them as the first post-study items:

- [ ] **R1 — `edit-profile` skips the live-session blocker.** Rename/delete
  refuse when a session fences the toolset (`profile_blockers`); the batch
  edit (`a04a0bf`) does not — it changes a fenced running agent's reachable
  surface mid-flight, unflagged. Add the blocker or surface it in the preview.
- [ ] **R2 — doctor and diff read different server maps.** `diff` resolves the
  central library; `doctor`'s narrowed expected render is inline-only — so a
  names-only library-backed manifest (the recommended pattern) gets perpetual
  "changes pending" with a fix cue that cannot fix it. Make doctor
  library-aware like diff.
- [ ] **R3 — one notion of "active", cleared when it should be.**
  `restore` and `delete-profile` leave `state.json`'s `active_profile` behind
  (a deleted name silently resurrects on re-create), and `toolset list`
  derives "active" from the session instead — three notions of active. Clear
  on restore/delete, unify the readers, and add witnesses; `882f6e1`'s
  behavior currently has none.

## Stage 0 — close confirmed correctness gaps

Closed 2026-07-23. Detail in `CHANGELOG.md` (v0.16.0) and the commits below.

### Workflow module boundary

- [x] Boa `IdleModuleLoader` landed and independently reviewed; refuses every
  import (`b05fd26`, witness `dynamic_import_of_real_on_disk_module_is_refused`).
- [x] Ambient-capability review of context construction, two independent
  reviewers (2026-07-23): timezone pinned to UTC, `WeakRef`/
  `FinalizationRegistry` poisoned; dynamic-compilation denial and runtime
  limits verified sound.
- [x] §9.3 script-boundary review discharged with zero blocking findings
  (2026-07-23); the `(preview)` label dropped — `workflow` is a visible
  command. Its open follow-ups live under "Experimental workflows" below.

### Consent snapshot and UI authorization

- [x] Immutable `ConsentSnapshot` with focused trust/CLI witnesses (`e1c8000`).
- [x] Independent line-by-line review of the consent/grant path; nine findings
  closed same day; consent digest v3 re-gates every existing entry
  (2026-07-23).
- [x] t3code half of the contract: digest-bound grants, the `agentstack:admin`
  authorization boundary, and version-mismatch fail-closed behavior
  (CLI envelope `717f29d`; t3code `f0196e536`, `d98b5080d`).

### Stage 0 gate

- [x] Closed 2026-07-23: security diffs reviewed line-by-line, focused suites
  green, the t3code trust flow proven end-to-end against the real binary, and
  no write guarantee enforced only by the frontend.

## Stage 1 — first value in under five minutes

### 1.1 Positioning reset

- [x] Complete (2026-07-23): strategy, README, website, and the public docs
  lead with cross-CLI portability; deep security material moved to where it
  becomes relevant; `ENFORCEMENT.md`/`ARCHITECTURE.md` remain the untouched
  authoritative layer.

### 1.2 One recommended onboarding journey

The default journey is:

```text
install → init → review import → apply → doctor
```

t3code presents this as a guided graphical flow; the terminal presents the
same sequence directly. Both must call the same CLI-owned planning, validation,
write, and status paths.

- [x] Complete (2026-07-23, `e1c8000` + same-day batch): init audited from a
  clean machine; `init --plan` is the stable read-only JSON contract with a
  no-write witness; detection evidence, the pre-write import review, lossy
  imports in plain language, visible destinations, and one success summary all
  ship on both surfaces; a failed target no longer hides successful ones; one
  `restore --last` returns the onboarding write set byte-for-byte.

### 1.3 t3code launch experience

- [x] Complete (2026-07-23/24): integration copy replaced with the shipped
  contract; capability negotiation (`717f29d` + t3code `d98b5080d`); setup RPC
  backed by `init --plan` with the four-group setup card; fixed closed actions
  only (consent-bound `setup-apply`, `doctor --json` status, id-addressed
  `restore-write`); server-side workspace identity; one recommended next
  action; no advanced nouns in setup; parity tests prove panel and CLI produce
  byte-identical files (`crates/cli/tests/t3code_parity.rs`); Lane C1
  workflow-observe contract shipped in v0.16.0 (`7b9e101`).

### 1.4 Progressive-disclosure acceptance

- [x] Complete except the user test (2026-07-23 → 2026-07-28): Setup/Toolset/
  Status/Undo are the only beginner concepts; first-run UI speaks outcome
  language; the ordinary import/apply journey is witnessed vocabulary-free
  (`ordinary_journey_vocab`); trust review shows the exact content surface;
  every surfaced denial renders what/boundary/protected/next/details; stronger
  modes sit behind "More protection" with honest cost/coverage labels; one
  canonical ladder (Unify · Switch · Diagnose · Recover · Share · Govern)
  across README, landing, guide, and tutorial; `toolset list` gives the taught
  noun a read; the released v0.16.0 serves the whole t3code panel.
- [x] The recovery pair `restore`/`adopt` joined the default `--help` — Undo
  is a beginner concept, so the way back is findable without `--help --all`
  (`8f96af3`, 2026-07-29).
- [ ] Test the first-run copy with users who have not read the security docs.

### 1.5 First-value proof

- [x] Complete (2026-07-23/28): self-asserting sandboxed demo
  (`examples/first-value-demo/run-demo.sh`); the recording embedded in README,
  landing, and start guide is the current binary's own output as an animated
  SVG; any output-shape drift fails the demo script nonzero, so CI keeps it
  honest.

### 1.6 Activation study

Study kit (recruiting, protocol, metrics, gate mapping):
[`docs/design/activation-study.md`](docs/design/activation-study.md). A
maintainer dry-run of the full journey (2026-07-29, sandboxed HOME, release
build) found and fixed four pre-study blockers — active-toolset drift
awareness in doctor/diff, positional `toolset create <name>`, the
no-such-toolset error and lock-summary jargon, and a stale undo hint — and
recorded 2.1s total tool time for the whole journey.

- [ ] Recruit five developers who use at least two supported agent CLIs and did
  not build AgentStack.
- [ ] Observe them without guiding individual commands.
- [ ] Record:
  - install success;
  - time to understand the product;
  - time to first manifest;
  - time to first successful apply;
  - time to clean doctor;
  - confusing terms and abandoned steps.
- [ ] Fix the three most common blockers before adding features.

### Stage 1 gate

- [ ] Four of five users finish without maintainer intervention.
- [ ] Median install-to-clean-doctor time is below five minutes.
- [ ] At least four describe the product as one setup across their coding CLIs.
- [ ] No participant needs Docker, policy authoring, gateway setup, or workflow
  concepts to receive first value.
- [ ] At least four participants understand every block they encounter and can
  choose the safe next action without maintainer explanation.

## Stage 2 — profiles and reversible sessions

### 2.1 Stabilize the profile contract

- [x] Complete (2026-07-23, `e1c8000` + follow-ups): `use --list --json` is the
  machine-readable inventory (name, harness, selection, readiness, trust,
  active state) with one actionable reason per blocked row; the second-toolset
  journey is documented (`docs/howto/name-a-toolset.md`).

### 2.2 Make temporary switching dependable

- [x] Complete (2026-07-23): fail-closed session-start gate (`e1c8000`);
  start/end reports name exactly what they activate and restore; the active
  session is visible in bare `agentstack`; abandoned sessions (12h) are
  defined once and flagged by every surface with the safe recovery command;
  overlapping projects and interrupted processes witnessed
  (`session_overlap.rs`).

### 2.3 Present profiles through user tasks

- [x] Complete (2026-07-23, `docs/howto/name-a-toolset.md`): two worked
  examples; a toolset is "a named subset of the setup you already have — not a
  policy, a permission level, or a workflow role"; sessions are the
  recommended way to switch, static apply the stable/offline path.

### 2.4 t3code toolset picker

- [x] Complete (2026-07-23): read-only toolsets RPC plus the two session verbs,
  name-bound with pre-spawn shape refusals and the CLI's fail-closed gate as
  the enforcement; "Toolsets" / "Use temporarily" language; readiness with one
  reason per blocked row; no create/edit surface in this slice; panel-close
  recovery witnessed. The browser-level walkthrough of a reopened panel
  remains part of the Stage 2 gate's user scenario.

### Stage 2 gate

- [ ] Three users successfully use two different profiles.
- [ ] They start and end sessions without manually editing native files.
- [ ] At least two users return and use profiles in a later session.
- [ ] Interrupted-session recovery works in a user-facing scenario.

## Stage 3 — lifecycle confidence

### 3.1 Connect diagnosis to action

- [x] Complete (2026-07-23): ~110 actionable doctor findings inventoried;
  every finding carries one concrete recommended action (diff / adopt / apply /
  restore / re-lock / re-trust) via `.with_fix()`, feeding doctor's `↳` line
  and its single `start with:` triage; informational restatements dropped;
  blockers stay visually separate from information.

### 3.2 Make writes predictable

- [ ] Standardize dry-run and diff summaries across apply, adopt, init, session,
  and restore.
- [ ] Always distinguish managed, foreign, and hand-edited entries.
- [ ] State whether a write is project-local, user-global, or machine-global.
- [ ] Show the undo path before a material write.
- [ ] Preserve foreign entries unless the user explicitly selects a reviewed
  pruning operation.
- [ ] Diagnose foreign writes into managed instruction files. TanStack Intent's
  `intent install` writes skill guidance into `CLAUDE.md` / `AGENTS.md` /
  `.cursorrules` — the same files whose managed regions we own. A user running
  both tools produces drift that doctor should name as foreign content with the
  adopt-or-reapply choice, not silently reconcile.

### 3.3 Adapter reliability

- [ ] Rank adapters by observed user demand rather than treating all thirteen as
  equally important.
- [ ] Create shared conformance fixtures for the top adapters:
  - import;
  - render;
  - idempotent reapply;
  - hand-edit drift;
  - adopt;
  - restore;
  - secret placeholder behavior.
- [ ] Label lossy adapter fields in import/diff output.
- [ ] Publish the tested behavior matrix.

### 3.4 Recovery scenarios

- [ ] Exercise five end-to-end scenarios:
  - accidental manifest edit;
  - intentional native hand edit;
  - foreign server written by another tool;
  - interrupted temporary session;
  - failed multi-target apply.
- [ ] Ensure each scenario produces a correct diagnosis and safe recovery path.

### Stage 3 gate

- [ ] Five lifecycle scenarios pass without inspecting internal state files.
- [ ] Five users can choose correctly between adopt, apply, and restore from the
  command output alone.
- [ ] Top adapters pass the published lifecycle matrix.

## Stage 4 — sharing and reuse evidence

### 4.1 Team handoff

- [ ] Write a minimal teammate journey:
  clone → inspect → provide local secret values → apply/select profile → doctor.
- [ ] Prove the same manifest and lockfile on two machines.
- [ ] Verify no secret value enters committed files or diagnostic output.
- [ ] Make platform-specific differences visible and actionable.

### 4.2 Library/package reuse

- [ ] Select one real server package and one real skill package used by the
  maintainer.
- [ ] Reuse each across two projects without copying definitions.
- [ ] Measure whether source, lock, trust, and update behavior is understandable.
- [ ] Simplify library terminology or commands based on that exercise.
- [ ] Do not build a public catalog until local reuse succeeds repeatedly.

### 4.3 External skill sources — TanStack Intent importer

TanStack Intent (announced 2026-03) is a CLI for library maintainers to ship
Agent Skills inside npm packages; consumers run `intent install`, which
discovers intent-enabled packages in `node_modules` and writes their skills
into agent config files. It is supply-side to our demand-side: it produces
versioned skills, we decide what a project is allowed to have. That makes it a
capability *source* for the library, not a competitor — but only if the skills
arrive through the manifest, pinned, rather than as text splatted into files we
manage (see the drift item in §3.2, which is the part worth doing regardless).

Evidence gate before building: the maintainer, or a study participant, must
actually depend on an intent-enabled package and want its skills. Do not build
this on the strength of the ecosystem existing.

- [ ] Confirm the trigger: at least one real project whose dependencies ship
  intent skills the user wants in a toolset.
- [ ] Import intent-enabled package skills into the central library as ordinary
  pinned entries — content digest, source recorded, re-gated on byte change.
  Reuse the existing import/adopt seam; do not add a second capability path.
- [ ] Treat package-shipped skill content as hostile input (rule 7): bound it,
  parse defensively, and require the trust gate before it can enter agent
  context.
- [ ] Decide how a package upgrade surfaces — a changed skill is a pin change,
  so it must re-gate rather than update silently.
- [ ] Do not adopt their authoring loop (`scaffold` / `validate` / `stale`).
  That is a maintainer-side workflow for people publishing libraries, which is
  not our user.

### 4.4 Team discovery

- [ ] Complete three independent project handoffs.
- [ ] Interview participants about repeated coordination pain.
- [ ] Determine whether the next need is:
  - signed sources;
  - organization policy distribution;
  - hosted profile/package coordination;
  - evidence export;
  - none of the above.

### Stage 4 gate

- [ ] Three project handoffs succeed without credential sharing.
- [ ] One reusable package is used in at least two projects.
- [ ] A repeated team problem—not architectural possibility—selects the next
  expansion.

## Engineering foundation track

This track supports the product stages. It does not authorize unrelated feature
work.

### Extract the authority data path

- [ ] Write a short extraction contract covering:
  `AuthorityGrant → ExecutionPlan → Gateway::try_call → secret resolution /
  upstream transport`.
- [ ] Identify the existing single constructors and dispatch points that must
  remain unique.
- [ ] Move existing code; do not reimplement it.
- [ ] Keep `CompiledRuleset` and `GrantHandoff` as explicit boundary types.
- [ ] Add `#![forbid(unsafe_code)]` to every extracted crate from its first
  commit.
- [ ] Keep the narrowing, trust, pin, secret, and gateway witnesses green.
- [ ] Add a structural check or review rule preventing a second upstream
  transport path.
- [ ] Stop when the CLI is an orchestration caller of the kernel; do not extract
  unrelated library, formatting, or command code merely to improve
  line-count statistics.

### Maintainability

- [ ] Split oversized command modules only when a stable domain seam exists.
- [ ] Keep product terminology consistent across CLI output, docs, JSON, and UI.
- [ ] Generate or verify command reference data where practical to reduce drift.
- [ ] Keep closed work in `CHANGELOG.md` or commit history, not new roadmap or
  memory documents.

### Security and enforcement maintenance

- [ ] Preserve the policy-narrowing property tests.
- [ ] Preserve byte-change trust witnesses.
- [ ] Preserve the single gateway dispatch seam.
- [ ] Keep the enforcement matrix synchronized with shipped behavior.
- [ ] Give the gateway, relay, external harness launch, and workflow interpreter
  comparable adversarial review.
- [ ] Propose new dependencies before adding them.

## Experimental workflows

Workflows remain available for supervised testing but are not part of the
beginner promise.

**Reviewable workflows (Lane C2 v1)** — shipped in v0.16.0 (`7b9e101`,
adversarially reviewed to zero blocking findings): a model proposes a workflow
as an `agentstack-blueprint` JSON blueprint, t3code's chat renderer draws the
shape as a graph, and the user approves / rejects / edits-with-the-model
before it runs. The engine is untouched: the `propose-workflow` skill compiles
on approve and runs through the existing `agentstack workflow run`. Both v1
caveats named at landing are now closed — the approved blueprint is
digest-bound to the executed script (F13, `405ef30`) and declaring is one
recorded, undoable transaction (F14, `dd3e595`). Deferred fast-follows:
native declarative per-node execution, direct-manipulation editing, and the
`workflow propose --json` contract migration. History:
`docs/design/launch-plan.md` (closed record).

Before promoting them out of experimental:

- [x] Complete the module-loader fix and independent script-boundary review
  (§9.3 discharged 2026-07-23, zero blocking findings — recommended
  follow-ups: a cross-model codex pass on quota refill and coverage findings
  A/B/C in the design doc's §9 gate 3).
- [x] Cross-model codex review pass (2026-07-29, gpt-5.6 Sol, static, at
  `6beac55`): isolation, bridge topology, and shell/path hygiene confirmed;
  verdict "reasonable to keep shipping as labeled experimental". Six
  promotion-blocking findings recorded — two sharpen limitations the code
  already documents (watchdog does stderr/fs I/O before its hard exit and can
  block there; no JS heap cap), and it found three new ones, folded into the
  open items below.
- [ ] Close the codex findings before promotion: (a) result serialization
  re-enters the bridge after the final pending-request drain — a getter on the
  root result can enqueue an agent request that is returned as `Done` with no
  `StepSpawned` evidence; (b) `Atomics.isLockFree`/`waitAsync` remain
  reachable and are host-dependent, breaking cross-host resume determinism;
  (c) the 1 MiB script cap lives only in the CLI caller — enforce it at the
  crate boundary; (d) give the watchdog a no-I/O hard-exit path; (e) no total
  instruction budget for native built-ins (regex/string work runs until the
  process watchdog). Full report retained by the maintainer.
- [x] Review F13 — the approved blueprint is bound to the executed bytes
  (`405ef30`, v0.16.0).
- [x] Review F14 — compile-on-approve is one recorded, undoable transaction
  through the restore ledger (`dd3e595`, v0.16.0).
- [ ] Review heap-growth and hostile string/regex behavior. (Phase 2b below is
  the piece that actually caps the JS heap the posture label admits is
  uncapped.)
- [ ] Preserve the out-of-thread watchdog and honest posture label.
- [ ] Run at least three recurring tasks on separate occasions.
  (1 of 3 done — 2026-07-23: workflow-acceptance map→reduce→verify, real
  `claude` ×5, PASS, 22.0s, evidence green; see design doc §9 gate 4.
  Occasions 2 and 3 must be separate sittings.)
- [ ] Confirm each task is easier to repeat than the equivalent native/manual
  orchestration.
  (Occasion 1 evidence: 22.0s governed vs 86.4s hand-wired courier path it
  replaces, at the 22.6s ungoverned bookend — governed for ~free.)
- [ ] Confirm roles never widen their selected profile or machine ceiling.
- [ ] Decide whether library distribution is necessary from demonstrated reuse.

### Workflow scaling lane

Design: [`docs/design/workflow-scaling.md`](docs/design/workflow-scaling.md).
MapReduce-informed, but only where the analogy holds: every framework freedom
Hadoop enjoys (retry, reorder, relocate, race) is paid for by task purity, so
the lane buys purity back where a profile can *prove* it, not where an author
claims it. Post-launch: the 2026-07-25 product review puts the everyday loop
ahead of this, and that stands. Phases 0–3 and 5 shipped in v0.16.0
(`ac76fc0`, `49c8f9c`).

- [x] Phase 0 — measurement rig (`examples/workflow-scale/`): deterministic
  replayable mock harness + `analyze.py`. Key finding that set the phase
  order: the batch barrier and the concurrency cap are coupled (efficiency
  0.885 at conc 4 → 0.547 at conc 16).
- [x] Phase 1 — continuous dispatch: persistent worker pool,
  `StepOutcome::Awaiting`; width 100 / conc 16: 15.36s → 12.00s, 0.902 at a
  flat latency distribution — the residual loss is the straggler tail.
  Spawn-evidence-before-launch, lockstep resume replay, park/swap exclusivity,
  and wall-check semantics preserved, with a mutation-verified overlap witness.
- [x] Phase 1b — the serial cliff is visible in `workflow list` (`*` marker +
  `serial_roles` in JSON; contract advertised in `49c8f9c`).
- [x] Phase 2a — schema-validated results: `agent(prompt, {schema})` with a
  bounded JSON-Schema subset and no new dependency. No automatic re-ask — a
  CLI retry would spend an agent slot the engine's ceiling never granted. The
  replay path runs the same transform (witnessed).
- [ ] Phase 2b — content-addressed artifact store
  (`~/.agentstack/artifacts/<sha256>`) plus a resident-result byte cap, past
  which `agent()` returns a frozen opaque `{digest, bytes, preview}` handle.
  This is what actually bounds the JS heap the posture label admits is
  uncapped.
- [x] Phase 3 — `shard()` / `partition()` in the prelude and
  `agentstack workflow explain <name>`: `partition` returns exactly `r`
  buckets with FNV-1a placement so replays reproduce the split; `explain`
  runs the same admission choke point as `run` and reports call *sites*,
  saying so explicitly.
- [~] Phase 4 — purity surface landed and **failing closed**; execution half
  BLOCKED. `[workflows.<n>.scheduling.<role>]` parses `effect_free` / `retry` /
  `speculative`, and validation refuses all three with the prerequisite named.
  The plan's premise was wrong: a `Profile` fences servers/skills/harness
  only, `[policy.filesystem]` is bundle-global and enforced only in sandbox
  mode, and workflow children run at host tier — so nothing today can verify
  effect-freedom. Deriving it would violate rule 8; accepting the author's
  claim would defeat the thesis. Unblocked by either per-profile
  filesystem/egress dimensions, or deriving purity from a sandbox/lockdown
  posture for the role's children.
- [x] Phase 5 — the `Dispatcher` seam with a local-only implementation:
  `TaskDescriptor` carries digests and names, never argv/policy/secrets/paths
  — the absence is the contract and has its own witness; the suite passes
  unchanged and the bench is unmoved.
  - [ ] Still open from Phase 5: cancellation propagation (the watchdog
    orphans in-flight children) and splitting report/list/runs out of
    `workflow.rs` (review F16).
- [ ] Phase 6 — distributed workers. Trigger NOT met as of 2026-07-26, so
  nothing was built: the binding constraint at width 100 is the straggler
  tail, whose fix is Phase 4's backup tasks — blocked on the purity
  prerequisite, not on a shortage of machines.

### t3code MCP harness bridge — research only

t3code already exposes an MCP surface and may be able to launch or supervise
other coding harnesses for a workflow. This could remove duplicated
per-harness process plumbing and make multi-agent workflows visible in the
primary UI. It is not an authorization mechanism and must not become a second
spawn path.

- [x] Inventory the actual t3code MCP tools, authentication, lifecycle,
  cancellation, result, and compatibility behavior
  (`docs/design/t3code-mcp-bridge-research.md`, 2026-07-23). Decision: the
  bridge is NOT buildable on today's surface; the items below stay open
  pending the upstream changes named in that document.
- [ ] Map every proposed MCP operation to the existing workflow child-run
  contract: strict lock, trust, machine policy, frozen `ExecutionPlan`,
  `AuthorityGrant`, scoped MCP configuration, and recorded outcome.
- [ ] Define an optional child-launch backend that accepts only an already
  admitted frozen plan or a narrow launch reference. It must not accept
  arbitrary argv, workspace paths, policy, secrets, or authority from t3code.
- [ ] Add capability negotiation and fail closed when the t3code MCP is absent,
  incompatible, or returns an unrecognized child identity.
- [ ] Prototype one workflow that launches two different harnesses through the
  backend and compare it with direct CLI launch for complexity, portability,
  cancellation, and evidence quality.
- [ ] Add witnesses proving the backend cannot bypass the single child-launch
  dispatch, widen a grant, omit evidence, or leave a child running after
  cancellation.
- [ ] Keep direct CLI child launch as the baseline and fallback. Promote the
  t3code backend only if repeated use shows less integration work without
  weaker authority or evidence.

Deferred until these conditions are met:

- Visual workflow authoring.
- Approval/pause controls.
- Scheduling and durable jobs.
- Cloud workflow execution.
- A generic workflow marketplace.

## Evidence-gated future ideas

The following are deliberately removed from the active roadmap:

- Cloudflare runner.
- Hosted multi-tenant control plane.
- Enterprise assurance program.
- Public registry or marketplace.
- Background jobs and schedules.
- Additional capability categories.
- Separate component repositories.

An idea returns only when:

1. At least three users report the same repeated problem.
2. The smallest useful outcome is defined.
3. Existing features cannot solve it more simply.
4. Success and exit criteria are measurable.
5. It does not displace an unfinished earlier-stage gate.

## Completion definition

The current product strategy is validated when:

- New users reach a clean cross-CLI setup in under five minutes.
- Profiles create repeated use.
- Doctor/diff/adopt/apply/restore provide understandable lifecycle confidence.
- Projects can be handed to another person without sharing secrets.
- Security remains a trusted foundation without being the only visible reason
  to adopt AgentStack.
