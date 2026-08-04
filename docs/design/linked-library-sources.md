# Linked library sources: several folders, one ordered list

> **Status:** Adopted 2026-08-03, for queue item 3 ("Library inversion") of
> [`STRATEGY.md`](../../STRATEGY.md). That document's shape bullet — *"the
> library is linked folders — source-agnostic"* — is the requirement; this
> document fixes the model it left to be designed: where the link list lives,
> what a source folder must look like, how name collisions across sources are
> decided and shown, and why none of it can change what a locked project
> serves. It authorizes no work beyond that item.

## The model

There is no longer *the* library. There is an **ordered list of linked source
folders**, each an ordinary directory somewhere on the device. A folder becomes
a source because the user linked it, and stops being one because they unlinked
it; nothing else about the folder is special. Whether it is a git clone, a
synced drive, or a plain directory the user made this morning is the folder's
own business and never AgentStack's.

`~/.agentstack/lib` — today's central library — is simply the **first source on
a fresh machine**. A user who never links anything else has one source, sees
one library, and gets byte-for-byte the behaviour they had before.

## Where the link list lives

`~/.agentstack/sources.toml` (under `AGENTSTACK_HOME`), beside the other
machine state:

```toml
version = 1

[[source]]
name = "local"
path = "~/.agentstack/lib"

[[source]]
name = "team"
path = "~/work/team-capabilities"
```

File order **is** precedence order. Three properties make this the right home:

1. **It is machine state, not project state.** The list is a set of absolute
   paths on one device; it is meaningless in a repository and would not survive
   a clone. It belongs with `adapters/`, `backups/`, and the store.
2. **It must never be a project-visible surface.** A repository that could add
   a source could point the resolver at content the user never linked — the
   file-drop version of invariant 3. Nothing outside the personal layer is ever
   consulted for the list, and no manifest key can extend it.
3. **Absent means today's behaviour.** A missing file is not an error and not
   an empty list: it is the single implicit source `local` →
   [`paths::lib_home()`]. Linking a second folder is the only thing that ever
   materializes the file.

A source's `name` is validated by the ordinary capability name contract, and
must be unique in the list; a source's `path` is stored `~`-relative where it
can be and expanded on read.

## What a source folder holds

The clean folder taxonomy, exactly as `~/.agentstack/lib` already uses it —
`library.toml` as the index, plus `skills/<name>/`, `servers/<name>.toml`,
`instructions/`, `hooks/<name>.toml`, `extensions/<name>/`,
`workflows/`, `packages/<name>/`. Bodies are directories; single definitions
are files. A linked folder with no `library.toml` yet is empty, not broken.

## The precedence rule: `PATH` semantics

> **The first source that holds a capability of the requested kind and name
> wins.**

Ordered list, first match, no merging, no scoring, no "most specific". The
identity of this product is *feels like a filesystem*, and `PATH` is the
precedent every developer already carries: prepend a directory to shadow, list
the directories to see the order, and the resolution of any one name is
explainable by reading down the list. A ranking rule with more cleverness in it
(newest wins, most-specific wins, closest-version wins) buys nothing here and
costs the one thing the model has to have — a user being able to predict which
copy they get without running anything.

Two fences on the rule:

- **Inline still wins.** A project's own `[skills.<name>]` /
  `[servers.<name>]` entry beats every linked source, unchanged. The project is
  not a source in the list; it is the thing the list is being resolved *for*.
- **The identity of a capability is its bare name.** Precedence decides *which
  bytes* a name resolves to. It never renames anything: the lock key, the
  rendered directory, and the gateway's name for a capability are all the bare
  name, in every source.

## Being explicit: `<source>:<name>`

A project that does not want to depend on the order says so:

```toml
[toolsets.backend]
skills = ["team:sql-review", "pdf"]
```

`<source>:<name>` — the source's link name, a colon, the capability's name.
Chosen because it is the spelling already in every developer's hands from
`remote:branch` and `docker`'s `image:tag`, it reads left-to-right as
"narrower context, then thing", and `:` cannot occur in a capability name (the
name contract admits only `[a-z0-9._-]`), so the split is unambiguous with no
escaping rule.

Three things a qualified reference does:

- It resolves **only** in the named source. Not found there is an error naming
  the source, never a silent fall-through to the next one — falling through
  would make the explicit form weaker than the implicit one.
- It **ignores order**. Reordering, or linking a new source in front, cannot
  change what it resolves to.
- It **does not rename**. `team:sql-review` locks, renders, and serves as
  `sql-review`; the qualifier is a selector, not part of the identity. Two
  sources' same-named skills therefore cannot both be selected by one project —
  they are one name, and the project picks which bytes it means.

An unknown source name is refused at resolution with the list of linked
sources, rather than being read as a capability literally named with a colon.

## Collisions are surfaced, never hidden

Silent shadowing is the one failure mode this rule must not have. Every surface
that reads across sources therefore knows the losers as well as the winner:

- **`agentstack lib sources`** lists the order and, under it, a
  `Shadowed names` block — one row per name, of the shape
  `sql-review  skill  local used · team shadowed   ↳ team:sql-review`.
- **`agentstack lib list`** prints that same block under the merged listing, so
  the browsing surface cannot show a capability without also showing the copy
  it is hiding.
- **`agentstack doctor`** reports collisions as an advisory — a warning, not an
  error, because shadowing is legal and often deliberate — naming the winner,
  the shadowed sources, and the qualified reference that pins the other copy.
  Its `Library sources` section hides itself entirely on a machine with one
  source and no collisions.
