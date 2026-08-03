# What replaces phases and gates

Type: grilling
Status: resolved

## Question

V2's phase/gate structure collapsed on zero users: every inter-phase gate was amended down to maintainer acceptance, and the doc accumulated strike-throughs recording the deviations. What is v3's plan control structure — a simple ordered queue with tripwires, evidence milestones that unlock work, or something else? The structure must survive contact with a zero-to-few-users reality without needing amendments.

## Answer

Resolved 2026-08-02 by maintainer decision in a wayfinder grilling session.

**Queue + revisit triggers.** No phases, no gates. v3 states direction, invariants, and named revisit triggers; `TODO.md` stays the sole sequencing authority. Evidence changes what is next — it never blocks all motion. Deviations edit the queue; the strategy document is reopened only when a named trigger fires, so it cannot accumulate amendments the way v2's gates did.

The named triggers:

1. **The activation study result** — pass or fail. Pass re-seeds the queue with launch plus the three-blocker fixes; fail makes the blocker list the roadmap (the kit's §7 rule, unchanged).
2. **Competitive tripwires** — v2's eve tripwires re-worded per the 2026-08 watch refresh: registry-as-distribution explicitly tracks vercel-labs/skills, and config-import moves by any major CLI (e.g. Codex `/import`) join the list.
3. **Real-usage threshold** — first sustained external users (issues/PRs from strangers, or a named install count). The moment maintainer acceptance stops being the only available evidence, the plan is rethought with users in it.

Deliberately not a trigger: t3code becoming publicly obtainable — the carried-forward note about revisiting its role may survive elsewhere, but it does not reopen the strategy on its own.
