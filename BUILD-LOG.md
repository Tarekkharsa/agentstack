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

## Item 5 — surface finish (branch `surface-finish`)

Three separable changes, one theme: the shape's finish, not new capability.

**The grouped review card.** The detail body is now grouped per capability with
change markers, on both renderers, and the two renderers still are not merged —
`docs/archive/design/consent-card.md` §Panel gives three reasons they must not
be, and all three still hold. In the terminal, each kind's items sit under a
header that carries the group's own tally (`+2 added, ~1 changed, 3 unchanged,
-1 removed`), folded from the SAME markers `ReviewDiff` handed the per-item
lines — a new index-aligned `marks` vector beside `current`, deliberately not a
field on `SurfaceItem`, which is a `trust`-crate serde record consent is bound
to and has no business carrying presentation. Servers gained the header they
never had; `secrets` and `policy` deliberately did not, because each is a single
aggregate line whose own `+`/`~` already IS its group's marker. In the payload,
a new `trust-card-groups-v1` carries `review.groups` — and the load-bearing
decision is that **a group holds INDICES into `review.items`, never copies**.
That is what makes "grouping is presentation, not granularity" structural rather
than a promise: a group has nowhere to put a fact of its own, so it has nowhere
to put a decision either. `review.question` carries the one closing question,
and there is exactly one `question` key in the whole payload — asserted by
walking every key path, because absence is the kind of property that stops being
true quietly. `trust-card-diff-v1`'s item shape is asserted EXACTLY, key set and
order, so the addition cannot have moved anything a panel already renders.
Delivery routing stays informational and unanswerable, as item 2's planner left
it.

**`run` is protected by default.** A bare `agentstack run <cli>` now takes the
fail-closed path `--locked` used to opt into; `--unprotected` is the explicit
way out, keeping its unchanged `HOST / ADVISORY` label and gaining a banner that
names what it turned off. The routing order is the whole design: `--locked`
first (so an explicit invocation reaches the identical refusals it always did,
including the `--locked --sandbox` not-yet limitation), then `--sandbox` /
`--lockdown` (so the isolation opt-ins mean exactly what they meant), then the
protected default, with `--unprotected` last. Only the *unflagged* run moved,
and it moved fail-closed. `--locked --unprotected` refuses rather than letting
flag order decide. `--prompt` now keys on "the protected run" instead of on the
flag name. Posture labels are untouched — `HOST / PROTECTED`,
`SANDBOX / PROXIED · DIRECT ROUTE OPEN`, `LOCKDOWN / ENFORCED · NO DIRECT ROUTE`
— and the protected run is still not kernel isolation, still says so.

**Varlock productization, surfacing only.** `VarlockResolver::detect` and a new
`varlock::health` now go through ONE `load`, so what `doctor` reports and what
the chain does can never be two answers; the chain's order, `${REF}` semantics,
and fail-closed resolution are byte-identical. `init` offers a `.env.schema`
next to the manifest when it has names to declare — declined silently when
non-interactive, never overwriting an existing schema, folded into init's one
undoable transaction, and carrying NAMES with empty values only. `doctor`
reports varlock health inside the existing **Secrets** section rather than as a
new check family: `Info` when the project has not opted in (a recommendation
never becomes the one next action), `Warn` for the silent-degradation case that
motivated it (schema present, binary not runnable, every ref quietly falling
through), `Warn` on a failed load, `Unchecked` over an empty schema so no green
line claims a pass over nothing.

Witnesses: `surface_finish` (7) — grouping present with change markers and
exactly one question; no per-capability answer affordance anywhere in the
payload; the shipped item shape byte-for-byte; the new default with its opt-out,
its contradiction refusal, `--locked`'s unchanged routing, a bare `--plan`
becoming the protected plan, and both isolation posture labels; an untrusted and
a drifted project each refused with a runnable harness binary sitting right
there unrun, against the same project running under `--unprotected`; init's
offer and doctor's three varlock health readings; and the mechanism guard — an
unresolved `${REF}` still blocks the write, with and without a `.env.schema`.
Plus one unit witness that the schema builder can never emit a value.

Judgment calls and debts, named rather than guessed through: **the opt-out is
spelled `--unprotected`**, not `--host` (the protected run is a host run too) and
not `--no-locked` (a negation of a flag that is now the default reads as a
double negative) — it names what you give up, which is the honest thing for a
fail-open escape hatch. **`--locked` was kept, not deprecated**: it is in
docs, examples, the workflow child constructor, and t3code's vocabulary, and
retiring it is a separate decision. **`.env.schema` goes next to the manifest**
(`.agentstack/`), because that is the directory `Chain::default_for_dir` is
handed — the docs' looser "drop a `.env.schema` in the project" was already
imprecise, and making `health` look in two places would have been the one thing
that lets doctor and the chain disagree. **The `.env.schema` body's decorator
lines** (`@defaultSensitive`, `@defaultRequired`, the `# ---` divider) are
varlock's documented schema shape but are not verified against a live varlock
in CI — the whole body is one small function, `env_schema_body`, so a correction
is one edit. And the ordering fact found while witnessing the flip: the
protected run resolves the harness binary BEFORE the trust gate, so a project
that is both untrusted and missing its CLI reads as "not on your PATH" rather
than as a trust refusal. It still fails closed either way; the message is just
about the wrong thing first. Not fixed here — reordering that resolution is a
behaviour change of its own — but the witness pins a real binary in place so it
proves the gate rather than the PATH.

