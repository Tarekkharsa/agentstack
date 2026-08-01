<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# FAQ

The questions that come up in the first week. If something is *broken* rather
than unclear, start at [troubleshooting](troubleshooting.md).

## Will this overwrite the CLI configs I already have?

No — and nothing is written until you pass `--write`. Every command previews
first, so a bare `agentstack apply` prints the exact diff and stops.

agentstack edits only the region it manages inside each native config. Servers
you added by hand, and servers a *different* project's manifest applied at
global scope, are kept and reported rather than pruned. Removing those takes
the explicit `agentstack apply --prune-foreign`.

`agentstack init` starts by **importing** what you already have into the
manifest, so the first apply is usually a no-op that just makes your existing
setup portable.

## What happens if I uninstall it?

`agentstack uninstall` takes off every managed region it rendered, previewing
first. Your `agentstack.toml` stays exactly where it is, so `agentstack apply
--write` brings the whole setup back later. Entries you or another tool wrote
into those same files are left alone.

Because the removal runs through the same machinery as a normal write, the
uninstall is itself undoable with `agentstack restore --last --write`. The
binary is not removed — take that off the way you installed it. Details:
[undo anything](howto/undo.md).

## Is my API key stored in the manifest?

No. Manifests hold `${REF}` placeholders only; values are resolved per machine
from your environment, varlock, a gitignored project `.env`, or the OS
keychain. A ref that does not resolve **blocks the write** rather than
rendering an empty string.

