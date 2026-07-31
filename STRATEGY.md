# AgentStack product strategy (v2)

> **Status:** operative product strategy — adopted 2026-07-31 by maintainer
> decision, superseding v1 (archived at
> [`docs/archive/STRATEGY-2026-07-v1.md`](docs/archive/STRATEGY-2026-07-v1.md))
>
> **Current as of:** AgentStack 0.17.x
>
> **Relationship to the other documents:** `TODO.md` remains the only ordered
> work queue — work enters it only through the phase gates below. `CLAUDE.md`
> carries the binding product-experience and engineering rules. The archived v1
> is a historical record; do not treat it as a second roadmap.
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

- **Any capability** — skill, server, instruction, hook: one mental model, not
  six kinds with separate quirks.
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
lockfile-like artifacts — written by the machine, read by humans, reviewed in
pull requests. The one thing never automated, inferred, or defaulted is consent
to new content.

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
value. Mechanism nouns live behind `--explain` and the architecture docs.

Delivery default: **dynamic (zero-files)** for MCP-capable harnesses — the
gateway registers once per CLI, capabilities lease in per project and task,
skills load on demand (index in context, body on load), and every load is a
brokered, policy-checked, recorded call. Static render and clean-at-rest
sessions remain for what MCP cannot inject (native instruction files,
extensions, offline starts).

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
5. **Change anything material:** the undo for it is offered in the same
   breath, and it works.
6. **Share:** sharing is signing; receiving is reviewing. Ungated ecosystems
   become our intake, because nothing activates without the yes.

## What never changes

The floor under every phase below. None of this is negotiable on the way to
the north star; the entire plan is about relocating cost, not relaxing
guarantees.

- Untrusted repository content is inert. The funnel stages; it never activates.
- Consent is content-bound. A byte change re-gates — the *presentation* of
  re-gating improves; the fact of it does not.
- Policy only narrows. Machine ceiling always wins.
- Secrets never serialize. `${REF}` resolves in memory; unresolved fails closed.
- Single authority and dispatch paths, with witnesses.
- Claims match enforcement. Every new convenience states honestly what it does
  and does not enforce.
- Native extensions (executable kind) keep the full ceremony. The funnel below
  is for inert-content kinds; code that runs in the harness process never gets
  a compressed review.

## The gap: seven deltas between today and the north star

- **D1 — Authoring.** Today: edit manifest → lock → trust → apply. North star:
  drop a file → one yes. The manifest becomes system-written.
- **D2 — Consent shape.** Today: trust is a workflow the user operates, and a
  re-gate says "digest mismatch." North star: one glanceable review card, and
  a re-gate shows the three lines that changed.
- **D3 — Consent memory.** Today: trust is keyed to project path on one
  machine; identical content asks again everywhere. North star: recognition —
  *"seen and approved in two other projects"* — shortens the card. Open design
  question below on how far this goes.
- **D4 — Surface uniformity.** Today: six capability kinds with per-kind
  quirks. North star: one shape, learned once, machine-enforced.
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

### Phase 0 — Instrument (now; runs alongside the §1.6 study)

**Outcome:** we know which deltas are felt, not theoretical.

Workstreams:

- Run the §1.6 study as planned. Add observation prompts for the deltas:
  where does a tester reach for a "drop a file" mental model and stall? Which
  ceremony step draws the first sigh? Can they say, afterward, what they
  consented to?
- Define and baseline the north-star metrics:
  - **TTLC** — time from file-drop to capability live in two CLIs.
  - **Concepts-before-value** — mechanism nouns a new user must read before
    first success.
  - **Review comprehension** — can the user restate what a yes granted?
  - **Recovery time** — from "something is wrong" to restored.
- Land the two zero-risk foundations from the teardown: the structural lint
  (every capability kind and policy dimension has the same complete shape:
  manifest table + lock pinning + doctor probe + `ENFORCEMENT.md` row +
  witness test) and the files-you-create-first docs restructure.

**Invariant check:** no product behavior changes in this phase.

**Gate to Phase 1:** study observations collected; metrics baselined.

### Phase 1 — The funnel (authoring: D1)

**Outcome:** dropping a file into the project is a supported, first-class way
to author a capability — and it ends in one confirmation, not four commands.

Workstreams:

- **Intake detection:** content dropped into `.agentstack/skills/` or
  `.agentstack/instructions/` is noticed at the next command touchpoint
  (`doctor`, `use`, `lock`, panel open) and offered for adoption with a
  preview. No daemon; detection is command-time, which also keeps it honest.
- **Single-action activation:** for the common case (new local inert content,
  no policy change), one command — working name `agentstack yes`, or folded
  into `use --write` — performs lock refresh, trust re-bind, and render behind
  one combined preview. Internally it is still lock → trust → apply in that
  order; the collapse is presentation, not semantics.
- **System-written manifest:** `add`/`adopt` paths write manifest entries via
  `toml_edit` (format-preserving); hand-editing remains supported but becomes
  the exception. Docs teach the drop-and-yes path first.
- **Scope guard:** servers still require declaration (they carry commands,
  env, and secrets — there is no file to "drop"); extensions are excluded from
  the funnel entirely.

**Invariant check:** staged content is provably inert before the yes (witness
test); the combined preview shows everything the separate steps showed.

