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

### W4 (planner, override, and the flip) — delivery is routed, and dynamic is the default

**The flip landed.** All seven remaining preconditions were verified first, each
against a witness that already existed or was written here: (1) `TrustAnchor`
re-verified from disk on every dispatch with no cache at all —
`trust_at_dispatch` (7); (2) pinned-byte serving and an announce-only sync —
`lib_sync_does_not_disturb_projects` (6); (3) the unleased fence, found open in
the registry half and closed there — `lease_registry`'s two fencing witnesses;
(4) the registry with liveness derived at read time from PID plus start token —
`lease_registry`'s stale/reuse witness; (5) mixed-lane atomicity proven by a
real failure injection — `upgrade_lanes` (4); (6) gateway-unavailable detection
with no writing path — `lease_registry`'s outage witness; (7) Render locally,
built here and witnessed in `delivery_planner`.

Shipped: `crates/cli/src/delivery.rs`, a pure planner over exactly two inputs
(capability kind, harness) plus the project's `[delivery]` table — it takes the
override table rather than the manifest so it cannot reach anything else. The
matrix is the contract's: skills and MCP servers dynamic on an MCP-capable
harness, instructions and settings rendered because MCP cannot inject them,
hooks and extensions rendered with the full ceremony always, and everything
rendered on a harness with no MCP. The three physical facts are tested *before*
the override, deliberately: Render locally can only move a capability towards
files, and testing it first would suggest a symmetry that does not exist. The
override is `[delivery] render_locally` per project and `[delivery.harness.<id>]`
per harness (most specific wins, in both directions), edited through `toml_edit`
like every other manifest edit, and cleared by *removing* the key — automatic is
the absence of an override, not a second stored value. `agentstack delivery
[--json]` is the read (`delivery-routing-v1`); `agentstack delivery
render-locally [--harness <id>] [--off] --write` is the write.

The flip itself is in two places. The planner's default lane, and the onboarding
fork: the wizard's delivery question became **automatic (recommended)** plus
"more control…", and a *scripted* setup on a project that has never rendered now
takes automatic where it used to keep the derived static mode. The automatic
fork states the routing, offers the one bridge registration the live lane needs,
points at the review, and **renders nothing** — the rendered lane's command is
the explicit `apply --write`. `init` states the routing per harness in its
pre-write review and again above its next-step list, in plain language that
stays inside `ordinary_journey_vocab`'s ban list.

Judgment calls, all deliberate. **`apply` was not made lane-aware.** It is the
rendered lane's command, running it is the explicit user action §Failure
semantics 3 requires a fallback render to be, and making the one command whose
job is static rendering skip kinds would be removing static rendering — which
the contract forbids in the same paragraph that defines the lanes. The flip is
therefore a change to what happens *automatically*, not to what happens when a
user asks for files. **The older per-project modes were not deleted**: they left
the wizard's front door and live behind "more control" and the shipped
`set-mode`, because deleting a working switch is not what "gone as a user-facing
concept" asks for. **A project that has already rendered keeps its render path**
in a scripted run — the files are a fact, and un-rendering stays the explicit
`set-mode` act rather than something a wizard does by omission. **The contract's
example line `2 skills re-pinned — live via gateway now` stays unprinted**,
revisited as W3 asked: the reason is no longer "static is the default" but that
`upgrade` performs a pinning act and cannot establish an activation one — the
bridge, the trust state, and the lease are all outside it, and Render locally
could send the same bytes to a file. Same boundary `package-members-v1` draws.

Honesty fixes that came with it: `Mode::ZeroFiles`'s "nothing on disk" and the
zero-files fork's "nothing is written to disk" were both bare-zero claims the
rules forbid; they now say *no generated files* and carry the sanctioned
sentence, which names the manifest, the lock, and any house-rules region.
`status` gained a `Delivery` block naming both lanes per CLI with the
zero-artifacts sentence and a separate `rendered lane:` line under it.

Tension named rather than guessed through: the shipped `set-mode` /
`doctor-mode-v1` vocabulary still calls `clean-at-rest` and `zero-files`
delivery *modes*, which the contract says are gone as user-facing concepts. The
routing and the modes are now two axes describing the same system, and the
product will read as two mechanisms until one of them is retired — a decision
larger than this workstream, and one with a `set-mode-v1` contract behind it.

### W4 (planner and the flip) — the arc lands

