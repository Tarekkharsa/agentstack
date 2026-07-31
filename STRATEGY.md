# AgentStack product strategy (v2)

> **Status:** operative product strategy — adopted 2026-07-31 by maintainer
> decision, superseding v1 (archived at
> [`docs/archive/STRATEGY-2026-07-v1.md`](docs/archive/STRATEGY-2026-07-v1.md));
> revised same day after independent adversarial review.
>
> **Current as of:** AgentStack 0.17.x
>
> **Relationship to the other documents:** [`TODO.md`](TODO.md) remains the
> only ordered work queue. *New product-capability work* enters it only through
> the phase gates below; the engineering-foundation track, open review
> findings, and the existing stage gates already in `TODO.md` continue on
> their own terms. [`CLAUDE.md`](CLAUDE.md) carries the binding
> product-experience and engineering rules. The archived v1 is a historical
> record; do not treat it as a second roadmap.
>
> **Numbering note:** the Phases here (Phase 0–4, always written with their
> names) and `TODO.md`'s Stages 0–4 are **unrelated numberings**. "The
> activation study" below means `TODO.md` Stage 1 §1.6, designed in
> [`docs/design/activation-study.md`](docs/design/activation-study.md).
>
> **Origin:** the vercel/eve source-level teardown (2026-07-31, § Competitive
> watch below) and the maintainer's mandate that follows from it: eve's
> authoring surface is clearer than ours, and we intend to be better than eve
> *and* better than our previous selves.

## The goal

> **Any capability, from anywhere, live in every agent you run — seconds after
> one deliberate yes, and never any other way.**

Identity in one line: **feels like a filesystem, thinks like a vault.**

Every word is load-bearing:

- **Any capability** — the declared kinds (skills, servers, instructions,
  settings, hooks, extensions, packs) under one mental model. The *inert*
  kinds get the compressed path; the *executable* kinds (hooks, extensions)
  always keep the full ceremony — see "What never changes."
