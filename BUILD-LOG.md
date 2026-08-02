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

### W5 (schema half) — the package layer

Shipped: a library package is `<lib_home>/packages/<name>/pack.toml` plus
member bodies, indexed as `[[package]]` in `library.toml` — the same artifact
the git pack rail already parses, so its name-contract gate and content scan
are reused rather than forked; the index checksum deliberately covers
`pack.toml` only, because a roll-up digest would let a member's bytes move
inside an "unchanged" package. A toolset selects packages with
`packages = [...]`, and `agentstack lock` expands each selection into the lock
as `[[package]]` (name, exact version, source, rev, the toolsets that pin it,
and removals) plus `[[package.member]]` rows carrying name, kind, origin,
checksum, and provenance. Every digest comes from an existing pinning act —
no second pinning or digest path. Per-member overrides are expressed as
`[package_overrides.<pkg>]` with `remove` and `replace`, and the effective
member set is visible in both the lock (per-member `origin` plus `removed`)
and `status --json`, gated on the new `package-members-v1`, so an override can
never diverge silently. Instruction members are toolset-scoped, always carry
the rendered lane on their pin so no surface can call one "live via gateway",
and are never materialized into `[instructions.*]`, which would orphan a
declaration when the package goes away.

Witnesses: `package_layer` (7) — a selection expands to exact members with
digests and provenance asserted field by field; runtime resolves the locked
member set after the library has moved ahead; an override reports as an
effective set naming both origins; a package carrying hooks or extensions is
refused by name; an unresolvable or drifted member fails closed; and a project
referencing no package has byte-identical manifest and lock.

Judgment calls: `pack.toml` now parses `[[hook]]`/`[[extension]]` **in order to
refuse them** — previously serde dropped them silently, so a pack declaring
hooks installed as though it had none; this tightens the shipped git pack rail
too, and is flagged as a deliberate fail-closed behaviour change. Expansion
happens in `lock` only (matching how instructions, extensions and workflows
already pin), scoped to selected toolsets, while pruning is scoped to every
declared toolset so `lock --profile backend` cannot drop another toolset's
expansion. A package's `pack.toml` drifting from its library index pin refuses
rather than reading as "the library moved ahead" — that exemption is fenced to
the MCP serve path for already-pinned skills, and this is an intake gate. Debt,
named in the design doc rather than hidden: a package instruction member is
pinned and reported but compiles into no file yet; that wiring is delivery-side
and is folded into W5's runtime half.

### W5 (runtime half) — boundary, laziness, and package members that actually reach something

Shipped: package instruction members now compile into the managed region from
their **pinned** bytes through the existing `plan_instructions` + `merge_md`
path, on the rendered lane only, and — following W3's precedent — `lock` never
creates an instruction file and never adds a region to a file that had none;
when a member changed but no target carries a region it says so plainly and
names `agentstack instructions --write`. `agentstack_load`/`list_loadable` now
read descriptions from the pinned store snapshot instead of live library
frontmatter, closing the seam W3 recorded as a debt (and making the listing
cheaper, since pinned skills skip resolution entirely). Package-carried
**server** members — previously pinned and reported but never served — now join
the gateway's upstream set as the same `FrozenServer` shape everything else
uses, fenced to a toolset that selects the package, resolved from the lock's
pinned definition rather than the package's current `pack.toml`, with a new
`Store::pin_server_definition` supplying the deposit that family was missing
entirely (it had a digest and no bytes).

Lazy start was **verified, not built**: gateway construction spawns no stdio
child and opens no socket, and `try_call` reaches one slot directly rather than
through `namespaced_tools()`, so a tool call starts exactly the server that owns
the tool.

Witnesses: `package_layer` (15 total) — the boundary is discoverable with no
body entering context; a listed description comes from pinned bytes; an
instruction member renders only into an existing managed region; a
package-carried server is exposed only under a toolset that selects it,
resolved from the lock rather than the mutated package, not started until one
of its tools is called, and fails closed with a visible reason when its pinned
definition cannot be verified.

