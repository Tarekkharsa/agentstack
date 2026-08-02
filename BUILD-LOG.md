# BUILD-LOG

Running log of the strategy-v3 build (TODO.md items 1–8), one honest paragraph
per item. Written by the build loop; the bar stays the maintainer's.

## Item 1 — W2: trust checked at dispatch (branch `w2-trust-at-dispatch`)

Shipped: a `TrustAnchor` (project root + consent digest, captured only when the
project is Trusted at gateway build) re-verified from disk on every upstream
dispatch and every `tools/list`; a violation empties the upstream capability
surface while control-plane tools stay reachable; refusals are seatbelt-shaped
under a new sixth `Trust` denial family (`tool: trust` in `calls.jsonl`, own
`TrustRefused` run event carrying identity, never bytes or arguments).
Witnesses: the three contract cases — revoke, out-of-band manifest edit,
wholesale lock replacement — each stop the NEXT call on a live stub-upstream
connection; control-plane survival on a real `agentstack mcp` subprocess;
seatbelt shape plus hostile-name non-forgeability; no MCP-invocable consent
path; the emptied `tools/list`. Judgment calls: no generation-token cache at
all — with no filesystem watcher any cache is an unauthoritative guess, and
the contract blesses always-recompute (three small reads plus one SHA-256 per
dispatch); a new denial family rather than reusing Pin, because "the yes
stopped applying" is not "the bytes drifted" and mislabeling would break
claims-match-enforcement (ENFORCEMENT.md updated to six families;
`STRATEGY.md`'s "five denial families" line is a historical v2 statement left
for the maintainer); never-trusted eager projects stay ungated
(consent-by-invocation preserved); a lease transition under a violated anchor
installs an empty gateway rather than rebuilding an ungated one. Debts and
tensions named for the maintainer: (1) eager mode on a never-trusted project
still has no dispatch gate — the contract's "every gateway dispatch" versus
the code's consent-by-invocation model needs an explicit ruling; (2) an eager
connection re-trusted mid-flight stays refused until the client reconnects
(fail closed; the refusal text names the fix).
