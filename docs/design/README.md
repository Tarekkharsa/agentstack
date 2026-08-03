# Design documents

Design documents explain **active technical contracts**: why a boundary is
where it is and what it guarantees. None of them is a roadmap, and none
authorizes work — that belongs to `STRATEGY.md` and `TODO.md`.

| Document | Answers | Status |
|---|---|---|
| [`automatic-delivery.md`](automatic-delivery.md) | How each capability reaches a harness: the dynamic (gateway lease) versus rendered lane routing, where the yes lives on the lease path, and what must hold before dynamic becomes the default | Active contract; adopted 2026-08-02 |
| [`pinned-serving-and-library-drift.md`](pinned-serving-and-library-drift.md) | Why a central library moving ahead of a project's pin is an update available rather than project drift, what the load path serves instead, and the scope fence around that exemption | Active contract; adopted 2026-08-02 |
| [`package-layer.md`](package-layer.md) | What a package looks like in the central library, how a toolset selects one, how the lock expands that reference into exact members with per-member digests and provenance, and how a per-member project override stays visible as an effective member set | Active contract; adopted 2026-08-03 |
| [`linked-library-sources.md`](linked-library-sources.md) | What a library source is now that any folder can be one, where the ordered link list lives, how the first-match-wins precedence rule decides a name across sources and surfaces the shadowed copies, and why none of it can change what a locked project serves | Active contract; adopted 2026-08-03 |
| [`instruction-variants.md`](instruction-variants.md) | How one instruction fragment carries per-(CLI, model) variants, how the most-specific-wins precedence resolves, how every variant body is pinned, which harness is actually *known* to consume which instruction channel, and how the model is determined — or honestly reported unknown | Active contract; adopted 2026-08-03 |
| [`activation-study.md`](activation-study.md) | The activation-study kit — the instrument for the bar-met moment (promoted from the archive 2026-08-02). | Ready to run; RC pin re-cut when it runs |

Everything written before the 2026-08-02 reset lives in
[`docs/archive/`](../archive/) — history, never direction. Consult it (or
git history) only when researching lineage; do not treat it as a second
roadmap.
