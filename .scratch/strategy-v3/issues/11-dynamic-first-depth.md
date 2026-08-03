# Dynamic-first depth

Type: grilling
Status: resolved

## Question

Raised by the maintainer during Draft v3 review: why is dynamic not the default — and can the other delivery modes be removed outright, keeping only dynamic?

## Answer

Resolved 2026-08-02 by maintainer decision.

**Dynamic becomes the default at arc-end.** When the automatic-delivery workstreams land (W2 first), gateway delivery is the default for skills and servers on MCP-capable harnesses — an outcome of the arc, not an ambition behind gates.

- **No user-facing modes.** The planner is one automatic behavior; "static mode", "clean-at-rest", and the "Prefer gateway" override disappear as user-facing concepts.
- **One escape hatch:** "render locally", per project or harness — offline machines, corporate no-daemon policies, and plain-files inspection are real needs; one named override covers all of them.
- **The rendered lane itself stays — physics, not a mode.** Instructions, settings, hooks, and extensions cannot travel over MCP; rendering is the only delivery path those kinds have. Non-MCP harnesses likewise keep full static delivery. Dynamic-only-for-everything was considered and rejected as impossible without dropping those capability kinds.
- **Precondition 8 is amended out of the contract.** The flip's study precondition existed to protect an instrument pinned to the static-default RC; with the study deferred to the bar-met moment and re-pinned then, it protects nothing. The remaining preconditions hold — they are the arc's own acceptance criteria.
- Adoption follow-through: amend `docs/design/automatic-delivery.md` accordingly (default wording, override set, precondition list).
