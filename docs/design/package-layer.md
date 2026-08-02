# The package layer: a compact reference that compiles to exact pins

> **Status:** Adopted 2026-08-03, during W5 of
> [`automatic-delivery.md`](automatic-delivery.md). That document is the
> contract; this one fixes the schema the contract left to be designed — the
> library package index, `packages = [...]` in a toolset, the lock expansion,
> and per-member project overrides. It does not amend the contract and
> authorizes no work beyond W5.

## What this settles

[`automatic-delivery.md`](automatic-delivery.md) §"Copy versus live reference,
settled" makes one claim and hands the schema to W5:

> A compact central package *reference* in the manifest is exactly as safe as
> vendored copying **iff the lock pins the expanded member set.**

Everything below exists to make the *iff* true — and to make the one thing a
compact reference can hide, per-member divergence, impossible to hide.

## The on-disk shape of a package in the library

A library package is a **directory body** at `<lib_home>/packages/<name>/`,
with a `pack.toml` at its root and member bodies at paths relative to that
root. `library.toml` gains a `[[package]]` index entry recording name,
version, checksum over `pack.toml`, provenance, and (for a git-sourced
package) url and rev.

Three reasons, in order of weight:

1. **It is the same artifact `pack.toml` already describes.** The git pack rail
   (`crates/cli/src/provider/gitpack.rs`) parses one optional server, `skill[]`,
   `instruction[]` and `targets[]`, applies the name-contract gate, and content
   scans every member. A library package reuses that parser verbatim, so there
   is one grammar, one hostile-input gate, and one scan — not a second package
   dialect that drifts from the first.
2. **The folder taxonomy already distinguishes bodies from definitions.** A
   library skill and a library extension are directories (`skills/<name>/`,
   `extensions/<name>/`); a library server and a library hook are single
   definition files (`servers/<name>.toml`, `hooks/<name>.toml`). A package has
   members, so it is a body: `packages/<name>/`.
3. **The index entry mirrors `LibraryExtension`.** Same
   `source`/`path`/`git`/`rev`/`checksum`/`version`/`provenance` fields, same
   `get_package` / `upsert_package` / `remove_package` accessors. A first-class
   kind, indexed exactly like the four that came before it — the contract asked
   for a first-class package index, not a parallel mechanism.

## `packages = [...]` in a toolset

```toml
[toolsets.backend]
packages = ["rust-backend"]
skills   = ["project-specific-review"]
servers  = ["project-database"]
```

`packages` sits beside `skills` and `servers` in `Profile` and selects by name
from the library package index. It carries **no wildcard**: `skills = ["*"]`
already means "every inline skill", and a `packages = ["*"]` would be a
default-broad activation of composed capability sets, which is the surprise the
existing wildcard rule deliberately avoids.

### Instruction-member semantics

A toolset today selects servers and skills and nothing else, while
`[instructions.*]` are manifest-global and pin regardless of any toolset. A
package can carry instruction members, so the semantics have to be stated:

- **Selection is toolset-scoped, at lock time.** Only packages named by a
  toolset the lock run selects are expanded. `agentstack lock --profile backend`
  expands `backend`'s packages; a bare `agentstack lock` expands every declared
  toolset's. A package nobody selects contributes no members and no pins.
- **A package's instruction members are rendered-lane, always.** They compile
  into the managed instruction region of an instruction file, exactly like a
  manifest-declared fragment; they are never served through the gateway. Each
  locked member therefore carries an explicit `lane` (`dynamic` for skills and
  servers, `rendered` for instructions), so the honesty rule of
  §"Mixed-lane upgrades" — *an instruction is never described as going live "via
  gateway"* — is carried by the data a surface reads, not by the care of each
  copywriter.
- **A package instruction member never becomes an `[instructions.*]` entry.**
  It is not the project's own fragment, and materializing it into the manifest
  would make removing the package leave orphaned declarations behind. It lives
  in the package's locked member set, with the package's provenance on it.

## The lock expansion

Locking a project whose toolset names a package writes one `[[package]]` entry
per selected package. This is the security core: the compact reference in the
manifest is safe **because** this expansion exists.

