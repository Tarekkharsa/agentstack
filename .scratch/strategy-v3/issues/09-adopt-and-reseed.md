# Adopt v3 and re-seed TODO.md

Type: task
Status: resolved
Blocked by: 08

## Question

The final yes, HITL: the maintainer adopts the draft (possibly after revision rounds), `STRATEGY.md` is replaced, `TODO.md` is re-seeded with the plan's ordered first items (starting with shipping v0.18.0), and the map closes. Record in the answer what was adopted and the commit that landed it.

## Answer

Resolved 2026-08-02. The maintainer gave the final yes and the adoption was applied to the working tree (uncommitted, for the maintainer's review):

- `STRATEGY.md` replaced with the accepted v3 (header changed from DRAFT to operative; "On adoption" retitled "Adoption record (2026-08-02)").
- `docs/design/automatic-delivery.md` amended: amendment note added; one override (Render locally); the flip's precondition list reduced to seven with the study precondition struck; flip lands with W4 at arc-end.
- `docs/archive/design/activation-study.md` promoted to `docs/design/activation-study.md` (git mv) and indexed in `docs/design/README.md`. The move repaired two pre-existing references (CHANGELOG.md and a Rust doc comment) that already pointed at the new path.
- `TODO.md` re-seeded with the nine-item queue from "The plan".

`make-docs-pages.py` and `check-docs-site.py` both pass. The adoption commit itself is the maintainer's; once committed, this map's destination is reached and the effort closes.
