<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Migration recipes

Bring the setup you already have. Every recipe starts with a read-only plan,
writes only after review, checks the result, and names the way back out.

## Claude Code + Codex

Use this when the two CLIs already have different MCP servers or inline token
values in their machine-wide configuration.

```sh
agentstack init --plan
agentstack init --secrets env
agentstack apply               # preview
agentstack apply --write
agentstack doctor
```

The plan names the Claude Code and Codex files it read, every server it can
map, unsupported fields it will leave alone, lifted secret reference *names*,
and both native destinations. The write creates one manifest and renders only
the targets that contributed configuration. Existing foreign entries are kept.
An import consents to what it imported, so the new project is trusted at those
bytes and the [review step](#the-review-step) below does not apply here — it is
for a setup arriving from somewhere else.

One thing `apply --write` may tell you instead of writing: delivery is routed,
and for an MCP-capable CLI servers are routed to the **live** lane, which
`apply` does not render. With no bridge registered yet it therefore writes
nothing and exits nonzero, naming the two ways forward — register the bridge
(`agentstack more gateway connect --all --write`) or route those kinds to files
(`agentstack more delivery render-locally --write`). `agentstack init` prints the
same two options at the end of the import. This applies to the recipe below as
well.

To undo onboarding, list the labelled entries and restore them newest first:

```sh
agentstack more restore
agentstack more restore --last --write
```

Repeat the last command for the preceding `init` entry if you also want to
remove the imported manifest and its setup files.

## Cursor + Gemini CLI

The commands are the same because adapter selection comes from detection, not a
migration-specific importer:

```sh
agentstack init --plan
agentstack init --secrets keychain
agentstack apply --write
agentstack doctor
```

Choose `keychain` when you do not want resolved values in a project `.env`.
AgentStack imports fields it can represent and explicitly lists anything lossy
or unsupported; it does not delete the original Cursor or Gemini fields. Run
`agentstack more diff` after the write to see managed, foreign, and hand-edited
entries separately.

## From dotfiles

Keep the portable inputs in dotfiles or the project repository:

```text
.agentstack/agentstack.toml
.agentstack/agentstack.lock
.agentstack/instructions/   # when used
.agentstack/skills/         # when used
```

Do **not** copy machine trust, the keychain, resolved `.env` values, history, or
`~/.agentstack/` between machines. Those records are machine-local by design.
On each machine:

```sh
agentstack more install --locked
agentstack trust .             # review — see "The review step" below
agentstack apply               # preview
agentstack apply --write
agentstack doctor
```

If your dotfile manager currently owns native CLI files, choose one owner before
the write. Keep AgentStack as source of truth and stop templating its managed
entries, or keep the dotfile template and do not ask AgentStack to own the same
region. A hand-added native entry you want to preserve can be pulled into the
manifest with `agentstack adopt --write` after reviewing `agentstack more diff`.

## A team without shared secrets

Commit declarations and pins, never values:

```sh
git add .agentstack/agentstack.toml .agentstack/agentstack.lock
git commit -m "share agent toolset"
```

A teammate then runs:

```sh
agentstack more install --locked
agentstack doctor              # list unresolved reference names
agentstack secret set GITHUB_TOKEN   # repeat for each unresolved reference
agentstack trust .             # review — see "The review step" below
agentstack apply               # preview
agentstack apply --write
agentstack doctor
```

Each person supplies values on their own machine and reviews the cloned project
before activation. Trust records are never copied. See [share a setup with your
team](howto/team-setup.md) for signatures, platform differences, and the full
handoff checklist.

## The review step

Every recipe above that brings a setup to a machine ends at the same gate, so
it is worth stating once rather than in each block. **Trust is recorded per
project directory and is never copied**, so a clone, a second checkout, or a
fresh git worktree of a project you already approved arrives *untrusted* at its
new path — even with the manifest and lockfile committed and `agentstack more
install --locked` already run.

Until it is reviewed, a write refuses in place and names the item it withheld:

- `agentstack apply --write` writes no native MCP server config, compiles no
  instruction fragments, renders no hooks and no native extensions;
- `agentstack use --write` materializes no skills.

Each prints a `✗` line pointing at `agentstack trust .` and exits nonzero, so a
half-configured setup is not what you get — nothing is written at all. `trust .`
shows the reviewed surface and asks one closing question, so it needs a person
at a terminal; a scripted rollout uses the digest-bound pair instead
(`agentstack trust --preview`, then `agentstack trust --yes --consented
<digest>`), which refuses a stale or missing digest.

Nothing about this is a migration: it is the same first-run review a project
gets anywhere, arriving once per machine and per checkout.

## What changed on disk

Two notes for anyone tracking the file formats rather than the commands.

**`agentstack.lock` gained `[[setting]]` rows, and nothing needs migrating.**
Each row pins one native settings key per target — `target`, `key`, `checksum`
over the value *as declared*, `${REF}`s unresolved — at the same grain the
renderer owns keys, so a probe can name which key moved. The lockfile version
did **not** move; these rows are additive:

- an older lock with no `[[setting]]` rows loads and works unchanged;
- `agentstack lock --write` backfills them on the next run;
- a project that declares no `[settings.*]` keeps a byte-identical lockfile —
  the list is omitted entirely, so no one is re-gated by the arrival of a new
  pin kind.

A settings pin is also not a delivery gate: unlike an unpinned skill or
instruction, an unpinned or drifted settings key does not refuse a render.

**`~/.agentstack/audit/trust.jsonl` gained two actions.** Beside `grant`,
`regrant`, `repin` and `revoke`, a line's `action` can now be `decide` or
`undecide` — a standing re-gate answer being recorded on an existing entry, or
withdrawn from one. Neither re-pins anything: both carry the digest the entry
already stood on. The file remains identity-only and append-only, and it is
machine-local: like every trust record, it is not copied between machines.

## Remove AgentStack completely

First preview removal while the manifest still exists. This is the only point
where AgentStack can identify and remove just its managed entries from every
native file:

```sh
agentstack more uninstall           # preview
agentstack more uninstall --write
```

The summary names what it removed and what it deliberately retained. The
manifest, local `.env`, and capability source files are your inputs, so they are
not silently deleted. Review and delete those yourself only if you no longer
want the setup.

If the project is already gone but machine state remains, run the same command
from a directory without a manifest. It previews and removes `~/.agentstack`
only; it does not guess which project files to edit.

Finally remove the executable through the channel that installed it:

- Homebrew: `brew uninstall agentstack`.
- A source build linked with `agentstack more self link`: run `agentstack more self which`
  to identify the link, then remove that link.
- The curl installer: run `agentstack more self which`, inspect the reported install
  path, then remove that binary with the permissions used to install it.

`agentstack more uninstall` is itself recorded before machine history is removed
when a manifest is available. If you may need its undo, pass `--keep-home` and
verify the native files before deleting the retained home.
