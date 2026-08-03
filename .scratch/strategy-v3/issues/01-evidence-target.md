# Who is the evidence from?

Type: grilling
Status: resolved

## Question

"Evidence first" needs a named adoption target before anything downstream can be decided. Which developers is v0.18.0 for — solo developers running multiple coding CLIs, teams standardizing agent setups across a repo, security-conscious organizations, or someone else? Which harnesses do they actually run, and where are they reachable? The answer shapes the fate of the activation study (04), the distribution plan (07), and the audience v3's language addresses.

## Answer

Resolved 2026-08-02 by maintainer decision in a wayfinder grilling session.

- **Primary target: multi-CLI solo developers** — people personally running two or more coding CLIs who feel config sprawl today. Portability is the visible hook; the trust gate is the differentiator underneath. Explicitly not the primary v0.18.0 evidence source: security-conscious team leads (need more maturity than an rc), skill authors/sharers (their evidence validates the exchange story, not the everyday one), and the eve-curious (a channel, not an audience — revisit under distribution).
- **Beachhead harness pair: Claude Code + Codex** — the combination that must be flawless, that all first-run material demonstrates, and that the maintainer dogfoods daily, so the evidence path is self-verifiable before anyone else touches it. Other harnesses stay supported; they are not the demo path.
- **Reachability (facts for the distribution ticket, not the channel decision):** public developer channels — HN, X, and the Claude Code / Codex communities.

Unblocks [Fate of the activation study](04-activation-study-fate.md) (the instrument now has a named subject) and [Distribution: how users arrive](07-distribution.md).
