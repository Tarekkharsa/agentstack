# Carry-forward audit of v2

Type: research
Status: resolved

## Question

Classify every section of `STRATEGY.md` v2 as keep-verbatim / rewrite / drop for v3, judged against ground truth: the shipped code (what actually exists at 0.18.0-rc.2) and `docs/design/automatic-delivery.md`. Flag claims v2 makes that the code no longer supports, amendments that can be collapsed into clean prose, and sections (invariants, non-goals, competitive watch, experience contract, carried-forward-from-v1) that survive as-is. Output: a fact table the draft (08) can lean on.

## Answer

Per-section verdict for STRATEGY.md v2, against ground truth (shipped code at
0.18.0-rc.2 + `docs/design/automatic-delivery.md`):

- **Keep-verbatim:** The goal, the design law, "What never changes"
  (invariants), Competitive watch: vercel/eve, Carried forward from v1 (minus
  two stale `TODO.md` pointers), and most of the experience contract
  (four-ideas table + six defining moments).
- **Rewrite:** the seven-deltas gap framing (D1-D7 are now mostly *closed*,
  not open), the experience contract's "Delivery ambition" paragraph
  (superseded by `automatic-delivery.md`'s concrete decision), Phase 4's
  "Trigger discipline" strikethrough-amendment (collapses to one sentence),
  and "Open design questions" (Q1 is answered, Q2 is decided).
- **Drop:** the five-phase plan mechanics and the Phase 0-3 narratives —
  their outcomes shipped and read better as facts than as a still-running
  plan. "How this document is used" also drops its phase-gate-feeds-TODO.md
  language.

Code-vs-doc mismatches found:

1. Phase 3 "seatbelt legibility" text calls egress/secret-scope refusal
   recording an open workstream item; `crates/cli/src/seatbelt.rs`'s doc
   comment shows both are already recorded (`SecretDenied`/`PinRejected`
   events) — code moved past v2's own text.
2. Open design question #1 ("where does the yes live in zero-files mode")
   is listed as unresolved in v2, but `docs/design/automatic-delivery.md`
   (adopted the same reset day) answers it directly under "Where the yes
   lives."
3. Several `TODO.md` cross-references (fork/deferral ledger, F19 measurement
   design, "Experimental workflows" checklist, M1 authority-kernel tracking)
   are now dangling — `TODO.md` was emptied to 12 lines in the 2026-08-02
   reset (`78fe54c`). The facts each pointer asserted were independently
   re-verified against code and still hold; only the citation trail broke.
4. `automatic-delivery.md`'s `consent-card.md` link resolves only into
   `docs/archive/design/consent-card.md`, not the live `docs/design/`
   directory — the one live design contract points at an archived file.

Branch: `research/carry-forward-audit`
Findings: `docs/design/research/carry-forward-audit-v2-to-v3.md`