- **From anywhere** — a file the user dropped, a repo they cloned, a teammate's
  bundle, an external registry (including ungated ones like eve's). Origin
  stops mattering because review does.
- **Every agent you run** — the portability promise; the thing a single-runtime
  framework structurally cannot say.
- **Seconds** — eve's authoring bar, met.
- **One deliberate yes** — consent compressed from a multi-step workflow into a
  single glanceable, content-bound moment. Compressed, never removed.
- **Never any other way** — the invariant that makes the rest worth having.

## The design law

> **Automate everything except the yes.**

Pinning, locking, staging, rendering, drift repair, recovery: all of it is the
system's job, done silently and correctly. The manifest and lock become
**system-maintained** — written by the machine in the common path, read by
humans, reviewed in pull requests. The manifest **remains the source of
truth**: the drop directories below are intake staging areas, never a second
truth, and when staged content and the manifest disagree the answer is always
a staged adoption offer, never a silent write. The one thing never automated,
inferred, or defaulted is consent to new content.

This is the precise inversion of both prior states. eve deleted the yes to
delete the machinery. v1-era AgentStack made the user operate the machinery
around the yes. v2 keeps exactly the yes and deletes everything else the user
currently touches.

## The experience contract

The user's entire cognitive surface is four ideas plus one recurring moment:

| Idea | The question it answers |
|---|---|
| **Setup** | What do I have? |
| **Toolset** | What does this task use? |
| **Status** | Is it ready — and if not, what one thing fixes it? |
| **Undo** | How do I take it back? |
| *(the yes)* | *Do I accept this exact content doing these exact things here?* |

Manifest, lock, trust digest, adapter, gateway, policy ceiling: all still
exist, all doing the same jobs, none of them vocabulary a user needs before
value. Mechanism nouns live behind `--explain` and the architecture docs. The
canonical user-facing ladder — **Unify · Switch · Diagnose · Recover · Share ·
Govern** — stays the spine of README, landing, and tutorial; the six moments
below are the v2 experience targets layered onto it, not a replacement.

**Delivery ambition — dynamic (zero-files) as the eventual default** for
MCP-capable harnesses: gateway registered once per CLI, capabilities leased
per project and task, skills loaded on demand as brokered, policy-checked,
recorded calls. This is an *ambition with named preconditions*, not the
current default (shipped default is static, and stays so): it flips only when
open question #1 (where the yes lives in zero-files mode) is resolved, when
[`docs/ENFORCEMENT.md`](docs/ENFORCEMENT.md) gains an honest column for the
lease path, and when first-run friction is measured acceptable — evaluated at
the Phase 3 gate, flipped no earlier than Phase 4. Static render and
clean-at-rest sessions remain permanently for what MCP cannot inject.

The six defining moments, stated as outcomes:

1. **Write a skill:** drop the file in the project. A quiet prompt — *"New
   skill 'summarize' — use it everywhere?"* — one yes, and it is live in every
   installed CLI. The manifest and lock updated themselves.
2. **Clone a stranger's repo:** its capabilities arrive staged, never active.
   The review reads like a two-line PR: what it runs, what it reaches, what
   changed since anyone last said yes.
3. **Sit at a new machine:** one command and the whole environment
   materializes, correct for whatever harnesses exist there, secrets resolving
   from that machine's own vault.
4. **Something is blocked:** the seatbelt moment. One sentence — *"Blocked:
   'web-search' tried api.evil.example — not in this project's allowed
   hosts."* — evidence attached, undo one keystroke.
5. **Change anything material:** the undo for it is named in the preview,
   before the write runs — and it works.
6. **Share:** sharing is signing; receiving is reviewing. Ungated ecosystems
   become our intake, because nothing activates without the yes.

## What never changes

The floor under every phase below. None of this is negotiable on the way to
the north star; the entire plan is about relocating cost, not relaxing
guarantees.

- Untrusted repository content is inert. The funnel stages; it never activates.
- All repository content is hostile input: parsed defensively, bounded, never
  interpolated into shell commands (`CLAUDE.md` invariant 7). This is the
  governing invariant for the file-drop funnel and for external-registry
  intake.
- No new unsafe code (`CLAUDE.md` invariant 1).
- Consent is content-bound. A byte change re-gates — the *presentation* of
  re-gating improves; the fact of it does not.
- Policy only narrows. Machine ceiling always wins.
- Secrets never serialize. `${REF}` resolves in memory; unresolved fails closed.
- Single authority and dispatch paths, with witnesses.
- Claims match enforcement. Every new convenience states honestly what it does
  and does not enforce.
- **Executable kinds — native extensions and hooks — keep the full ceremony.**
  Both run code or commands in or around the harness process at user
  permission; neither ever gets a compressed review. The funnel below is for
  inert-content kinds only.
- Progressive disclosure must never become progressive enforcement.

## The gap: seven deltas between today and the north star

- **D1 — Authoring.** Today: edit manifest → lock → trust → apply. North star:
  drop a file → one yes. The manifest becomes system-maintained.
- **D2 — Consent shape.** Today: trust is a workflow the user operates, and a
  re-gate says "digest mismatch." North star: one glanceable review card, and
  a re-gate shows the three lines that changed.
- **D3 — Consent memory.** Today: trust is keyed to project path on one
  machine; identical content asks again everywhere. North star: recognition —
  *"seen and approved in two other projects"* — shortens the card, never skips
  it.
- **D4 — Surface uniformity.** Today: seven capability kinds with per-kind
  quirks. North star: the inert kinds (skills, instructions, settings, packs,
  and servers' declarations) present one shape, learned once,
  machine-enforced; hooks and extensions share the shape but keep their
  ceremony; workflows remain the separately-gated advanced lane.
- **D5 — Legibility of enforcement.** Today: doctor and the recorder hold the
  answers if you know to ask. North star: every denial and every material
  change explains itself in one sentence with its own undo.
- **D6 — Materialization.** Today: init/import per machine, rungs assembled by
  hand. North star: one command on a fresh machine.
- **D7 — Exchange.** Today: signing exists as a primitive. North star: sharing
  is signing, receiving is reviewing, and external ecosystems flow through the
  same staged intake.

## The plan

Five phases. Each has an outcome, concrete workstreams, an invariant check,
and an evidence gate to the next. Phases are sequential in ambition but not
strictly in time — a later phase's design doc may be written early, but its
build waits for its gate.

> **Amendment (2026-07-31, maintainer decision):** with zero external users,
> the activation study moves from *inter-phase gate* to *release gate*.
> Phases 1–4 build now, in order, on maintainer acceptance and the invariant
> checks below; the study runs once, against the completed v2 journey, and
> **v0.18.0 does not publish until it passes** (its pass condition and the
> three-blocker rule unchanged, per `docs/design/activation-study.md`). The
> trade is explicit: pre-build falsification is exchanged for post-build
> validation, accepted because the maintainer is currently the only
> stakeholder and bears the rework risk knowingly. Everything in "What never
> changes" is untouched by this amendment.

### Phase 0 — Instrument (now; runs alongside the activation study)

**Outcome:** we know which deltas are felt, not theoretical — and consent
becomes measurable.

Workstreams:

- Run the activation study as planned. Add observation prompts for the
  deltas: where does a tester reach for a "drop a file" mental model and
  stall? Which ceremony step draws the first sigh? Can they say, afterward,
  what they consented to?
- Define and baseline the north-star metrics — **measured only through
  opt-in studies until the F19 privacy-preserving measurement design is
  approved** (`TODO.md`):
  - **TTLC** — time from file-drop to capability live in two CLIs.
  - **Concepts-before-value** — mechanism nouns a new user must read before
    first success.
  - **Review comprehension** — can the user restate what a yes granted?
  - **Recovery time** — from "something is wrong" to restored.
- **Wire trust-store mutation recording** (already a named planned mitigation
  in `docs/ENFORCEMENT.md`): every grant, re-grant, and revocation becomes a
  recorded event. Later consent gates are counted over these events; without
  them, "no consent surprise" is unfalsifiable.
- Land the two no-behavior-change foundations from the teardown: the
  structural lint — every **policy dimension** keeps its `ENFORCEMENT.md` row,
  and every **capability kind** has manifest table + lock pinning + doctor
  probe + witness test + an explicit honest enforcement statement (on the
  model of the existing "Native extensions" treatment; kinds without a
  runtime cell say so plainly, they never get an invented row) — and the
  files-you-create-first docs restructure.

**Invariant check:** no product behavior changes in this phase (the recorder
wiring adds events, never gates).

**Gate to Phase 1:** ~~the activation study passes~~ — amended 2026-07-31:
Phase 0's build work is complete and verified (metrics defined, trust-mutation
recording live, structural lint in CI, files-first docs); Phase 1 may build.
The study becomes the v0.18.0 release gate — a study that falsifies the
thesis still stops the *release* and forces rework, it just does so after the
build instead of before it.

### Phase 1 — The funnel (authoring: D1)

**Outcome:** dropping a file into the project is a supported, first-class way
to author a capability — and, for content you demonstrably wrote yourself, it
ends in one confirmation.

Workstreams:

- **Intake detection:** content dropped into `.agentstack/skills/` or
  `.agentstack/instructions/` is noticed at the next command touchpoint
  (`doctor`, `use`, `lock`, panel open) and offered for adoption with a
  preview. No daemon; detection is command-time, which also keeps it honest.
- **Provenance before compression:** the single-action path applies only to
  content with a local-authorship signal — untracked in git, or created after
  the project's last trust grant. Content that *arrived with the clone* (or
  fails the signal) always takes the full staged-review path of moment 2.
  The two states are witnessed by tests: same directory, different provenance,
  different path.