- **`agentstack status`** carries one sentence per shadowed name, and
  `status --json` the same sentences under `shadowed_names` (ui-contract
  feature `library-sources-v1`). Both are absent when nothing collides — the
  orientation screen stays four ideas wide until there is something to say.

The advisory level is deliberate: a collision is an ambiguity the user may have
created on purpose, and progressive disclosure says a legal state does not get
an error. What it never gets is silence.

## Precedence cannot change what a locked project serves

This is the load-bearing guarantee, and it is not a new mechanism — it is
[`pinned-serving-and-library-drift.md`](pinned-serving-and-library-drift.md)
holding, unmodified, with more than one source in the picture.

Selection and serving are different acts on different days:

- **Selection** happens at lock time. Resolution walks the ordered sources,
  finds bytes, and `agentstack lock --write` writes their **digest** into
  `agentstack.lock`.
- **Serving** happens at load/render time and never re-walks the sources. Both
  the MCP load path and the rendered lane read the pinned bytes from the
  content-addressed store, addressed by the locked digest and re-verified
  against it before a byte reaches agent context or a harness file.

So relinking, reordering, unlinking, or a source moving ahead changes what the
*next* `lock` would select and changes nothing a locked project serves. When a
pinned name would now resolve to different bytes, that is the divergence
`pinned-serving-and-library-drift.md` already classifies: the serve path serves
the pin and notes an update is available; the consent and activation gates
(`use`, `trust`, `doctor`, `apply`) still call it drift and still refuse until
`agentstack lock --write` rewrites the lock — which flips the trust digest and re-gates
through the ordinary review card. Nothing about the source list can produce a
silent swap, because no serving path reads the source list at all.

## What `init` imports, and where it lands

`init` keeps its one importer (`detect_import` over each adapter's native
config). What changes is the destination: the MCP servers it finds are written
as **library server definitions in the first linked source** —
`<source>/servers/<name>.toml` plus an index entry — and the project manifest
references them **by name** from its default toolset. Adapter settings stay in
the project manifest, because they are that project's targeting decisions and
not reusable capabilities.

The result is the clean project the delivery arc asked for: a manifest that
says *which* capabilities this project uses, and a library that holds *what
they are*. It is also the honest reading of "import" — a server that was
already configured globally in three CLIs was never project-specific content.
`--project-servers` writes the old shape for a caller that wants the servers
inline.

One consequence follows and is deliberate: a manifest that references library
capabilities by name needs **pins** before anything can serve them, so the
library-first import writes `agentstack.lock` in the same transaction and the
import's trust grant binds to manifest **and** lock together. That is the
design law, not an extension of it — pinning is the machine's job, and the one
thing never automated is the yes. Without it the import would end on
"library server, not locked" and charge the user two commands for a decision
they never had to make. The grant's fail-closed edge is unchanged: a lock this
run did *not* write (reachable under `--force`) still withholds trust, because
it is content the import never showed anyone. Undo is unchanged too — the
library definitions, the index, and the lock are captured into the same
history entry as the manifest, so one `restore` reverses all of it.

## Quick capture still works

Library-first authoring is the default, not the only path. `agentstack lib new`
scaffolds into the first linked source and its follow-up line names the library
command first. But a file dropped in `.agentstack/skills/<name>/` is still
noticed by the funnel, still inert until declared, and `agentstack yes` still
takes it live as a project-local capability in one confirmation. Quick capture
exists precisely for the moment when deciding where something belongs would
cost more than writing it; `adopt --to-library` remains the one-flag promotion
afterwards.

## Invariant walk

Against `CLAUDE.md`'s non-negotiable invariants.

- **3 · Untrusted repository content is inert.** The source list lives only in
  the personal layer, so repository content cannot add a source, cannot point
  resolution at a folder the user did not link, and cannot make itself win a
  name. A linked folder is read at *selection* time only, and what it offers
  still passes the trust gate before it can reach agent context.
- **4 · Pinned byte changes re-gate.** Untouched, and re-stated above: the
  source list is not in the lock and not in the trust digest, because it cannot
  change what a locked project serves. A name that would now resolve to
  different bytes re-gates through `agentstack lock --write` exactly as a single
  library moving ahead already does. No cache and no partial-trust path is
  introduced.
- **5 · Secrets never serialize.** A linked source's `servers/<name>.toml`
  holds `${REF}` placeholders like the central library's always has;
  `sources.toml` holds names and paths and nothing else, and an unresolved
  `${REF}` still fails closed at activation.
- **7 · All repository content is hostile input.** A linked folder is
  somebody's directory, so it is parsed exactly as the central library already
  is: `library.toml` bounded and defensively parsed, entry names put through
  the name contract, bodies content-scanned at add time, paths containment-
  checked, and every string headed for a terminal sanitized. A source's own
  `name` and `path` pass the same gates before they are written to
  `sources.toml`. Nothing from a source is interpolated into a shell command.
- **8 · Claims match enforcement.** The collision advisory says which copy is
  used and which is shadowed rather than implying a merge; `lib sources` shows
  the order that actually decides; and no surface claims a linked folder is
  versioned or shared unless the user made it a git repo themselves.

## What this does not do

- **It does not make a source a trust boundary.** Linking a folder grants no
  consent to anything in it. Content from a linked source enters a project the
  same way it always has — declared, locked, and consented to per project.
- **It does not add sharing.** Sharing is access to a folder, solo-first;
  `lib sync` stays the productized git option for a source that is a git repo,
  and a plain folder is a first-class source with no warning implying
  otherwise.
- **It does not scope precedence per project.** The order is the user's, on
  their machine, for every project. A project that needs a different answer
  says so with a qualified reference, which is exactly the escape hatch a
  global ordering needs and the only one it needs.
