# Where the automatic-delivery arc sits

Type: grilling
Status: resolved
Blocked by: 05

## Question

`docs/design/automatic-delivery.md` is the only live design contract: five workstreams, W2 a security release blocker of the dynamic lane, the flip no earlier than the release after v0.18.0. Under an evidence-first plan, where does this arc sit — queued behind adoption evidence, interleaved (e.g. W2 early because it is security), or explicitly parked with a named resume trigger? v3 must place it deliberately so the contract and the plan agree.

## Answer

Resolved 2026-08-02 by maintainer decision in a wayfinder grilling session.

**The arc parks behind the study-result trigger; W2 is first out of the park.** No automatic-delivery work runs before the activation study. When the trigger fires and the three observed blockers are fixed (the kit's §7 rule), the queue takes **W2 — trust checked at dispatch — first**, because it hardens an already-shipped opt-in surface: the lease path exists today, and a revoked yes leaving a live connection serving is a gap in current behavior, not a future one. The remaining workstreams follow in the contract's own order (W4 last), and the flip stays behind all eight preconditions in `docs/design/automatic-delivery.md` — which remains the operative contract v3 points at, not text v3 restates.

Rejected: landing W2 before the release (re-cutting the pinned study instrument a second time in one day, delaying the evidence the whole plan waits on), and parking the arc behind the real-usage trigger (risks the one live design contract going stale while its security fix waits on adoption nobody has measured yet).

### Amended 2026-08-02 (later the same day)

Inverted by the same maintainer decision: **the arc is no longer parked — it is the queue.** W2 first (its security reasoning unchanged), then the contract's order with W4 last; the flip still behind all eight preconditions, including the now-deferred study (precondition 8).

**Note:** the "including the now-deferred study (precondition 8)" statement above is superseded by [Dynamic-first depth](11-dynamic-first-depth.md) — the study no longer gates the flip.