- **Single-action activation, first-time adoption only:** for new local inert
  content with no policy change, one command — working name `agentstack yes`,
  or folded into `use --write` — performs lock refresh, trust re-bind, and
  render behind one combined preview. Internally it is still lock → trust →
  apply in that order; the collapse is presentation, not semantics.
  **Re-gates of changed content stay on the current explicit path until the
  Phase 2 diff card exists** — compressing re-consent before the diff is
  visible would be worse than today.
- **System-maintained manifest:** `add`/`adopt` paths write manifest entries
  via `toml_edit` (format-preserving); hand-editing remains supported but
  becomes the exception. Docs teach the drop-and-yes path first. The shipped
  catalog instruction (`crates/cli/catalog/instructions/agentstack/rules.md`)
  is updated in the same change so rendered agent guidance matches the new
  authoring model.
- **Library hand-off:** the adoption offer includes "save to library" so a
  dropped capability can land in the central library for cross-project reuse
  instead of (or as well as) this project's manifest. The library and packs
  remain named composition concepts, not an afterthought.
- **Scope guard:** servers still require declaration (they carry commands,
  env, and secrets — there is no file to "drop"); **hooks and extensions are
  excluded from the funnel entirely** (executable kinds, full ceremony).

**Invariant check:** staged content is provably inert before the yes (witness
test); the provenance split is witness-tested; all staged content is parsed
as hostile input per invariant 7; the combined preview shows everything the
separate steps showed.

