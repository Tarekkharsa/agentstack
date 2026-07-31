# The consent card

> **Status:** Active — the contract for Strategy v2 Phase 2 (the review card).
> Direction lives in [`STRATEGY.md`](../../STRATEGY.md) "Phase 2 — One yes";
> the ordered work lives in [`TODO.md`](../../TODO.md). This document does not
> restate either. It fixes the three *contracts* Phase 2 introduces, because
> they share storage and would otherwise be designed three times.

Three pieces, one document:

- **(a) the card** — what the grant screen says, and the rule that bounds it;
- **(b) prior bytes** — how a re-gate renders "3 lines changed" instead of
  "digest mismatch";
- **(c) recognition** — how repeated review of identical content gets cheaper
  without getting weaker.

They share one property: **all three degrade to today's behaviour and never
gate.** A missing snapshot, a missing index, a corrupt record — each falls back
to the message the user gets today. None of them can block, widen, or
substitute for consent. This is stated once here and is binding on all three.

## The bounding rule

> **The card may never disclose less than today's preview.**

This is the one property in this document that gets a machine witness rather
than a promise, because it is the only one whose violation is silent. The
witness is specified in §(a) below. Everything else in Phase 2 is presentation
over a gate that does not change: the same `grant_gated` entry point, the same
digest computation, the same single grant path.

## (a) The card

### Shape

Today's review is one ~630-line function printing sections top-to-bottom
(`crates/cli/src/commands/trust.rs`, `grant_gated`). It is complete and it is
honest; it is not glanceable. The card keeps every fact and changes the
presentation: **two to five plain lines per item**, answering in the user's
words —

- what it **runs** (commands),
- what it **contacts** (hosts/URLs),
- what it **may read** (secret references),
- whether the bytes are **pinned**,
- and on a re-gate, **what changed** (§b).

The machine-ceiling line stays, unchanged and unconditional. Section framing,
blocker summaries, and the post-grant confirmation stay. Mechanism nouns the
moment does not need stay out, per `CLAUDE.md`'s vocabulary rules.

### The seam

`ConsentCard { lines, question, answer }` already exists
(`commands/trust.rs`), built by the Phase 1 funnel (`commands/yes.rs`) with an
empty `lines` vector and spliced into `grant_gated` verbatim. That is the
insertion point: the card is composed *before* the gate and rendered *by* it.
No new grant constructor, no second path — invariant 6 holds by construction.

### The witness — and why the obvious one is wrong

The tempting witness is "enumerate every fact today's preview prints, assert
each appears on the card." It must not be built that way, for a reason found
while verifying this design:

> **Today's preview discloses nothing about `[hooks.*]` or `[settings.*]`,**
> which are declared capability kinds on the `Manifest` struct. Editing a hook
> changes the manifest bytes, so the *digest* re-gates and
> [`ENFORCEMENT.md`](../ENFORCEMENT.md) correctly says so — but the human is
> re-asked without being shown the hook. Hooks are an executable kind carrying
> the full ceremony; this is the exact shape of a consent surprise, and Phase
> 2's gate counts those.

Anchoring coverage to "what today's screen shows" would certify that omission
forever. The invariant is therefore stated one level up:

> **Every kind the manifest can declare has a disclosure site on the card, or
> an explicit baselined line saying it does not.**

Three legs, in dependency order:

1. **Item coverage (self-updating).** Every reviewed item already passes
   through `ReviewDiff::mark`, which persists `SurfaceItem { kind, name,
   identity }` into the trust store, readable back via `trust::prior_surface`.
   A test spawns the real binary over a maximal fixture, captures the card, and
   asserts every recorded item's name appears. Compare *sanitized to
   sanitized* — `mark` receives raw names while lines print through
   `text::sanitize_line`. This leg auto-covers any future kind that calls
   `mark`, and by construction cannot see a kind that does not.
2. **Kind coverage (catches what leg 1 cannot).** A `:review` requirement in
   `tools/check-structure.py` beside the existing `:lock`/`:doctor`/`:witness`
   checks: every `EXPECTED_KINDS` member needs a disclosure site in the card or
   a line in `check-structure-baseline.txt`. This is the only leg that catches
   hooks and settings, and the only one that catches a ninth kind added without
   a card line. **Decision (2026-07-31): the hooks/settings gap is fixed, not
   baselined** — both gain disclosure sites on the card in the same change that
   introduces this leg. Hooks are an executable kind carrying the full
   ceremony, so a yes to an unseen hook is the precise surprise Phase 2's gate
   counts; leaving it for Phase 3 would ship a card that omits an executable
   capability.
3. **Framing lines (residual).** Section headers, the ceiling line, the
   "unbound, by design" note and the blocker summary never touch `mark`. Their
   honest witness is a golden transcript over the same fixture, scoped narrowly
   to those lines so legs 1–2 keep owning what can be proved.

Honest limits: leg 1 is substring containment, not semantics — it cannot catch
a card that lists an item but describes it worse. Coverage is fixture-relative;
the fixture is hand-maintained and is the one irreducibly rotting input, which
is what leg 2 exists to make detectable.

## (b) Prior bytes

### The correction

The problem was framed as "lock digests alone cannot render a diff, so build a
content-addressed snapshot store under `~/.agentstack` keyed by digest." **That
store already exists and is already correct.**
`~/.agentstack/store/content/<sha256>/` (`crates/cli/src/store.rs`,
`snapshot_content`) is write-once, keyed by exactly the checksum the lockfile
records, re-verified before reuse, crash-safe via temp-then-rename, and never
evicted — there is no GC or prune path in the workspace. Building a second one
would duplicate it.