That is why the manifest is safe to commit, and why a teammate cloning it runs
`agentstack secret set <NAME>` for their own values instead of receiving yours.
More: [concepts — secrets](concepts.md#secrets).

## Do I have to commit the manifest?

You do not have to, but that is where the value is: `.agentstack/` (manifest +
lockfile) is intent, never credentials, so committing it is how a team shares
one setup. The *rendered* files are a different matter — agentstack adds them
to a managed `.gitignore` block by default, since they are compiled output. If
your team prefers to commit them, pass `--no-gitignore`.

## Can my teammate use a different CLI than me?

Yes. That is the point. One manifest compiles to each CLI's native format, so
you can be on Claude Code and your teammate on Codex from the same committed
file. `[targets].default` lists which CLIs commands act on, and each person can
narrow that for their own machine.

Thirteen adapters ship today: Antigravity, Claude Code, Claude Desktop, Codex
CLI, GitHub Copilot CLI, Cursor, Gemini CLI, Junie, Kiro, OpenCode, Pi, VS
Code, and Windsurf. `agentstack adapters list` shows which are installed here.
They are not equally verified — five get a nightly check against the real CLI
and the rest are best-effort. The
[adapter support matrix](adapters.md) says which is which, and what each one
manages.

## What if two of my CLIs already define the same server?

`agentstack init` imports both into one manifest entry. If the two definitions
genuinely differ, it keeps the first one imported and says so out loud:

```text
⚠ server 'github' is defined differently by 1 other CLI — kept the first
  definition imported (the other stays in its CLI's own config)
```

Nothing is lost — the other definition stays in its own CLI's config until you
apply. Review the merged entry, edit it if the wrong one won, then
`agentstack apply --write` makes both CLIs agree. That is how they stop
drifting apart.

## Do I need Docker?

No. Docker is only needed for `agentstack run --sandbox` and `--lockdown`, the
top of the protection ladder. Importing, unifying, applying, naming toolsets,
diagnosing, and undoing all work with no container runtime at all. See
[which mode do I need?](choose.md).

## Why does it ask me to trust my own project?

Usually it does not. A consented `agentstack init` records trust as part of
setup, so the gate mainly appears in two situations: a repository you
**cloned** (someone else wrote those declarations), and a manifest that
**changed** since you approved it.

Untrusted means inert — a repo's declarations cannot spawn servers, enter agent
context, or resolve secrets until a human has read them. Trust is bound to the
content it approved, so a `git pull` that changes pinned bytes drops the repo
back to inert on purpose. `agentstack trust .` prints the full declared surface
and asks. Details: [trust a cloned repo](howto/trust-a-repo.md).

## I dropped a skill folder into `.agentstack/skills/` — how do I use it without editing any config?

Run `agentstack yes` (v0.18.0 and later; the current stable install serves
v0.17.1). The dropped files are noticed, pinned, and shown on one review card —
what gets declared, what each CLI will receive, and the undo that reverses it —
and one confirmation records them in the manifest and lock and renders them
everywhere. No manifest edit, no `.mcp.json`, no per-CLI skills directory.

The one caveat is provenance: files you demonstrably wrote here (untracked in
git, or newer than the last review) get that one-step path, while anything that
arrived with a clone is somebody else's work and takes the full staged review
instead. Walkthrough: [add a skill](howto/add-a-skill.md).

## Does agentstack replace my agent CLI?

No. It configures the CLIs you already run — you keep launching `claude`,
`codex`, or whatever you use. `agentstack run <cli>` is an optional wrapper
that launches one as a tracked run so you get a flight recording, and the
stronger `--locked` / `--sandbox` / `--lockdown` postures build on that. Plain
`apply` needs none of it.

## What is the difference between `use` and `session start`?

`agentstack use <name> --write` activates a toolset persistently — it renders
its servers and materializes its skills, and they stay until you change them.
`agentstack session start <name>` is the temporary form: the same activation,
but `agentstack session end` puts every file back.

Naming a toolset does not activate it. `agentstack toolset create` writes the
manifest entry and re-locks, and renders nothing at all — activation is always
a separate, explicit step. More:
[name a toolset](howto/name-a-toolset.md).

## Does it work on Windows?

Not as a supported platform. A Windows binary is published with each release,
but CI never runs on Windows and the codebase carries almost no
Windows-specific handling, so it is untested rather than supported. macOS and
Linux are the platforms the project stands behind. WSL works, because that is
Linux.

## Does it work offline?

Mostly. `doctor`, `diff`, `apply`, `use`, `restore` and toolset resolution are
static and offline. Three things reach the network when you ask them to:
`agentstack search` queries the MCP Registry alongside your local library,
`agentstack install` fetches git-hosted skills into the store, and
`agentstack doctor --live` performs a real MCP handshake against HTTP servers.

Everything fetched is pinned in `agentstack.lock`, so a later run verifies
bytes rather than re-downloading them.

## I ran a command and nothing happened — is it broken?

Almost certainly not. Nothing touches disk without `--write`; a bare command
prints the plan and exits. This is deliberate: you should be able to run any
agentstack verb on an unfamiliar machine and learn what it *would* do.

If you expected output and got none, `agentstack status` says where the project
stands in one screen and names the single next step.

## The servers applied, but my CLI still doesn't see them

Restart the CLI. A harness reads its config at startup, and `apply` says so
when it writes. If a restart does not fix it, the cause is usually scope
(project-local versus the CLI's user-level config) — see
[my CLI doesn't see the servers](troubleshooting.md#my-cli-doesnt-see-the-servers).

## Does "trusted" mean the code is safe?

No, and the distinction matters. Trust is **consent to activate what you
reviewed** — it certifies that a human read the declarations and that the bytes
have not changed since. It does not audit the server's source, and an allowed
destination can still exfiltrate.

The same discipline applies elsewhere: host advisory checks are not
confinement, and recording is not prevention. Exactly what each mode does and
does not enforce is written down, per cell, in the
[enforcement matrix](ENFORCEMENT.md).

## Where do I report something wrong with the docs?

The Markdown under `docs/` is the source of truth and the pages here are
compiled from it, so a fix is a normal pull request against the `.md` file.

- [Troubleshooting](troubleshooting.md) — search for the error text you got
- [Concepts](concepts.md) — every term in two or three plain sentences
- [Reference](reference.md) — the complete command inventory