Judgment calls and limits, all deliberate: **transparent mode's tool listing
defeats lazy start** — that path plus `tools_search`, `tools_bindings` and
code-mode execution enumerate every upstream, which necessarily starts every
stdio child and dials every HTTP endpoint. You cannot enumerate a server's
tools without asking it; the only fix is a persisted per-server tool-list cache
keyed by definition digest, which is new on-disk state on the delivery path and
a new capability lane, so it was witnessed and documented rather than built.
**This is a direct input to W4:** if the flip makes transparent mode the
default, lazy start is defeated for everyone and the honest surfaces must say
so. Package servers reach the live host gateway but **not** `run --sandbox` /
`--lockdown`, because the grant handoff artifact would need a new binding kind
— authority construction, deliberately not built. Package members carry no
standing re-gate answers (`Blocked`/`KeepPinned`); not a content-binding hole,
since every member digest is in the lock and the lock is in the trust digest,
but a consent granularity that does not exist. A package server whose name is
already claimed is refused rather than shadowed, and an unfenced gateway serves
no package servers at all (unfenced is already maximal, so unioning them in
would make package membership a widening mechanism). **Flagged first for
review:** a package member's `ResolvedServer` carries `ServerOrigin::Inline`
rather than a new variant — carried but never consulted on this path, with
`provenance` naming the true source — chosen over rippling a third variant
through six display and lockfile-schema sites.

### W4 (registry half) — leases become visible, and the fence was found open

**The finding that matters most in this item.** Flip precondition 3 (toolset
fencing) did not hold. With no lease open, the auto-project gateway built
`Gateway::from_manifest`, whose ambient fence resolves to `None` and serves
`effective_runtime_servers(.., None)` — every manifest server *plus* every
toolset's, the implicit union the contract explicitly forbids — reachable
through `tools_search`/`tools_execute`. Verified empirically by disabling the
fix and watching both fixtures' tools list. Now, in the zero-files lane, a
project that declares any toolset gets an empty gateway until a lease names
one, and closing a lease returns to that state.

Also shipped: a machine-level lease registry whose liveness is **derived at
read time**, never read from the file as truth — `live` only when the recorded
PID exists *and* its start token still matches; `stale` when the PID is gone or
exists with a different start time (PID reuse); `unknown` when start time is
unavailable, which never folds into `live`. Start time comes from
`/proc/<pid>/stat` on Linux (split at the last `)`, because the comm field can
contain parens) and `/bin/ps -o lstart=` on macOS with the PID passed as an
argv entry, never through a shell — no new dependency, no unsafe, nothing added
to `sys.rs`. `agentstack lease status [--json]` is the authoritative read,
advertised as `lease-status-v1`. Gateway-unavailable detection did not exist and
now does, defined as "the harness has the bridge registered but the command its
config names is not executable here"; a bare command name is deliberately never
reported, because the harness's PATH is not reconstructible and claiming an
unprovable outage is the same dishonesty as claiming unprovable health. One
sentence stem feeds both `status` and `doctor` so they cannot drift.
`ENFORCEMENT.md` gained the lease column with its honest qualifications.

Witnesses: `lease_registry` (5) — an open lease visible to another surface; a
stale record never reading as live, proven both by a dead PID and by a live PID
whose start time does not match (simulated reuse); no lease meaning
control-plane tools only with several toolsets declared; a lease exposing
exactly its toolset; an unavailable gateway yielding no tools, printing the
one-sentence outage from both surfaces, and leaving the project tree
byte-for-byte identical.

Judgment calls: the unleased fence is applied to the zero-files lane only, not
to `Gateway::build` generally — applying it in the constructor would reach
`agentstack run`, code mode, and the eager `--manifest-dir` bridge, where
naming a directory is itself the consent; that is a far larger blast radius
than precondition 3 asks for. The contract says "several toolsets" and the fence
triggers on one or more, because "two is fenced, one is not" would be an odd
cliff and a single toolset plus loose inline servers is still a union no
explicit selection stands behind. A registry write failure degrades to an
honest note rather than costing the user their toolset, since the registry is an
observation surface with no enforcement role — a one-line change if the
maintainer prefers it fail closed. Honest qualification recorded in
ENFORCEMENT.md rather than left as a superlative: the lease column is strongest
on tools and audit and on the fence, **not** on isolation, where its egress and
filesystem rows are identical to the gateway column.