Shipped: a pure delivery planner (`crates/cli/src/delivery.rs`) taking two
inputs — capability kind and harness — plus the project's `[delivery]` table,
so it cannot reach anything else. Skills and servers route dynamic on
MCP-capable harnesses; instructions, settings, hooks and extensions route
rendered; every kind routes rendered on a non-MCP harness. `init` states the
routing per harness in plain language before it writes, and `status` shows both
lanes with the zero-artifacts sentence and a separate `rendered lane:` line.
The single override is `[delivery] render_locally` per project and
`[delivery.harness.<id>]` per harness, most specific winning in both
directions, reachable from the wizard's "more control" path and from
`agentstack delivery render-locally`; automatic is the *absence* of an
override, never a stored `false`. **The default flipped**, with all seven
remaining preconditions verified and witnessed rather than assumed — and
precondition 3 held only because the registry half found the unleased fence
open and closed it.

Witnesses: `delivery_planner` (5) — each kind routed to its lane including the
non-MCP harness case, a project in both lanes at once as the normal case, the
override writing files where the lease would have worked in both scopes and
both directions, the flipped default itself, and the honesty rules asserted
against real command output.

Judgment calls: `apply` was deliberately **not** made lane-aware — it is the
rendered lane's command, and running it is exactly the explicit user action the
contract requires a fallback render to be, so the flip changes what happens
automatically, not what happens when a user asks for files. The older delivery
modes were not deleted; they left the wizard's front door and remain behind
"more control" and the shipped `set-mode`, because deleting a working switch is
more than "gone as a user-facing concept" asks for. A project that has already
rendered keeps its render path in a scripted setup, since the files are a fact
and un-rendering stays an explicit act. The "live via gateway now" line stays
unprinted even post-flip, for a stronger reason than before: `upgrade` performs
a pinning act and cannot establish an activation one — bridge registration,
trust state and the lease are all outside it — so even a conditional print
would be a liveness claim from a pinning path.

**Tension recorded for the maintainer, not resolved here:** `set-mode` and
`doctor-mode-v1` still call clean-at-rest and zero-files delivery *modes*, so
`status` now shows `Mode static` directly above a `Delivery … served live`
block. Routing and modes are two axes over one system, and the product will
read as two mechanisms until one is retired. STRATEGY.md does say those
concepts disappear as user-facing ones, but retiring them means retiring a
shipped `set-mode-v1` ui-contract feature that a panel may depend on — a
maintainer call, not a build-loop one.

## Item 3 — library inversion (branch `library-inversion`)

Shipped: the library is now an ordered list of linked source folders, stored at
`~/.agentstack/sources.toml` — machine state, never project-visible, because a
repository able to add a source could aim resolution at a folder the user never
linked (invariant 3). A missing file is not an empty list; it is the single
implicit `local` source at `paths::lib_home()`, so an un-linked machine is
byte-identical to before, and `lib link` materializes that implicit entry the
first time a second folder is added so linking can never silently unlink the
central library. Precedence is `PATH` semantics — first match wins — with
`<source>:<name>` as the fully-qualified selector (`:` cannot occur in a
capability name, so the split needs no escaping). The qualifier is a *selector,
not an identity*: the lock key, rendered directory and gateway name are always
the bare name. Collisions are computed once at merge time and surfaced on `lib
sources`, `lib list`, `doctor`, `status` and `status --json` (gated on the new
`library-sources-v1`), naming the winner and the shadowed source and offering
the qualified pin. `lib link|unlink|sources|reorder` manage the list; a plain
non-git folder is first-class and `lib sync` refuses honestly rather than
implying misconfiguration. `init` now imports discovered MCP servers through
the one library write path into the first linked source, with the project
referencing them by name and `--project-servers` as the escape hatch.

Witnesses: `linked_sources` (7) — order resolution, a shadowed name reported
rather than hidden, a qualified reference ignoring order even after reordering,
a plain non-git folder working end to end, init importing into a linked folder
while the project stays clean, a single-library setup behaving exactly as
before, and the decisive one: **reordering sources cannot change what a locked
project serves** — the lock bytes are unchanged, the materialized body still
reads the original source's bytes, and re-activation fails closed naming the
drift instead of swapping.

The safety argument is structural rather than defensive: the source list is
read only during selection, and every serving path reads pinned bytes from the
content store by locked digest, so no serving path reads `sources.toml` at all.

**Flagged for line-by-line review** (reviewed here and judged sound, but it
touches trust granting): `init` now writes `agentstack.lock`, and the import
grant binds manifest **and** that lock. Library-first import made it necessary
— a name reference with no pin left the ordinary journey sitting at one warning
and cost two extra commands. The grant includes lock bytes only when this same
run wrote them, reads them back from disk so a later edit reads `Changed`, and
still withholds trust entirely for a lock that was merely lying on disk. Same
`trust_reviewed` constructor, fuller snapshot, no second grant path. Debts:
extensions and hooks get precedence and correct body roots but no qualified
reference spelling yet, since they are declared by manifest key rather than
selected from a reference list; `lib sources` shows "(folder not found)" for an
unpopulated central library, which is honest but may read as a fault on a first
run; and the pre-existing copy debt — `use`/`doctor`/`trust` describing a
moved-ahead library as "drift" in wording that can read as breakage — is now
slightly easier to reach, since reordering is a new route to that state.