```toml
[[package]]
name = "rust-backend"                 # the name the toolset selected
version = "1.4.0"                     # the exact package version
source = "library:rust-backend"       # or "git:<url>@<tag>[#subdir]"
rev = "9f2c…"                         # exact revision; absent for a path body
toolsets = ["backend"]                # which toolsets selected it, sorted
removed = ["legacy-migration"]        # package members this project dropped

[[package.member]]
name = "sql-review"
kind = "skill"                        # skill | server | instruction
lane = "dynamic"                      # dynamic | rendered
origin = "package"                    # package | project-override
checksum = "<64 hex>"                 # per-member content digest
provenance = "package:rust-backend@1.4.0#skills/sql-review"
```

Field by field, and why each is load-bearing:

- `name` / `version` / `source` / `rev` — the exact package identity and
  revision the contract requires. `rev` is what makes "the same package, later"
  distinguishable from "the same package".
- `toolsets` — why this package is in the lock at all. Without it, removing one
  toolset's `packages` line leaves an entry nothing explains.
- `member.checksum` — the per-member content digest, produced by the **existing**
  pinning acts: `Store::pin` for a skill body (tree digest, deposits the bytes),
  `Store::pin_instruction` for an instruction fragment (file digest, deposits
  the bytes), and the server definition digest for a server. No second digest
  path exists, and none may be added.
- `member.provenance` — where these bytes came from, as a string a human reads:
  `package:<name>@<version>#<path-in-package>` for a package member,
  `project:<manifest-key>` for an overriding one.
- `member.origin` and `removed` — the effective member set, below.

Runtime resolves a package member from **these entries**, never from whatever
the library currently holds. That is the reproducibility rule of
[`automatic-delivery.md`](automatic-delivery.md) §"The reproducibility rule"
applied to packages, and it is the same rule
[`pinned-serving-and-library-drift.md`](pinned-serving-and-library-drift.md)
already applies to a single library skill: bytes are served from the
content-addressed store by digest, so a library that moves ahead changes nothing
in any project.

## Per-member overrides, as an effective member set

A project may diverge from a package on one member. The requirement is that the
divergence is **visible**, never silent.

```toml
[package_overrides.rust-backend]
remove = ["legacy-migration"]

[package_overrides.rust-backend.replace]
sql-review = "house-sql-review"
```

The smallest schema that expresses it: one table per package, a `remove` list of
package member names, and a `replace` map from a package member name to a
**project-declared capability of the same kind** — an ordinary `[skills.*]`,
`[servers.*]` or `[instructions.*]` entry. The replacement is not a second
declaration form; it is a name the project already declares, so it pins through
the paths that already pin project capabilities.

Fail-closed edges, all of them refusals at lock time:

- A `remove` or `replace` key naming a member the package does not carry —
  refused. A stale override that silently matches nothing is how a project
  believes it dropped something it still has.
- A `replace` target the project does not declare — refused.
- A `replace` target of a different kind than the member it replaces — refused.
  Swapping a skill for a server is not an override, it is a different
  composition.
- An override for a package no selected toolset names — refused, same reasoning
  as the stale key.

**Where the effective member set is visible.** In both places members are
listed:

- the lock's `[[package.member]]` entries, each tagged `origin = "package"` or
  `origin = "project-override"`, plus the package entry's `removed` list; and
- `status --json`, whose `project.packages[]` carries the same member rows with
  the same `origin` tags, the same `removed` list, and an `overrides` count —
  gated on the `package-members-v1` ui-contract feature.

A reader of either surface can always answer "which of these came from the
package, and which did this project change?" without diffing against the
package itself.

## The boundary at runtime: names and descriptions, never bodies

Selecting a package makes its **boundary** available, not its contents.
`agentstack_list_loadable` lists each member skill by name, one-line
description, `origin = "package"`, and the member's provenance string — read
from the lock and from the pinned bytes in the content store, never from the
library. A member's **body** enters agent context only when `agentstack_load`
is called for that one member, and only after the store snapshot is re-verified
against the digest the lock pins; a missing or tampered snapshot refuses and
names `agentstack lock`. There is no path from the loader back to the package
body in the library.

Two consequences worth stating, because both are the point:

- **A package's instruction members are not loadable.** The loadable index is
  skill-only; an instruction is rendered-lane and reaches a file, not a
  context window.
