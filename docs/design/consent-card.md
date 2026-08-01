# The consent card

> **Status:** Active — the contract for Strategy v2 Phase 2 (the review card).
> Phase 2 shipped 2026-07-31, so most of this document now describes behaviour
> that exists; the one part still unbuilt is the structured `ConsentCard`
> payload in §Panel, and it is marked there. Direction lives in
> [`STRATEGY.md`](../../STRATEGY.md) "Phase 2 — One yes"; the ordered work
> lives in [`TODO.md`](../../TODO.md). This document does not restate either.
> It fixes the three *contracts* Phase 2 introduces, because they share storage
> and would otherwise be designed three times.
>
> **Where feature names are decided:** `crates/cli/src/ui_contract.rs` is the
> single source of truth for every advertised contract string. When this
> document and that file disagree, the file wins and this document is wrong —
> see the naming correction at the end of §Panel.

Three pieces, one document:

- **(a) the card** — what the grant screen says, and the rule that bounds it;
- **(b) prior bytes** — how a re-gate renders "3 lines changed" instead of
  "digest mismatch";
- **(c) content-digest recognition** — how repeated review of identical content
  gets cheaper without getting weaker. (Named in full to keep it apart from
  *publisher-key recognition* in the share/receive flow, which changes the
  card's words about provenance and deliberately does not shorten it.)

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
the snapshot load-bearing rather than decorative. Concretely: trust and lock
stay at the consented pin, **and the delivered artifact is materialized from
the content-store snapshot**. Where delivery normally symlinks into the project,
a keep-pinned item switches to a copy — a link would track the very drift the
user just declined, shipping the new bytes under the old pin's name. Status then
reports the divergence honestly; keep-pinned resolves *this consent moment*, it
does not silence drift.

`block` excludes the item from delivery (failing closed, as drift does today)
**and records the refusal as a standing state**, so `status` shows it once with
the way out named rather than re-asking on every command.

The diff is **capped**: a 400-line rewrite names the files and the counts and
never floods the terminal.

### Answers stage; the single final yes commits

> **The review loop may collect per-item answers as it walks. Nothing acts on
> them — no re-lock, no recorded decision, no pinned-copy, no exclusion — until
> the one final confirmation that already gates the grant.**

This is the constraint the wiring is most likely to violate, because acting on
each answer as it is given is the obvious implementation and it quietly creates
three or four moments where a human commits to something. There is exactly one
such moment, and it is the same one there has always been.

**Where the commit moment is (decided 2026-07-31, during wiring).** The contract
above says "the single final yes", and on the `agentstack yes` funnel that is
literally `ConsentCard`'s confirmation. On plain `agentstack trust` there was no
such moment: typing the command at a terminal *is* the consent, and that happens
**before** any per-item answer is given. Rather than let N answers commit with no
closing yes — the exact shape this section forbids — a re-gate that collected any
answer asks one final confirmation before committing. A clean review, or a
re-gate where the human answered nothing, is unchanged and prompts nothing.

**What `accept` must do (decided 2026-07-31, from an adversarial review).**
Accept re-locks, and the consent digest covers the lock bytes, so accept moves
the very digest the review rendered from. Both naive commit paths are broken:
with `--consented-digest` the grant fails `ConsentMismatch` *after* the lock was
rewritten (residue after a failed grant), and without one the grant records the
pre-accept digest and the project immediately reads `Changed` — the user accepts
and silently gets an untrusted project. The correct handling recomputes rather
than re-reads, following `repin`'s existing precedent ("computed from the written
content, never from a disk re-read"): the new digest is taken over
`snapshot.manifest` + `snapshot.local` + the lock bytes this process just
serialized. Two conditions keep that honest — the lock delta must contain
*exactly* the accepted items (never a manifest-wide re-pin, which would fold
un-consented moves, including items the human answered keep-pinned or block on,
into the granted digest), and the new checksum must be obtained through
`Store::pin` so the accepted bytes reach the content store and the next re-gate
can still diff.

Its witness extends Phase 1's `declining_leaves_nothing_behind`: walk a re-gate
giving all three answer kinds, decline the final gate, then assert the manifest,
the lock, the trust store **including its recorded decisions**, the delivered
artifacts, and the event log are all byte-identical to before. Answers given and
then declined leave no residue anywhere. `Rollback::capture` is the existing
shape; the trust store's decisions are the new thing it has to cover.

Two consequences follow, both inherited obligations rather than new scope:

- **Re-gates join the compressed path** once the diff card exists, per
  `STRATEGY.md`. Phase 1's event-parity witness extends to the regrant flow —
  same actions, same order, same digests as the explicit sequence.
- **The collision refusal becomes this card.** Phase 1 refuses a dropped file
  whose name is already declared. That refusal already knows the existing pin
  identity, so it becomes "this name is declared as `<pin>`; the drop would
  replace it" with the diff — **still refuse-by-default, now informed.**

## (c) Content-digest recognition

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

**Why the two renderers are not merged (decided 2026-07-31, from an adversarial
review).** The obvious plan — extract one `review_surface()` walk that both the
terminal review and `trust --preview` consume — is refuted by three properties
that are each deliberate:

1. **They disclose different things on purpose.** `preview_value` *redacts* a
   library server whose live definition does not match its lock pin, emitting
   an `unverified` marker instead of the command line, so an external UI cannot
   bind consent to bytes the digest does not cover. The authoritative review
   does the opposite — it prints the live command line with a `DRIFTED`
   annotation — because the card may never disclose *less*. A shared surface
   must pick one, and one of the two choices is a consent regression.
2. **Preview must not write.** The review walk reaches `Store::ensure_worktree`
   (git worktree materialization) through the lock-status resolvers. Preview's
   read-only contract is shipped, witnessed, and consumed by the panel RPC;
   sharing the walk would add git subprocesses and unbounded latency to it.
3. **The lint anchor pins the walk in place.** `tools/check-structure.py`
   locates the `:review` disclosure evidence by matching literal `diff.mark(`
   inside `crates/cli/src/commands/trust.rs`. Moving the walk silently voids
   the requirement for every kind.

So the contract is narrower and honest: the preview gains the kinds it can
compute with **no resolver and no store** — hooks, settings, and policy, which
are pure manifest reads — and says plainly what it still excludes and why. That
closes the disclosure gap that mattered most (hooks are executable and were
absent from the machine-readable surface entirely) without moving a walk that
must not move.

A related hazard, recorded so a future refactor does not rediscover it:
`PendingAnswer.blocker_ix` is a positional index into one global `blockers`
vector. Decomposing the walk into per-kind functions with local vectors would
misalign it, and the failure is silent — accepting one item could clear an
unrelated item's blocker, emptying the vector so the bail never fires and an
unpinned surface is trusted. Any future decomposition must key blockers by
`(kind, name)` first.


The panel consumes structured JSON from `preview_value()` through
`ui_contract::envelope`, so re-labelling existing fields flows through with no
panel work. **`ConsentCard` content has no machine-readable form today** — it
is print-only inside `grant_gated`, so the funnel's and the diff card's lines
reach the terminal and nothing else. Emitting them structurally (a `diff`
object mirroring `ReviewDiff`, behind a new feature name — a new name, never a
revision of `trust-preview`, so an older binary is not misrepresented) is
CLI-side work; the panel cannot pick it up until the CLI emits it. Recorded
here as the t3code follow-up.

> **Naming correction, 2026-08-01. `trust-review-card-v1` is taken.** An
> earlier draft of this paragraph proposed that name for the unbuilt structured
> `ConsentCard` payload above. P2.D then shipped it meaning something narrower:
> in `crates/cli/src/ui_contract.rs` the advertised feature
> `trust-review-card-v1` means *`trust --preview` additionally carries `hooks`,
> `settings`, `policy_requested`, and `machine_policy_ceiling`, plus `hooks`
> and `settings` counts*. **The shipped meaning is authoritative** — external
> consumers (the t3code fork) negotiate against the binary, not against this
> document, and a fork built from the old draft would have targeted a payload
> that does not exist.
>
> The unbuilt structured-`ConsentCard` payload therefore needs a *different*
> name when it is built. Working name: **`trust-card-diff-v1`** — chosen over
> `trust-review-card-v2` because it is not a revision of the shipped payload
> (that one carries kinds; this one would carry the card's rendered lines and a
> `diff` object), and a `-v2` would falsely imply a migration path off `-v1`.
> The final name is fixed when the payload ships, in `ui_contract.rs`, which
> stays the single source of truth for every advertised feature string.
