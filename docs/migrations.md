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

To undo onboarding, list the labelled entries and restore them newest first:

```sh
agentstack x restore
agentstack x restore --last --write
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
`agentstack x diff` after the write to see managed, foreign, and hand-edited
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
agentstack x install --locked
agentstack apply               # preview
agentstack apply --write
agentstack doctor
```

If your dotfile manager currently owns native CLI files, choose one owner before
the write. Keep AgentStack as source of truth and stop templating its managed
entries, or keep the dotfile template and do not ask AgentStack to own the same
region. A hand-added native entry you want to preserve can be pulled into the
manifest with `agentstack adopt --write` after reviewing `agentstack x diff`.

## A team without shared secrets

Commit declarations and pins, never values:

```sh
git add .agentstack/agentstack.toml .agentstack/agentstack.lock
git commit -m "share agent toolset"
```

A teammate then runs:

```sh
agentstack x install --locked
agentstack doctor              # list unresolved reference names
agentstack secret set GITHUB_TOKEN   # repeat for each unresolved reference
agentstack apply               # preview
agentstack apply --write
agentstack doctor
```

Each person supplies values on their own machine and reviews the cloned project
before activation. Trust records are never copied. See [share a setup with your
team](howto/team-setup.md) for signatures, platform differences, and the full
handoff checklist.

## Remove AgentStack completely

First preview removal while the manifest still exists. This is the only point
where AgentStack can identify and remove just its managed entries from every
native file:

```sh
agentstack x uninstall           # preview
agentstack x uninstall --write
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
- A source build linked with `agentstack x self link`: run `agentstack x self which`
  to identify the link, then remove that link.
- The curl installer: run `agentstack x self which`, inspect the reported install
  path, then remove that binary with the permissions used to install it.

`agentstack x uninstall` is itself recorded before machine history is removed
when a manifest is available. If you may need its undo, pass `--keep-home` and
verify the native files before deleting the retained home.
