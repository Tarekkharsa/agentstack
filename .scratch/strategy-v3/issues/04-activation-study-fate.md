# Fate of the activation study

Type: grilling
Status: resolved
Blocked by: 01

## Question

The §1.6 activation study is v0.18.0's sole release gate, but its design lives in `docs/archive/` — history, not direction — and it is pinned to rc.2. Given the named evidence target (01): re-adopt the study as designed, redesign it lighter, or replace it with a different evidence instrument (e.g. real-user feedback after a soft launch)? Whatever the answer, v3 must state the release gate in operative text, not by citation into the archive.

## Answer

Resolved 2026-08-02 by maintainer decision in a wayfinder grilling session.

**Re-adopt the kit as-is.** The archived kit (`docs/archive/design/activation-study.md`) is ready to run: five participants matching the named evidence target (multi-CLI solo devs), pilot-rehearsed, its one found blocker fixed in v0.17.1, and its install line already re-cut on 2026-08-02 to pin v0.18.0-rc.2. Protocol, thresholds, and the §7 pass condition are unchanged and stay unchanged.

- The study remains **the release gate**: v0.18.0 publishes only when it passes (≥4/5 finish unaided; median install→clean doctor < 5 min; ≥4/5 say "one setup across CLIs"; 5/5 needed zero advanced concepts; ≥4/5 understood every block), and the three-blocker rule holds — the top three observed blockers are fixed before any new feature work.
- v3 states this gate in its own operative text, never by citation into the archive.
- Mechanical follow-through (carried by the draft and adoption steps, not a new decision): promote the kit doc from `docs/archive/design/` back into `docs/design/` and index it in `docs/design/README.md`, so the gate's instrument is operative again.

Rejected: lightening it (the thresholds were pilot-calibrated; weakening the falsifier saves days and costs the thesis test), and both soft-launch variants (they lose the moderated observation that the delivery flip's precondition 8 depends on).

### Amended 2026-08-02 (later the same day)

By maintainer decision during Draft v3 review: **the study no longer gates the next release.** The maintainer will not put the product in front of other people until personally happy with it; the queue is now finishing the automatic-delivery arc. The kit survives, ready to run — it returns when the maintainer declares the bar met, as the way "other people" begin, and it remains precondition 8 of the delivery flip (the contract still requires it; deferring the study defers the flip's last precondition, it does not delete it). Its rc.2 pin is re-cut to the then-current RC when it runs.

**Note:** the "remains precondition 8 of the delivery flip" statement above is superseded by [Dynamic-first depth](11-dynamic-first-depth.md) — the study no longer gates the flip.