**Gate to Phase 2:** funnel used successfully by testers or the maintainer's
own daily use for two weeks without a consent-surprise incident; TTLC drops
measurably against baseline.

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
  panel mirrors the same card via the existing trust-from-UI flow.
- **Consent recognition (design doc first, then build):** a content-addressed
  memory of past approvals. Decision to make deliberately: recognition
  *pre-fills and shortens* the card ("this exact content is approved in two
  other projects on this machine") but does **not** auto-skip the per-project
  yes — preserving the context binding that path-keyed trust deliberately
  provides. A machine-level "always allow this exact content" opt-in for
  power users may follow; it is an explicit widening and gets its own review.
- **Undo in the same breath:** every material write ends by naming its undo
  (`restore` invocation) — groundwork for D5.

**Invariant check:** the property that a byte change re-gates is untouched;
recognition never crosses machines; the review card never shows less than the
current prompt does (it shows it better).

**Gate to Phase 3:** review-comprehension metric improves against baseline;
zero instances of a user saying yes to something the card did not surface.

### Phase 3 — Four ideas (surface: D4, D5)

**Outcome:** the visible product is Setup, Toolset, Status, Undo, plus the
yes — and the seatbelt explains itself.

Workstreams:

- **Kind convergence:** retire per-kind authoring quirks so every inert
  capability kind presents the same shape (name, source, pin, what-it-asks).
  The Phase-0 structural lint is the enforcement; this phase is the migration.
- **Vocabulary completion:** first-contact surfaces (CLI help, `init`,
  `doctor`, panel) speak only the four ideas; mechanism nouns move behind
  `--explain` and the architecture docs. Finish the rename ladder the
  toolset/status work started.
- **Seatbelt legibility:** every enforcement denial (gateway tool block,
  egress refusal, secret-scope refusal, filesystem guard) produces one plain
  sentence — what was stopped, why, and the safe next step — backed by a
  recorder event the user can open. The recorder already captures the
  decision; this phase gives it a voice.
- **Status as one next action:** `doctor` (status) always ends with exactly
  one recommended command, never a list of findings without a path.

**Invariant check:** renames never change semantics; a legible denial is still
a denial (no "explain then allow anyway" path).

**Gate to Phase 4:** concepts-before-value at or near four; testers describe a
blocked action correctly in their own words.

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
  review card, yes. eve's ecosystem is Apache-2.0 and machine-readable
  supply: its `SKILL.md` skills are format-compatible with our library, its
  MCP "connections" map to our server definitions, and its registry JSON and
  integration catalog can be read by `add from` as intake sources (NOTICE
  preserved). Their open connectors become our supply — always through
  staged review, never activation.
- Explicitly evidence-gated per the competitive-watch tripwires: build the
  external intake only when users ask for it (tripwire 3), not because it is
  satisfying.

**Invariant check:** intake never becomes activation; signature verification
and local consent remain separate decisions (a valid signature shortens
review, never replaces the yes).

**Gate to "done":** the six defining moments above each demonstrable end to
end, on camera, by someone who is not the maintainer.

## Open design questions

Named now so no phase resolves them by accident:

1. **How far does consent recognition go?** Recommended answer: recognition
   shortens the card, never skips the yes; machine-level standing approval is
   a separate, explicit, reviewable widening. To be settled in the Phase 2
   design doc.
2. **Where does the yes live in zero-files mode?** Leases activate without
   files on disk; the review card must be equally strong in the MCP path.
3. **What is the funnel story for servers?** They have no droppable file.
   Likely answer: `add server` stays declarative but inherits the Phase 2
   card; wizard-grade prompts close the gap.
4. **Does `agentstack yes` exist as a command or a mode?** Naming and
   placement decided after Phase 1 prototyping, not before.
5. **Scoped-MITM credential brokering** (from the eve teardown) is adjacent
   but separate: it extends what the vault protects, not how the yes works.
   It keeps its own design-doc lane after §1.6 and is not a phase of this
   plan.

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
   strategy revisit: eve imports existing CLI setups (it already detects Claude
   Code, Codex, Cursor, and others as parent processes); eve renders or exports
   configuration to other harnesses; eve's registry becomes de-facto skill
   distribution and users ask us to consume it; or eve ships a content-bound
   trust gate.

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
- **Progressive disclosure:** outcome first, safe defaults silently, safety
  explained when relevant, blocked actions always name the safe next step,
  stronger modes behind "more protection." (Also binding via `CLAUDE.md`.)
- **Engineering strategy:** extend existing seams; never reimplement working
  trust, policy, gateway, runtime, recording, import, render, or restore
  paths; `trust` and `policy` stay small review boundaries.
- **Evidence gating:** no new capability lanes without user evidence; the
  §1.6 study gates expansion; workflows stay the advanced lane behind their
  own review.
- **Non-goals:** no agent-building framework or general coding agent, no
  hosted multi-tenant runner, no public marketplace before local reuse earns
  one, no enterprise administration suite, no background-jobs platform, no
  second embedded dashboard. Beating eve means winning the moments above —
  not acquiring their product shape.

## How this document is used

- Each phase's build work enters `TODO.md` through its gate, as ordinary
  gated lanes; this file is the map, never the queue.
- The metrics in Phase 0 are the scoreboard. If a phase ships and its metric
  does not move, the phase is not done.
- Revisit this document at each competitive-watch tripwire and after each
  user-evidence milestone; it should get sharper, not longer.