The real gap is narrower and in two parts:

1. **No pointer.** `SurfaceItem.identity` is what the review *showed*, not the
   pin: skills record the origin word (`"library"` / `"inline"`) and
   instruction fragments record `""`. Servers, extensions, workflows and policy
   already record their real identity — command line, target, roles, rules —
   which is why `ReviewDiff` can already mark `+`/`~`/`-` for those kinds
   today. Skills and instructions are the two kinds that cannot, and the reason
   is a missing field, not missing bytes.
2. **Path sources have no bytes.** `SkillSource::Path` resolves to the live
   project directory and never calls `snapshot_content`. This is exactly the
   Phase 1 funnel shape — an adopted drop is declared as `path = "./skills/x"`
   — so the drop-a-file-then-`yes` case is the one case with a genuine byte
   gap.

### The contract

- **Record the pin.** `SurfaceItem` gains an **additive, optional** field
  carrying the approved content digest. It is *not* folded into `identity`:
  that field's documented meaning is "what the human agreed to run/contact, not
  whether it happens to be locked," and overloading it would both contradict a
  deliberate decision and make every existing skill read as `~ changed` on the
  first re-trust after upgrade. Additive and `Option`-typed means old records
  deserialize unchanged and simply have no prior bytes to offer — the degrade
  path, not a migration.
- **Capture bytes for path sources at pin time**, through the existing
  `snapshot_content`, at the moments consent pins bytes. No new store, no new
  digest computation.
- **No backfill.** It is impossible and is not attempted. A re-gate with no
  recorded snapshot shows today's changed-content message **plus the pin
  identity** — honest about what it does and does not know.

This is a read-side addition in `cli` plus one optional field on a `trust`
serde record. It touches neither digest computation, nor byte-change re-gating,
nor the grant path. The field addition still gets line-by-line review per
`CLAUDE.md`, because it lives in the `trust` crate.

### The re-gate card

Names *what* changed (which skill, instruction, or server entry), shows the
real diff **when it is small**, and offers three first-class answers:

| Answer | Meaning |
|---|---|
| **accept** | approve the new bytes; they become the pin |
| **keep pinned** | keep using the bytes already approved — not a deferral |
| **block** | refuse; nothing activates |

`keep pinned` must actually keep the pinned bytes in use, which is what makes
the snapshot load-bearing rather than decorative. The diff is **capped**: a
400-line rewrite names the files and the counts and never floods the terminal.

Two consequences follow, both inherited obligations rather than new scope:

- **Re-gates join the compressed path** once the diff card exists, per
  `STRATEGY.md`. Phase 1's event-parity witness extends to the regrant flow —
  same actions, same order, same digests as the explicit sequence.
- **The collision refusal becomes this card.** Phase 1 refuses a dropped file
  whose name is already declared. That refusal already knows the existing pin
  identity, so it becomes "this name is declared as `<pin>`; the drop would
  replace it" with the diff — **still refuse-by-default, now informed.**

## (c) Recognition

A machine-local index of past approvals, keyed by content digest, written on
grant and read at card time. Its only effect: **it shortens the card's body**
— "this exact content is approved in two other projects on this machine" — so
the user reads less of what they have already read.

Bounds, all witnessed:

- **It never shortens the gate.** The per-project yes still happens. Path-keyed
  trust exists to bind consent to context; recognition preserves that binding
  deliberately.
- **Presence changes lines only** — never the outcome, never the recorded
  events.
- **Index absent or unreadable → the full card.** Same as any other degrade.
- **It stores digests and project keys, never content.** The snapshot store
  holds content; the index must not duplicate it.
- **It never crosses machines.** Not synced, not shared, not exportable.

**Machine-level standing approval ("always allow this exact content, anywhere
on this machine") is explicitly NOT built here.** It is a widening of the gate,
not a presentation of it, and gets its own review.

## What these are not

For [`docs/ENFORCEMENT.md`](../ENFORCEMENT.md), where the honest statement
lives beside its neighbours:

- The snapshot store and the recognition index are **implementation-internal
  on-disk state**, not manifest capability kinds. They add no policy dimension
  and no lock entry.
- Neither is **tamper-evident**. Both live under the user's own
  `~/.agentstack` at user permissions. A local attacker who can write there can
  write anything else that matters too — but the card must never *imply* an
  integrity property it does not have.
- Neither is **synced**, and neither is **consulted by verification**. Trust
  checks read the trust store and the lock, exactly as today. These two are
  read at card-render time only.
- The snapshot store is **not a backup** and not a recovery mechanism. `undo`
  and `restore` are unrelated paths.

## Panel

The panel consumes structured JSON from `preview_value()` through
`ui_contract::envelope`, so re-labelling existing fields flows through with no
panel work. **`ConsentCard` content has no machine-readable form today** — it
is print-only inside `grant_gated`, so the funnel's and the diff card's lines
reach the terminal and nothing else. Emitting them structurally (a `diff`
object mirroring `ReviewDiff`, behind a new feature name such as
`trust-review-card-v1` — a new name, never a revision of `trust-preview`, so an
older binary is not misrepresented) is CLI-side work; the panel cannot pick it
up until the CLI emits it. Recorded here as the t3code follow-up.
