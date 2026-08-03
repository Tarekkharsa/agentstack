# Metrics scoreboard in v3

Type: grilling
Status: resolved

## Question

Does v3 keep v2's north-star metrics scoreboard — TTLC, concepts-before-value, review comprehension, recovery time — and if so, measured how? The F19 privacy-preserving measurement design is archived; the re-adopted activation study carries baselines for one-shot moderated measurement, but a scoreboard implies ongoing measurement v2 never defined. Candidate answers: keep all four, measured only through opt-in studies; trim to what the study actually observes; or drop the scoreboard and let the study's pass condition be the whole measurement story.

## Answer

Resolved 2026-08-02 by maintainer decision in a wayfinder grilling session.

**No scoreboard in v3; the study kit owns the metrics.** v2's scoreboard existed to score phases ("if a phase ships and its metric does not move, the phase is not done"); with phases gone, that role is gone. The re-adopted activation-study kit already defines and baselines all four metrics (TTLC, concepts-before-value, review comprehension, recovery time) inside its own protocol, and its pass condition covers the ground the scoreboard was watching. v3 states the release gate and nothing more.

Ongoing measurement is deliberately deferred, not dropped: it becomes a question **when the real-usage trigger fires** — that revisit is where telemetry-vs-studies gets decided with actual users in the picture. The F19 privacy-preserving measurement design stays archived until then.