## Item 4 — instructions target CLI and model (branch `instructions-variants`)

Shipped: `[[instructions.<name>.variant]]` — an ordered list of bodies selected
by `cli`, `model`, or both, resolved most-specific-first (exact `(cli, model)` >
`(cli)` > `(model)` > the base `path`) with identical selectors breaking to the
first declared, the same first-match rule linked sources use. `targets` keeps
its own job — *whether* a fragment reaches a CLI — and `variant` decides *which
bytes* once it does. The precedence function is a pure `select_variant` in core,
shared by the manifest's variants and a library body's, which are the same
grammar in two homes: a sourceless `[instructions.<name>]` now resolves through
the ordered linked sources to `<source>/instructions/<name>/instruction.toml`,
containment-checked before any body is read. Every declared body is pinned —
`[[instruction.variant]]` in the lock, from the same `Store::pin_instruction`
act, including variants nothing currently selects, because consent is over
content and not over what happened to be chosen today.

The model is never sniffed. It comes from an explicitly named toolset's `model`
or from `[settings.<cli>] model` (the value we compile into that CLI's own
config), and is otherwise **unknown** — a first-class answer that uses the least
specific matching body and says so, with the source of the model reported beside
every selection. Deliberately not read: a default toolset nobody named, trailing
`run` argv, and Claude Code's undocumented `SessionStart` `model` field.

The research the item rests on
(`docs/design/research/dynamic-instructions-2026-08.md`) disproved the delivery
contract's "MCP cannot inject these": the protocol has a purpose-built
`initialize` `instructions` field and our own gateway already uses it. **The
lane did not change and the justification did** — in `automatic-delivery.md`'s
own amendment style, in the planner (`Reason::NoModelAwareChannel`), and in
`docs/{concepts,choose,reference,ARCHITECTURE}.md`. Instructions stay rendered
for a reason that is a property of the channel, not caution: `initialize`
carries a client name and version and nothing about the model, so routing there
would force every fragment to the least specific match — defeating the feature —
and it fires before any lease names a toolset, which would put selected content
in agent context with no lease behind it (the fence W4 closed). Channel
confirmation is descriptor data, not a branch: `instructions.live` carries
`channel`/`display`/`confirmation`/`note`, confirmed for Claude Code and
unconfirmed for Codex, Copilot CLI, OpenCode and Junie, absent for Pi and for
the seven adapters with no instruction channel at all. `status` prints one row
per targeted harness — the file that carries house rules, the variant and why,
and the live channel's confirmation with "not used" attached in both states —
and `status --json` carries the same rows under the new `instruction-channels-v1`
feature, whose `used: false` is a field rather than an omission precisely so a
`confirmed` channel can never be read as a serving claim.

Witnesses: `instruction_variants` (6) — all four precedence levels plus a
deterministic tie, the unknown-model fallback asserted against real `status`
output, every variant pinned with a *deselected* one still failing closed at the
`ensure_instructions_compilable` gate, an unconfirmed channel never borrowing the
confirmed wording, the seven channel-less adapters named rather than omitted,
and variants resolving across two linked sources including a reorder flipping
which source's variant compiles.

Judgment calls and debts, named rather than guessed through: **instructions did
not move to the dynamic lane** (argued above; the day a channel is both
confirmed and model-aware, the descriptor gains it and that argument is the
checklist). A toolset's `model` is *intent* while `[settings.<cli>] model` is a
*fact we write*; the toolset wins as the narrower act and the reported source is
how a user sees that it did — AgentStack does **not** reconcile them, because
that would mean a toolset selection silently rewriting a harness's native
config. No qualified `<source>:<name>` spelling for a library house rule, the
same debt extensions and hooks carry: these kinds are declared by manifest key,
not selected from a reference list. A locked run's grant still binds the
fragment's **base** body path and digest; every variant is pinned in the lock and
strict verification compares all of them, so a drifted variant never reaches
grant construction — but the grant's own record is the base. Gemini CLI and
Cursor stayed channel-less: both have documented global-file channels, and
wiring them up means AgentStack starting to write two files it has never
written, which is an intake decision with its own consent surface. And a
vocabulary gap surfaced rather than papered over: the design docs say
`[toolsets.<name>]` while the manifest key is still `profiles` — the CLI's
`--toolset`/`--profile` alias is the same split, and it predates this item.
