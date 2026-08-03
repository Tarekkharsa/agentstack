# Pinned serving, and what a library moving ahead means

> **Status:** Adopted 2026-08-02, during W3 of
> [`automatic-delivery.md`](automatic-delivery.md). That document is the
> contract; this one settles a single question it left readable two ways, and
> fixes the scope of the answer. It does not amend the contract and does not
> authorize work beyond W3.

## The question

W3 makes runtime serve a skill's **pinned** bytes from the content-addressed
store. But the MCP load path re-resolves the *live* library first, and for a
library-sourced skill that resolution reads exactly the directory `lib sync`
rewrites. After a sync pulls new bytes for a skill a project pins,
`classify_skill` reports `ChecksumDrift` and `skill_verdict` returns `Block` —
so the load is refused before the serve point is ever reached.

Is that divergence **project drift** (refuse, as with any other drift), or an
**update available** (serve the pin, mention the newer version)?

## The two readings, and the lines that settle it

The reading that refuses comes from §"Failure semantics" 2:

> Drift in any member — dynamic or rendered — marks the project **Changed**,
> and Changed blocks new leases and new loads at the choke points we control.

The reading that serves comes from §"The reproducibility rule":

> the library can move ahead arbitrarily far without changing, breaking, or
> interrupting any project, because no project ever reads it at serving time

and from §"Update model" rule 1:

> **`lib sync` announces; it never re-gates and never interrupts.** Pulling new
> library versions changes nothing in any project.

They only conflict if "member" means "whatever the resolver currently finds".
It does not. The consent unit is the project's **composition** — what its
manifest declares and its lock pins. A library moving ahead changes neither, so
the project's consent digest is untouched and the project is not `Changed`.
Rule 1 then says what must happen: nothing, in any project. Refusing the load
would be `lib sync` interrupting a project, which is the exact behaviour the
rule names and forbids.

## The decision

For a **library-sourced** skill that **carries a lock pin**, checksum
divergence between the live library directory and the pin is an *update
available*. The load serves the pinned bytes from the store and carries a
one-line note naming that a newer version exists in the library and the one
command that takes it (`agentstack lock`). Per §"Update model" rule 4,
keep-pinned is the resting state: the note offers, and must never imply the
project is broken or stale.

## Scope fence

Narrow on purpose; everything outside it fails closed exactly as before.