**Gate to Phase 2:** (amended 2026-07-31) the funnel works end to end in the
maintainer's own use with zero consent-surprise incidents counted over the
recorded trust-mutation events, and the witnesses (inertness, provenance
split) are green. Tester evidence moves to the release gate: the study
exercises the funnel journey before v0.18.0 publishes.

### Phase 2 — One yes (consent: D2, D3)

**Outcome:** the yes becomes the product's signature moment — glanceable,
plain-language, diff-aware — and repeated review of identical content gets
cheaper without getting weaker.

Workstreams:

- **The review card:** redesign the trust prompt as two to five lines of plain
  words: exact commands it will run, hosts it will contact, secret references
  it may resolve, skill/instruction pin status — and, on re-gate, *what
  changed* since the last yes, rendered from lock deltas as a real diff
  ("this skill changed 3 lines") instead of "digest mismatch." CLI first;
  panel mirrors the same card via the existing trust-from-UI flow. Once the
  diff card ships, re-gates join the compressed path.
- **Consent recognition** — settled direction, detailed in a design doc
  before build: a content-addressed memory of past approvals that *pre-fills
  and shortens* the card ("this exact content is approved in two other
  projects on this machine") but does **not** auto-skip the per-project
  yes — preserving the context binding that path-keyed trust deliberately
  provides. A machine-level "always allow this exact content" opt-in for
  power users may follow; it is an explicit widening and gets its own review.
- **Undo in the preview:** every material write names its undo *in the
  preview, before it runs* — groundwork for D5, and the v1 "explain writes
  before performing them" rule carried forward intact.

**Invariant check:** the property that a byte change re-gates is untouched;
recognition never crosses machines; the review card never shows less than the
current prompt does (it shows it better).

**Gate to Phase 3:** review-comprehension metric improves against baseline;
zero instances, counted over recorded grant events, of a user saying yes to
something the card did not surface.

### Phase 3 — Four ideas (surface: D4, D5)

**Outcome:** the visible product is Setup, Toolset, Status, Undo, plus the
yes — and the seatbelt explains itself.

Workstreams:

- **Kind convergence:** retire per-kind authoring quirks so every inert
  capability kind presents the same shape (name, source, pin, what-it-asks).
  The Phase-0 structural lint is the enforcement; this phase is the
  migration. Hooks and extensions share the presentation shape but keep
  their ceremony; workflows are out of scope here.
- **Vocabulary completion:** first-contact surfaces (CLI help, `init`,
  `doctor`, panel) speak only the four ideas; mechanism nouns move behind
  `--explain` and the architecture docs. The six-rung ladder keeps naming
  the journey in README, landing, and tutorial. Finish the rename ladder the
  toolset/status work started.
- **Seatbelt legibility:** every enforcement denial (gateway tool block,
  egress refusal, secret-scope refusal, filesystem guard) produces one plain
  sentence — what was stopped, why, and the safe next step — backed by a
  recorder event the user can open. Honest baseline: today the recorder
  captures gateway and guard decisions; egress and secret-scope refusals on
  the host path are **not** recorded yet, and adding those events is part of
  this workstream, per-mode honesty maintained in `docs/ENFORCEMENT.md`.
- **Status as one next action:** `doctor` (status) always ends with exactly
  one recommended command, never a list of findings without a path.

**Invariant check:** renames never change semantics; a legible denial is still
a denial (no "explain then allow anyway" path).

**Gate to Phase 4:** concepts-before-value at or near four; testers describe a
blocked action correctly in their own words. The dynamic-default preconditions
(experience contract above) are evaluated here.

### Phase 4 — Anywhere (exchange: D6, D7)

**Outcome:** environments teleport and content flows in and out — governed.

Workstreams:

- **One-command materialization:** a single bootstrap command on a fresh
  machine (`install --locked` + secret-reference reconciliation prompts +
  doctor, composed) that ends with every installed harness configured and a
  one-line status. The pieces exist; this is composition and polish.
- **Sharing is signing:** fold detached signing into the share flow and
  signature verification into intake, with publisher-key UX that a human can
  operate without reading a design doc.
- **Governed intake from external ecosystems:** import capabilities from
  ungated registries through the same staged-review funnel — quarantine,
  review card, yes — with every fetched byte treated as hostile input
  (invariant 7). eve's ecosystem is Apache-2.0 and machine-readable supply:
  its `SKILL.md` skills are format-compatible with our library, its MCP
  "connections" map to our server definitions, and its registry JSON and
  integration catalog can be read by `add from` as intake sources.
  **Attribution capture is an explicit workstream:** the library/lock schema
  gains license + origin fields so upstream LICENSE/NOTICE obligations are
  carried mechanically, not by promise.
- **Trigger discipline:** this phase's external-intake build starts on
  *user demand* — a study participant or real user asks to import from an
  external registry. That trigger is deliberately distinct from
  competitive-watch tripwire 3 (eve's registry becoming de-facto
  distribution), which triggers a strategy revisit, not a build.

**Invariant check:** intake never becomes activation; signature verification
and local consent remain separate decisions (a valid signature shortens
review, never replaces the yes).

**Gate to "done":** the six defining moments above each demonstrable end to
end, on camera, by someone who is not the maintainer.

## Open design questions

Genuinely open — named now so no phase resolves them by accident. (Two
earlier questions — recognition depth and the server funnel story — had
settled directions and now live in Phase 2's and Phase 1's text.)

1. **Where does the yes live in zero-files mode?** Leases activate without
   files on disk; the review card must be equally strong in the MCP path.
   Blocks the dynamic-default flip until answered.
2. **Does `agentstack yes` exist as a command or a mode?** Naming and
   placement decided after Phase 1 prototyping, not before.
3. **Scoped-MITM credential brokering** (from the eve teardown) is adjacent
   but separate: it extends what the vault protects, not how the yes works.
   It keeps its own design-doc lane after the activation study and is not a
   phase of this plan.

## Competitive watch: vercel/eve

Flagged 2026-07-31 after a source-level review. eve (public beta) is Vercel's
filesystem-first framework for building durable agents. It is not a category
competitor today — it builds and hosts one new agent rather than managing the
environment of existing CLIs, it has no import path for existing setups, and it
has no content trust gate at all (its registry installs files and runs setup
commands behind a single y/n prompt). But it competes for the same developer's
mental model of where agent capabilities live, its skill format and on-demand
loading match ours, and Vercel's distribution reach means its conventions will
shape expectations.

Three things follow:

1. **The bar it sets is authoring clarity.** In eve, capability nouns are user
   goals (tools, skills, schedules) and adding a capability is dropping a file
   in a conventional location — no registration ceremony. AgentStack must match
   that ease on the happy path without weakening an invariant: the manifest
   stays the source of truth, but the number of concepts and steps a user
   touches before value must keep shrinking. "As easy as eve on the surface,
   fail-closed underneath" is the standard — and the whole of this document is
   the response.
2. **Its open ecosystem is an opportunity, not only a threat.** eve is
   Apache-2.0. Its skills, connector catalog, and registry format are
   machine-readable supply for our governed intake (Phase 4): anything can
   flow in, because nothing activates without the yes.
3. **Tripwires that upgrade eve to a direct competitor**, each warranting a
   strategy revisit (a revisit, not an automatic build): eve imports existing
   CLI setups (it already detects Claude Code, Codex, Cursor, and others as
   parent processes); eve renders or exports configuration to other
   harnesses; eve's registry becomes de-facto skill distribution and users
   ask us to consume it; or eve ships a content-bound trust gate.

## Carried forward from v1 (still binding)

The archived v1 records the full original rationale — value ladder, pillars,
positioning, UX strategy. Its still-binding decisions are restated here so no
one needs to read two documents for direction:

- **The promise:** *Define your agent setup once. Use it across every coding
  CLI.* Cross-vendor portability is the product; trust, policy, and evidence
  make it dependable.
- **Surfaces:** the CLI is the primary surface, the authority, and the launch
  channel. t3code is the optional graphical companion over the same fixed
  action contract — never a second authority, never a second UI to rebuild.
  **When t3code is publicly obtainable, revisit its role.**
- **Progressive disclosure:** outcome first, safe defaults silently, safety
  explained when relevant, blocked actions always name the safe next step,
  stronger modes behind "more protection." **Progressive disclosure must
  never become progressive enforcement.** (Also binding via `CLAUDE.md`.)
- **Composition concepts:** manifest, toolset, and **library package** — the
  library and packs are the reuse path across projects and the intended
  promotion target for generated capabilities; the funnel feeds them, it
  does not replace them.
- **Engineering strategy:** extend existing seams; never reimplement working
  trust, policy, gateway, runtime, recording, import, render, or restore
  paths; `trust` and `policy` stay small review boundaries; the M1
  authority-kernel extraction and the engineering-foundation track in
  `TODO.md` continue unchanged.
- **Evidence gating:** no new capability lanes without user evidence; the
  activation study gates expansion. **Workflows promote only through the
  checklist under "Experimental workflows" in `TODO.md`** (recurring-use
  evidence plus its open review findings) — "advanced lane" here never
  means that checklist is discharged.
- **Non-goals:** no agent-building framework or general coding agent, no
  hosted multi-tenant runner, no generic workflow engine, no
  Cloudflare-specific product, no public marketplace before local reuse
  earns one (a deliberate softening of v1's flat refusal, in line with its
  own "evidence-gated possibilities" note), no enterprise administration
  suite, no background-jobs platform, no separate repositories for
  components without independent adoption, no second embedded dashboard,
  and no new capability categories before the current ones are validated.
  Beating eve means winning the moments above — not acquiring their product
  shape.

## How this document is used

- Each phase's build work enters `TODO.md` through its gate, as ordinary
  gated lanes; this file is the map, never the queue.
- A visual companion —
  [`docs/design/strategy-v2-vision.html`](docs/design/strategy-v2-vision.html)
  — shows the end state as CLI mockups, panel wireframes, the full command
  surface, and the feature inventory. It illustrates; it never overrides.
  This document wins on any divergence, and command names shown there are
  working names pending their phases.
- Before any task starts, v1's admission questions still apply: does it serve
  the current gate; is the smallest useful outcome defined; could existing
  features solve it more simply; are success and exit criteria measurable;
  does it introduce a new concept or capability lane; does it displace an
  unfinished earlier gate?
- The metrics in Phase 0 are the scoreboard. If a phase ships and its metric
  does not move, the phase is not done.
- Revisit this document at each competitive-watch tripwire and after each
  user-evidence milestone; it should get sharper, not longer.