- **Twenty members cost twenty lines, not twenty bodies.** Eagerly injecting
  the bodies because a package was selected would recreate the context bloat
  this lane exists to remove, under a new name
  ([`automatic-delivery.md`](automatic-delivery.md) §"Boundary, not bodies").

Server members are **lazy on the same principle**: a stdio child is spawned,
and an HTTP upstream is dialled, on the first message that needs one — not when
a toolset is selected and not when the served server set is read. The one path
that contacts every upstream at once is *aggregated tool discovery*
(transparent mode's `tools/list`, `tools_search`, code-mode bindings), because
enumerating tools nobody has cached means asking each server for them. The
default compact surface never does it.

## A package's server member, served

A `[[package.member]]` of kind `server` **is** a gateway upstream, on exactly
the same terms as a manifest-declared one. It joins the served set at one
place — the point in `Gateway::build` where the manifest/library resolution has
just finished — as the same `FrozenServer` entry that resolution produces, so
there is one upstream set, one transport path and one dispatch path
(`CLAUDE.md` invariant 6). Nothing downstream distinguishes it: the compiled
tool firewall folds over its name with all the others, its `${REF}`s resolve at
the one secret-resolution site, and a call to it passes the trust-at-dispatch
gate that fronts every tool call.

Four properties, each load-bearing:

- **The toolset is the whole admission decision.** A package's server is
  reachable under a toolset that selected the package, and nowhere else — the
  same fence, computed the same way, that decides which skill members are
  loadable. A fence naming a toolset that is gone still fences to *nothing*
  rather than widening to everything. And the **unfenced** gateway contributes
  no package servers at all: unfenced already means every server the manifest
  declares anywhere, so unioning every package's server on top would make
  package membership a way to widen the surface rather than something a toolset
  selects.
- **The definition comes from the lock and the content store.** The member's
  pinned checksum addresses a deposit of the exact definition bytes it covers,
  re-verified against that digest at read time. The package's current
  `pack.toml` is never opened at serving time — it is precisely the mutable
  thing the pin exists to stop reading, and a serving path that re-read it
  would run a command nobody reviewed. This is
  [`pinned-serving-and-library-drift.md`](pinned-serving-and-library-drift.md)
  §"The rendered lane" applied to the one pin family that previously had a
  digest but no deposit.
- **Unverifiable fails closed, and says so.** A member whose definition is
  absent from the store, no longer hashes to its pin, or no longer parses is
  not served, and the refusal takes the existing seatbelt path a drifted
  library pin takes: it names the capability, the reason and `agentstack lock`,
  lands in the audit log, and puts the member on the gateway's skipped list. A
  name the project already serves from its manifest or the library is refused
  the same way rather than shadowing it — two upstreams under one name have no
  unambiguous dispatch.
- **Lazy, like every other upstream.** Selecting the toolset builds the served
  surface; reading which servers are proxied names them. Neither starts a stdio
  child or dials an HTTP endpoint. Only calling one of its tools does.

One boundary is deliberately not crossed: a **sandboxed or lockdown run**
(`Gateway::from_frozen`) consumes the frozen server set its execution plan
already classified, and that set is still built from `[servers.*]` and a
toolset's `servers` list alone. Package-carried servers therefore reach the
live host gateway (session, lease) and not a sandboxed run. Extending them into
a run requires giving the run-grant artifact a binding for a package-carried
definition — a change to authority construction, which is a decision to take
deliberately rather than a wiring detail to add here.

## The rendered-lane compile of an instruction member

A package's instruction members compile into the managed instruction region
through `render::instructions::plan_instructions` — the same compiler, the same
`merge_md` region contract, and the same file the manifest's own
`[instructions.*]` fragments reach. Package members compile **after** the
project's own fragments, so a project's prose always has the last word in the
region, and each is labelled `<package>:<member>` wherever fragments are
reported.

Four properties, each load-bearing:

- **Pinned bytes, always.** The prose comes from the content store, addressed
  by the member's digest and re-verified against it, exactly like a keep-pinned
  fragment. A library that moves ahead rewrites nobody's `CLAUDE.md`; taking a
  newer version means running `agentstack lock`.
- **Still not an `[instructions.*]` entry.** Nothing is materialized into the
  manifest, so dropping the package leaves no orphaned declaration behind.
- **Conservative scoping, on the automatic path.** `agentstack lock` — the
  command that re-pins members — refreshes the managed region only in a file
  that **already carries one**. It never creates an instruction file, and never
  adds a region to a file that had none: that is a decision a human makes by
  running `apply` or `instructions --write`. This is W3's precedent
  (`commands/upgrade.rs`), applied verbatim rather than re-derived.
- **The report names the lane and the file.** `lock` prints a separate
  `rendered lane:` line saying what was written and where — or, when no target
  carries a region, that **no file was written**, plus the one command that
  would render it. A member whose pinned bytes fail verification is excluded
  and named rather than silently dropped from the region. No surface describes
  an instruction as going live "via gateway", because it does not.

## Deferred by name: hooks and extensions

A package carrying hook or extension members is **refused**, by name, with a
message saying package-carried executable kinds are not supported in v1 and
naming what to do instead (declare them in the project manifest, where the full
consent ceremony applies).

This is not an omission to be helpful about later in a patch. Hooks and
extensions run commands at user permission; `CLAUDE.md`'s standing
classification puts hooks alongside extensions as an executable capability kind
for which **the full consent ceremony always applies, and no compressed-consent
path may ever cover them**. A package reference is, by construction, a
compressed consent path: one name in a toolset stands for a set of members. The
two cannot meet, so the fence is a permanent property of the v1 schema and not a
TODO.

The refusal is at the parse gate, beside the existing name-contract gate, so it
covers a library package and a git-installed pack identically. Before this,
`pack.toml` parsing simply *dropped* unknown arrays — a hook member would have
been silently ignored, which is the worse of the two failures.

## Invariant walk

Against `CLAUDE.md`'s non-negotiable invariants.

- **3 · Untrusted repository content is inert.** A package reference resolves no
  bodies and contacts nothing before the trust gate. Member names, paths and the
  package's own name pass the same name-contract gate (`text::validate_name`)
  and containment check (`..`/absolute refused, must stay inside the package)
  that the git pack rail already applies, and every member is content-scanned at
  intake. Nothing a package declares enters agent context until the project's
  lock — which the trust digest covers — has been consented to.
- **4 · Pinned byte changes re-gate.** Every member carries its own digest in
  the lock, so any member's bytes moving means different lock bytes, which flips
  the trust digest and re-gates through the ordinary review card. No cache and no
  partial-trust path is introduced: nothing accepts a package on the strength of
  the package's identity alone, and there is no "the version is unchanged, skip
  the members" fast path. A package whose version is unchanged but whose member
  bytes moved re-gates exactly like any other content change, because the
  comparison is over the members, not the version string.
- **5 · Secrets never serialize.** A package's server member locks its
  **definition** digest, computed over the `${REF}`-only server table — the same
  thing `LockedServer` has always pinned. No resolved value reaches the lock, the
  index, or `status --json`, and an unresolved `${REF}` fails closed at
  activation as before.
- **7 · All repository content is hostile input.** `pack.toml` is remote content:
  it is read bounded, parsed defensively, name-gated all-or-nothing (one bad name
  rejects the whole package, matching the atomic-install semantics), its member
  paths are containment-checked before any read, and every string that reaches a
  terminal or a JSON consumer — package name, version, provenance — passes
  `text::sanitize_line` first. Nothing from a package is interpolated into a
  shell command.
- **8 · Claims match enforcement.** The per-member `lane` field exists so no
  surface can describe a rendered instruction as served through the gateway. The
  `removed` list exists so no surface can present an overridden composition as
  the package's own.

## What this does not do

Named so nobody reads it in. The four consumers this document originally
deferred — the boundary without bodies, lazy server start, the rendered-lane
compile of an instruction member, and serving a package's server member — have
all landed and are described above. One thing remains untrue of a package, and
it is not a gap in the pins:

- **Package members carry no standing re-gate answers.** `Blocked` and
  `KeepPinned` decisions are keyed to manifest and library capabilities; the
  trust walk does not enumerate package members, so a member cannot be
  individually blocked or kept-pinned. It is not a hole in content binding —
  every member's digest is in the lock, and the lock is in the trust digest, so
  a member's bytes moving re-gates the whole project through the ordinary card.
  It is a granularity the consent surface does not offer yet.

It is not a reason to read the lock as incomplete: the pins are exactly what
the acceptance criterion asks for, and every consumer above adds a reader,
never a second source of truth.