- **Only the MCP load/serve path** (`load_capability_with_lease`) treats the
  divergence as an *update available*. `use`, `doctor`, `trust`, `apply` and
  every other gate still classify it as drift and still refuse. (The separate
  question of *where pinned bytes are read from* is not fenced this way — see
  [The rendered lane](#the-rendered-lane).)
- **Only library-sourced skills.** An inline (project-local) skill whose bytes
  changed is the project's own content changing — that is project drift and
  keeps refusing. The distinction is the resolver's `SkillOrigin`, decided by
  where the reference was satisfied (an inline `[skills.<name>]` block wins
  over the library index), never inferred at the gate.
- **Only checksum drift.** Rev drift, an uncached git source offline, a broken
  ref, a standing `Blocked` decision, and keep-pinned delivery all behave
  exactly as they did.
- **Only when the pinned snapshot is already in the store and still hashes to
  its own name.** No pin, a store miss, a tampered snapshot: today's refusal.
  The live directory is the thing that no longer matches the pin, so it is
  never a source the store may be repaired from here.

## Why this is not a weakening

- **Invariant 3 (untrusted content is inert)** is *strengthened*. The loader
  stops reading the mutable library at serving time altogether; what reaches
  agent context is the verified snapshot of the bytes a human approved.
- **Invariant 4 (pinned byte changes re-gate)** still holds. It binds what the
  **lock** pins, and the lock is unchanged by a sync. Taking the newer version
  means running `agentstack lock`, which rewrites lock bytes, which flips the
  trust digest, which re-gates through the ordinary review card. No cache and
  no partial-trust path is introduced: the store lookup is content-addressed
  and re-verified at read time.
- Nothing unreviewed becomes reachable, because nothing new is served — the
  bytes served are the same ones served before the sync.

## The rendered lane

Serving pinned bytes from the store is a property of *reading*, and the MCP
load path is not the only reader. `use --write` materializes a project's skills
into each target's skills directory, and `render::skills` points that artifact —
a symlink by default, a copy on adapters that declare one — **at its source
directory**. When that source was the live library directory, the artifact
tracked whatever the directory later became: after a `lib sync` pulled new bytes
for a pinned skill, the symlink's target *string* was unchanged, the lock was
unchanged, and the trust digest was unchanged, but the bytes a harness read
*through* the link were the library's new ones. Unreviewed content in agent
context with nothing re-gating it — the precise failure invariant 4 exists to
prevent, and a direct contradiction of W3's acceptance ("a `lib sync` changes no
active bytes and no rendered file in any project").

The rendered lane therefore follows the same rule as the serve lane: **a skill
that carries a lock pin is materialized against the content-addressed snapshot
for that digest, never against the live directory it was resolved from.** Both
delivery strategies read that snapshot, so the copy fallback is covered by
construction rather than by a second code path. The redirection happens after
the fail-closed drift gate, which keeps owning the "these bytes moved" refusal;
reaching the redirection means the live bytes still hash to the pin, which is
what makes repairing an absent snapshot from them safe.

Same shape at `agentstack add skill --write`, which materializes into a static
project directly rather than through `use`: it renders the store snapshot of the
bytes it just pinned. What the *lock entry* records is a separate question and is
unchanged — a path skill still records its declared directory.

Fail-closed, as on the serve lane: a pin whose snapshot is missing and cannot be
repaired, or present and not hashing to its own name, refuses the activation
naming the skill and `agentstack lock`. There is deliberately no fallback to the
live directory; a silent fallback would restore the hole exactly.

**Unpinned skills are untouched.** There is no digest to serve by, so they keep
today's behaviour — the live path, plus the existing pin-me warning. Inventing a
pin at render time would be inventing consent.

## What stays blocked

Inline-skill drift; rev drift; a git source that cannot be verified offline; a
broken ref; a skill the human answered `Blocked` on; an unpinned inline skill;
and any pinned skill whose store snapshot is missing, unplaceable, or fails
verification. Each keeps its existing message.

## Debt

One of the two below is closed; the other is still true.

**The description index reads live bytes — closed 2026-08-03 (W5).** The catalog
used to resolve each skill `PathOnly` and read the `description` out of the live
`SKILL.md` frontmatter, so after a sync the *one-line description* an agent saw
for a pinned skill could come from the library's newer bytes while the *body* it
loaded was the pinned one. `list_loadable_with_lease` now takes the description
from the **store snapshot for the skill's lock pin** whenever there is one,
falling back to the live resolution only for an unpinned skill or a snapshot
that holds no `SKILL.md`. That is the second of the two options weighed above —
serving descriptions from the store — and it is the cheap one: no body is
digested at list time, and a pinned skill skips resolution entirely, so the list
got *cheaper* rather than dearer.

The one thing that decision does **not** buy: the description line is not
re-verified against the pin. That is deliberate and is not a weakening — it is
exactly the trust level the live-frontmatter read already had (one bounded line
of content assumed hostile either way), while the **body** is still re-verified
against the pin by the load path before a byte of it reaches agent context. What
changed is only which bytes the line comes from: the ones the project consented
to. The listed entry carries `pinned: true` so a reader can tell which answer it
got. Witnessed by
`a_listed_description_comes_from_the_pinned_bytes_not_the_live_library` in
`crates/cli/tests/package_layer.rs`.

**The copy still tells two stories.** `use`, `doctor`, and `trust` describe this
same divergence in their own words, from before the distinction existed — as
drift, in copy that can read as "your project is broken". Their **behaviour** is
correct and deliberately untouched (they are consent and activation gates, not
the serve path, and a project that wants the newer bytes really does have to run
`agentstack lock`), but their **wording** should be reviewed against this
decision so a user is not told two different stories about one library that
simply moved ahead.
