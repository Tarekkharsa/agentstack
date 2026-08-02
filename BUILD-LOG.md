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

## Item 2 — the delivery arc (branch `delivery-arc`)

### W1 — the yes on the lease path

Shipped: refused leases and refused loads now emit the same two-destination
evidence the W2 dispatch refusal writes — one `calls.jsonl` row (`tool: trust`,
denied) plus the run-scoped `TrustRefused` mirror — where before, the one
moment a project stopped serving was the one moment nothing could be looked up
afterwards. All three lease-path refusals share the seatbelt sentence shape.
`status --json` gained an optional `needs_your_yes` object (refused count, last
timestamp, the one fix command), present only when calls were actually refused
since the last yes; doctor's existing trust finding carries the refused count
rather than a new check family; `needs-your-yes-v1` is registered in the
ui-contract. No card payload rides on any of it: the refusal names `agentstack
trust`, and that command remains the only renderer of the one authoritative
card, so "discloses no less" is met without a second card walk. Witnesses: six
in `crates/cli/tests/yes_on_lease_path.rs` — refused lease and refused load
each named-and-recorded, the refusal leading to an undiminished card while
carrying none itself, no MCP-invocable consent path on the lease path (forged
`agentstack_trust`/`_consent`/`_yes` calls all error and the trust store is
byte-identical across them), `status` naming needs-your-yes after a refusal,
and a live-connection dispatch refusal proving the two recording paths and the
one reading path agree on the project string. Judgment calls: the refusal's
`state` tag is re-derived from disk rather than parsed back out of the
human-readable note, which costs precision honestly (a revoked yes reads back
as `untrusted` here, since the store keeps no trace of a removed entry — only
the dispatch path, holding the anchor, can say `revoked`); the needs-your-yes
read is skipped entirely for trusted projects so `status` stays instant; the
capability name rides only in the denial's subject slot, following the Pin
family, after the first pass produced stutters like "helper tried to load skill
helper". Debt for the maintainer: **no path is canonicalized on this seam**.
The three sites agree on the derivation but each takes the caller's spelling
verbatim, so a gateway launched with `/var/...` and a `status` run from
`/private/var/...` would under-report the refusal count. Nothing serves that
should not — this is an honesty/reporting gap, not an enforcement one — and the
real fix is canonicalizing once inside `commands::load`, a wider behaviour
change than W1 should make unilaterally.

### W3 — update semantics

Shipped in three parts. **Serving:** the reproducibility rule is now real —
`Store::pinned_content` makes the content-addressed snapshot the thing a skill
body is read from, so the loader never reads the mutable library directory.
An absent snapshot self-heals by depositing the live bytes (safe only because
that line is reached solely after the pin has been proven equal, and the
deposit re-proves the address as it lands); a present-but-unverifiable snapshot
refuses, naming `agentstack lock`, and never falls back to live bytes. **The
library moving ahead is an update, not drift:** for a *library-sourced* skill
carrying a lock pin whose store snapshot verifies, divergence from the live
library now serves the pinned bytes plus a note offering `agentstack lock`,
instead of blocking. That decision, its two contract readings and the lines
that settle them, the scope fence, and the invariant walk are written up in
`docs/design/pinned-serving-and-library-drift.md`. **The rendered lane was
leaking:** `use --write` (and `add`) materialized library skills as symlinks
into the *live* library, so after a `lib sync` a harness read new, unreviewed
bytes through an unchanged link with no re-gate — pre-existing, and exactly
what W3's acceptance forbids. Both call sites now materialize against the
pinned store snapshot, refusing outright if the store cannot produce verified
bytes. **Offer and report:** `status` gained an optional `updates` object from
a check that makes no network call at all (ledger tag versus already-fetched
local git refs), so it can neither hang nor fail; upgrade reports the dynamic
and rendered lanes on separate lines; and the mixed-lane upgrade became one
transaction — lock re-pin, instruction pins, and the managed-region re-render
all moved inside the rollback envelope, which now restores manifest, skill
dirs, instruction fragments, the lockfile, and every instruction file carrying
a managed region.

Witnesses: `lib_sync_does_not_disturb_projects` (6) — a sync leaves every
project byte identical including symlink targets, a project keeps serving its
pinned bytes, a pinned skill is served from the store while the live library is
mutated underneath, an inline skill that drifts still blocks, a rendered
artifact still reads its pinned bytes after a sync, and a tampered store copy
refuses to materialize; `upgrade_lanes` (4) — the all-or-nothing transaction
proven by a real failure injection (an unwritable second adapter directory,
failing after the manifest, assets, fragments, lock and the first region were
all written, with all five artifact classes asserted byte-identical
afterwards), separate lane lines with no "gateway" claim over an instruction,
no instruction file created where none existed, and the update offer with its
honest negative.

Judgment calls: the moved-ahead exemption requires an *already-verified*
snapshot (a separate `has_pinned_content`) so the repair branch stays
unreachable there and `pinned_content`'s "caller has proven the digest"
contract is never weakened; the store redirection is unconditional on origin,
so an inline skill's rendered artifact also reads pinned bytes — consistent
with the MCP lane, where a drifted inline skill already blocks, and strictly
better than the old link that leaked ungated edits; the contract's example
dynamic-lane line "live via gateway now" was deliberately NOT printed, because
it is false on static projects and the default is still static until W4 —
revisit at the flip. Debts: `agentstack_list_loadable` still reads skill
descriptions from the live library, so an index line can be newer than the body
that loads; `use`, `doctor`, and `trust` still describe library divergence in
drift language that can read as breakage, and their copy needs a pass against
this decision. Flagged for line-by-line review: upgrade now writes instruction
pins (a digest-computation path it never touched) and reloads the manifest
inside its transaction, so a manifest-validation change can fail an upgrade
after the write.