**Post-item note (found in item 5, caused by items 2 and 3):** `tools/check-structure.py`
was failing on two undeclared manifest fields, `package_overrides` and
`delivery`. Both are configuration rather than capability kinds — one reshapes
selection of an existing kind's members, the other steers how already-declared
capabilities are delivered — so both were declared in `CONFIG_ALLOWLIST`. The
lint exists to force exactly this decision deliberately rather than letting a
new field drift in unclassified; it now reports zero findings.

## Item 6 — workflows promotion (branch `workflows-promotion`)

Shipped: `Profile.effort` beside item 4's `Profile.model`, both carried onto
each `RoleBinding` at admission and spliced into the harness launch as argv
fragments ahead of the `--` guard, in fixed order, so argv and the grant digest
stay a function of inputs alone — and never written into any harness's
persistent settings file. An adapter that cannot take a value reports which of
two distinct things is true: it has no notion of that dimension at all, or it
has the setting but no confirmed way to select it for a single headless launch
(Claude Code's `effortLevel`). Both warn per child and the run proceeds; a
value the adapter's own catalog rejects is a manifest error that refuses the
child before launch. Five named algorithm helpers joined the prelude —
`mapReduce`, `reduceByKey`, `combine`, `verify`, `keepUnrefuted` — and the
argument that none can widen a role is structural: **not one calls `agent()`**,
so a run happens only when the caller's own callback asks for it, through the
same bridge a hand-written script uses, leaving the role-admission check and
`max_agents` the sole authority path.

All six open security-review findings closed with named witnesses: the
watchdog's exit is now armed ahead of all four blocking reporting operations;
interpreter memory is bounded at every untrusted ingress via
`HostHooks::max_buffer_size` (64 MiB against Boa's 1.5 GiB default); every
native enters through one guard whose RAII release covers `?` and unwind paths;
a **run-total** native-call budget exists because Boa's own loop limit lives on
the `CallFrame` and therefore bounds one frame rather than the run;
locale-sensitive APIs are poisoned non-configurable so a replayed journal
cannot re-derive a different ordering on another host; and `boa_engine` is
test-confined to the `workflow` crate, which itself takes no agentstack
dependency.

Two witnesses were verified by construction rather than assertion. Disabling
the re-entrancy guard turned its witness **red on the right thing** — the
getter's nested `agent()` produced a genuine second spawn request, ordered
first, because the getter runs during the outer call's own argument conversion.
Adding `boa_engine` to another crate's manifest turned the boundary witness red
with the right message. Both were restored and re-verified green.

**The command tree is un-hidden.** Per the ruling, the six findings were the
gate; the repeated-use criterion predates the 2026-08-02 reset and was rewritten
in `docs/workflows.md` as an honest maturity note — 1 of 3 occasions, the
2026-07-23 acceptance run — explicitly a signal to weigh rather than a gate
anything waits on. Every *Honest limits* claim survives verbatim, and the new
copy states in three places that un-hiding moved discoverability only, not
enforcement, and that a host-tier step is still cooperative-guard only. Both
partial bounds are stated where the claims are made, guarded by a witness that
fails if either residual disappears from `POSTURE_LABEL` or the docs: a trusted,
reviewed script can still allocate on purpose, and Boa's own built-ins tick no
counter at any setting.

Judgment calls and debts: `cli.rs` justified hiding the tree "until the
repeated-use gate in `TODO.md` closes", but post-reset `TODO.md` contains no
such gate — the code cited a source that no longer said what it claimed, so the
citation was removed rather than preserved as a dangling reference.
`docs/workflows.md`'s own worked example had always called `keepUnrefuted`,
which never existed in the prelude, so anyone who copied it got a
`ReferenceError`; that is closed by shipping the helper and saying so, not by
editing the example to hide it. `selection_for`'s doc comment claimed every
surface routes through it, which is false — a workflow child runs quiet, so the
drive loop is the surface that speaks per child — and was corrected.

## Item 7 — packaging (branch `packaging`)

Shipped: `agentstack image` — one toolset and its lock-pinned members composed
into a container image the user builds locally and runs themselves. The
artifact format was the open decision and is written up first in
`docs/design/packaging.md`: a runner base image plus one added layer under
`/agentstack` carrying the descriptor, the manifest and lock, each selected
server's **definition** with its `${REF}` placeholders verbatim, package
instruction members, a fixed secret guard, and the toolset's skill bodies laid
down in the harness's own skills directory under an image-owned `HOME`. The
reuse argument is the whole design: the default `FROM` is the same
`AGENTSTACK_SANDBOX_IMAGE` value `run --sandbox` would have launched and the
`WORKDIR` is the same `/workspace` mount point, so the built image is a
drop-in for that variable and every network, mount, proxy, sidecar and
recorder mechanism stays exactly where it already is. Nothing is pushed,
tagged remotely, signed, or registered — the hosted non-goal is untouched.
Members come from the lock and their bytes from the content store by digest
(never the live library), through the same `frozen_runtime_servers` set a
sandbox run is assembled from and the same `render::skills` materialization
seam `use --write` runs, forced to Copy because a symlink into the host store
is a dangling path inside an image. `crates/runtime/src/image.rs` is the new
seam: an `ImageSpec` beside `SandboxSpec`, rendering the Dockerfile from
charset-validated values in JSON exec form and invoking `docker` as argv —
never a shell, no new dependency, no unsafe.

**Secrets, the sharp constraint.** The build path constructs no resolver at
all; server definitions travel as `ResolvedServer::server` promises them, and
only the `${REF}` NAMES reach the artifact. The witness plants the same value
three ways the chain can see (process env, `.agentstack/.env`, `.env`), then
walks every byte the build writes and finds none of it, while requiring the
placeholder and the name to be present — so the absence is honesty rather than
a dropped server. A fixed entrypoint script (a compile-time constant with
nothing interpolated into it) reads the names and refuses with `exit 78`
unless the run's own environment supplies them, using `printenv "$ref"` rather
than `eval` so a file-derived name is an argument and never a program
fragment.

**The posture label is the shipped `Posture::Sandbox` and nothing stronger**,
printed in its shipped spelling with the caveat attached in the same breath:
posture is a property of the run, not the image, so a bare `docker run` earns
the container boundary and nothing else, and `--lockdown` is deliberately not
claimed because topological confinement comes from the internal network and
the sidecar, neither of which an image contains. `ENFORCEMENT.md` gained a
§"Packaged images" section saying the same thing, so the authoritative matrix
and the design doc cannot drift. Reproducibility is claimed only for the
AgentStack layer (content-addressed, verifiable member by member from
`image.json`) and explicitly **not** for the image: a Docker build is not
bit-reproducible and the base is a floating tag unless `--from` names a digest.

Witnesses: `packaging` (6) — the plan naming every pinned member with the
lock's own digest and nothing else; the secret witness above; the two
fail-closed shapes (no lock entry, and a store deposit that no longer hashes
to its own name) each refusing *before* a context directory exists; the
posture label asserted against `Posture::from_slug` and `GrantPosture` rather
than a string typed in the test, with ENFORCED asserted absent; honest
degradation with no daemon, distinguishing a missing client from a stopped one
and handing over the exact `docker build` line against a context proven
complete file by file; plus a Docker-gated end-to-end that builds a real image
`FROM alpine:3`, reads its labels back, and watches the guard refuse to start
without its secret and step aside with it. `AGENTSTACK_DOCKER` exists so the
daemon-absent branch is deterministic on a machine that has Docker.

Judgment calls and limits, named rather than guessed through. **`--write`
stages the context even when Docker is missing**, then errors: the artifact is
still finishable by hand, and losing the staging work to a stopped daemon
would be a worse failure than a non-zero exit. **A dry run over an unbuildable
plan prints the whole plan and then exits non-zero**, because a plan you cannot
read is worse than one that refuses. **The build requires trust** (invariant 3:
baking skill bytes into an image puts them where an agent reads them) while the
plan does not, so diagnosis stays available. **Server definitions are carried,
never rendered into native config** — the one existing path that renders them
resolves `${REF}` through a `ScopedResolver` and writes concrete values, which
is right for a local machine and categorically wrong for a distributable
artifact. **`[instructions.*]` fragments are not compiled into the image**: the
only shipped global-scope compile merges into the *builder's own* instruction
file, and shipping that merge would put a person's private notes into a
distributable artifact; package instruction members are carried and named as
carried. **The build backend shells out to `docker` while runs keep bollard** —
the daemon's build endpoint wants a tar stream this workspace has no writer
for, and bollard is behind an opt-in feature that is off in default builds and
CI, which a headline capability cannot depend on. Tension recorded for the
maintainer: the word *package* now names two things — a library composition
(`docs/design/package-layer.md`) and this artifact — and the command is spelled
`image` to keep them apart, but the strategy's own sentence says "packaging",
so one of the two nouns will eventually want renaming.

**Run incident (item 7):** the packaging agent ran `rm -rf .scratch` at the repo
root, mistaking the maintainer's untracked strategy-v3 decision records for its
own temporary output. All 15 files were recovered intact from a dangling git
object — the pre-amend commit `70aed8a`, created when an earlier `git add -A`
had briefly and wrongly staged `.scratch/` before it was amended out. Two
mistakes cancelling is luck, not process. `.scratch/` remains untracked, as the
maintainer had it.

## Item 8 — the panel (branch `panel-surfaces`)

**Mostly verification, plus one gap and two wrong addresses.** Items 1–7 had
already shipped the reads behind all four surfaces the queue names, so this
item's honest job was to check them from a panel's side rather than to add
capability, and the audit says: lease status, the grouped review card, and
library sources were already served correctly and needed nothing. Two of the
three had the wrong address written down, though — `library-sources-v1` and
`instruction-channels-v1` both said "`status --json` gains …" when the arrays
are on `status --json`'s **`project`** object, so a panel following the feature
docs would have looked one level too high and concluded the binary predated the
contract. Both doc-comments now name the real path (and this was caught by the
witness failing, not by reading).

The one real gap was **workflow control**. Item 6 shipped per-role model and
effort, but the only surface carrying them was `workflow explain --json`, which
(a) emitted a bare body with no `schema_version`/`features`, so a panel could
read the richest workflow payload the CLI has and never *negotiate* it, and
(b) re-gates on trust, because parsing an untrusted bundle's script is what
rule 3 forbids — so on the state a panel usually meets a project in, the
model/effort story was simply unavailable. Closed as `workflow-role-selection-v1`:
`workflow list --json` rows gain `role_details[]` (`role`, `harness`, `model`,
`effort`, `serial`, `undeliverable[]`) from the SAME `role_selection` walk
`explain` renders and the same authority the launch path asks — the bound
adapter's descriptor — and `explain --json` gained the envelope. `list` is the
refusal-free surface, so the tree answers untrusted. Deliberately NOT
index-aligned with `roles`: a role with no declared toolset contributes no
entry at all, because a fabricated `model: null` beside it would read as
"declares no model" when the truth is "nothing could be established".

**No new fixed verb, and that is the finding, not an omission.** Every action
the four surfaces would want turns out to need an authority path the read
surface does not have. Leases are opened and closed by the MCP connection that
owns them — `lease-status-v1` says outright there is no action on the contract,
and a panel-driven open would be a second lease owner with no process to die
with. Linked library sources are personal-layer machine state (`lib link` /
`reorder`): a project-scoped panel able to re-point resolution at a folder the
user never linked is invariant 3 with extra steps. Running or resuming a
workflow stays deferred per the UI control-plane's own §Deferred. And no
per-item consent answer was added anywhere, per `consent-card.md` §Panel —
the card still has exactly one question and one answer, over the whole project.

What *was* added on the action side is a declaration, not a capability:
`ui_contract::PANEL_ACTIONS` names the closed set (22 entries) in the CLI
rather than only in the panel's TypeScript, so "the panel is never a second
authority" became a property this repository can enumerate. Nothing reads it at
runtime — same rule as `FEATURES`.

Witnesses: `panel_surfaces` (6) — all four surfaces read end to end through the
real binary on the fixed argv a panel emits (not in-process helpers), each
negotiable by name and coherent with the others; a stale and an unknown lease
row never rendering live in the panel payload, each carrying the `why` sentence
so no UI has to infer liveness from `pid` itself; the grouped card reaching the
panel with exactly one `question` — asserted by walking every key path in the
payload, plus the absence of any accept/keep-pinned/block spelling, plus every
item appearing in exactly one group by index; the closed set checked verb by
verb against the clap tree with each digest flag proven to exist on the verb
that claims it; a stale consent digest refused with the manifest byte-identical
after; and the second-authority witness in two halves — a source scan proving
no code outside `ui_contract.rs` reads `FEATURES` or `PANEL_ACTIONS`, and a
project advertising every contract this binary serves still refused activation
on its own trust gate.

Judgment calls and tensions, named rather than guessed through. **The queue's
phrasing "every action … bound to a `consent_digest`" is not what the shipped
closed set is**, and the table says so instead of pretending: thirteen actions bind
a digest, and nine do not (`apply` and `adopt` in both scopes, `guard install`,
`trust --revoke`, `restore <id> --write`, `session start` / `end`) — they introduce
no new content for a human to review — they render, adopt, revert, or withdraw.
Withdrawing a yes is the sharpest case: binding a revoke to a reviewed preview
would make failing-to-revoke the easy path, which is the wrong direction to
fail. `Consent::Preconditions` is the honest name for that half, and what makes
those safe is the CLI's own gates, witnessed elsewhere. **`docs/automation.md`
was three items behind** — none of `lease-status-v1`, `delivery-routing-v1`,
`image-plan-v1`, `package-members-v1`, `library-sources-v1`,
`instruction-channels-v1`, `needs-your-yes-v1`, `update-offer-v1`,
`trust-card-diff-v1` or `trust-card-groups-v1` appeared on the integrator's
table, which is a read-surface gap even though every payload worked; all are
now listed with their limits, and `workflow explain --json` moved off the
"not part of this contract" list. One honesty fix fell out of the same pass:
that page promised its reads "start no process", while `lease status` runs
`/bin/ps` per recorded PID on macOS because there is no `/proc` — now named in
the row and in the section preamble, beside `doctor --probe`. **The two card
renderers were not merged** and were not touched; the three reasons in
`consent-card.md` §Panel still hold.
