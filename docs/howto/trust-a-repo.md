<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Trust a cloned repo

For anyone who clones repos that ship their own agent capabilities and wants
them inert until reviewed. Prerequisite: the CLIs you use, installed on this
machine.

```bash
# Once per machine: register the AgentStack gateway in your CLIs
agentstack x gateway connect --all --write

# Clone a repo and enter it
git clone <some-repo> && cd <some-repo>

# The repo is inert — an agent here sees control-plane tools only,
# nothing spawned, nothing contacted, no secrets resolved
agentstack trust .          # review what it declares, then pin its digest

agentstack trust --list     # every trusted project + whether it still matches
agentstack trust --revoke   # withdraw trust
```

`gateway connect --all --write` registers agentstack's gateway once in each
CLI's global MCP ([Model Context Protocol](../concepts.md)) config. After that,
every repo you open serves its own MCP servers with no files copied in — but a
repo you just cloned is **inert**: none of its servers run or are contacted, and
no secrets resolve, until you run `agentstack trust .`. Trust shows exactly what
the manifest runs and contacts, then pins the [consent digest](../concepts.md)
of the [manifest](../concepts.md), its local overlay, and the
[lockfile](../concepts.md). Any edit — a `git pull`, an `agentstack lock --write` —
drops the repo back to inert until you trust it again. To vet one server or
skill in depth first — its provenance, effective policy, and context cost — run
`agentstack x explain <name>`; see [see what your agents did](see-what-happened.md).

## What the review gates

The gate is not only about the live lane. Until this project is trusted at its
current bytes, **five kinds of delivery refuse** — every one of them a way the
repo's own words or commands would otherwise reach an agent:

| Refused until reviewed | The command that hits it |
| --- | --- |
| MCP server definitions written into a CLI's native config | `agentstack apply --write` |
| Skill files materialized into a CLI's skills directory | `agentstack use --write`, `agentstack add skill … --write` |
| Instruction fragments compiled into the managed `CLAUDE.md` / `AGENTS.md` region | `agentstack apply --write`, `agentstack x instructions --write` |
| Lifecycle hooks | `agentstack apply --write` |
| Native harness extensions | `agentstack apply --write` |

Serving over the gateway is gated the same way, and so are
`agentstack x session start` and a protected `agentstack run` — those two refuse
outright rather than deliver a partial setup.

**What stays outside it.** Three things deliberately do not ask:

- **Taking bytes off disk.** A removal-only plan — deactivation,
  `agentstack x unrender --write`, `agentstack x restore --write`,
  `agentstack x uninstall --write`, hook and extension pruning — is the inert
  direction, so it keeps working in a repo you have not trusted. You can always
  undo, and you never need consent to remove.
- **Your machine layer.** The machine manifest in `$AGENTSTACK_HOME` is your own
  personal setup, not a project, so `trust` can never reach it — gating it would
  make machine-level capabilities permanently undeliverable rather than merely
  pending a review no command can perform.
- **A command reviewing its own write.** `agentstack add … --write` is judged
  against the trust state as it stood *before* it wrote, because a command must
  not refuse the very thing you typed it to get. In an already-trusted project
  the add still delivers; it then leaves the project reading drifted, so the
  next command asks for the review.

## What a refusal looks like

It is loud, it names the capability, and it exits nonzero. Nothing is written:

```text
✗ refusing to materialize skills: project at /path/to/repo is not trusted —
  review and `agentstack trust .` before putting its words into an agent's
  context ('pdf-review')
✗ skills not materialized — the project has not been trusted for this content
error: 3 targets blocked — each ✗ above names the blocker
```

A repo you cloned and never reviewed says **is not trusted**; a repo whose
manifest, overlay, or lockfile moved since you said yes says **changed since it
was trusted**. Both are fixed by the same command, `agentstack trust .`.

## Lock first, then trust

The [lockfile](../concepts.md) is part of the consent surface, so
`agentstack lock --write` **invalidates a grant you already have** — new pins
are new consent. That fixes the order for good:

```bash
agentstack lock --write     # pin the new bytes (this re-opens consent)
agentstack trust .          # review what moved, then approve it
agentstack use --write      # …now it activates
```

The reverse order costs you the review twice. It also means re-locking is not a
way past a refusal: when pinned content has drifted, `agentstack lock --write`
accepts the new bytes but does **not** deliver them — accepting content is not
an answer to the consent question it reopens. `lock --write` says so as it runs:

```text
⚠ this project is trusted — new pins are new consent, so its trust is now
  stale; re-review and re-grant with `agentstack trust .`
```

`agentstack trust` honours the global `--manifest-dir <dir>` flag, so you can
review a project without changing into it.

## Limits

**What trust covers, and what it doesn't.** Trust pins those three files and
gates the five deliveries above; it does **not** vouch for the code the declared
servers point at. A server that runs a local script authorizes *that command*,
not later edits to it — so review referenced scripts as part of `trust .`, the
way you'd read a `.envrc` before `direnv allow`. The full boundary is the
enforcement matrix's
[What "trusted" does and does not mean](../ENFORCEMENT.md#what-trusted-does-and-does-not-mean).

**It gates activation, not behaviour.** Trust is consent to a set of bytes, not
a sandbox: it decides whether a server may be delivered, never what it does once
it runs. For runtime confinement, see [lock down a run](lock-down-a-run.md).

**Re-trusting adds up.** Consent is one grant over the whole project, not one
per capability, and it is bound to bytes rather than to intent. So adding one
server, pulling one commit, or re-pinning one skill drifts the single grant, and
the next `apply`, `use`, or `session start` refuses until you review again — in
a repo whose setup changes daily, that is a review per change. Two things keep
it survivable: the review is a **diff**, marking only what moved since you last
said yes (`+` added, `~` changed, `-` removed) rather than making you re-read
the whole surface, and the undo direction never asks — deactivating,
`x unrender --write`, and `x restore --write` all work untrusted, so postponing
a review never traps you. The practical habit is to batch a run of edits, then
spend one `lock --write` and one `trust .` on the lot.

**And it does not travel.** The grant lives in your own trust store, keyed by
the project's path on this machine. A teammate, a second checkout of the same
repo in another directory, a new laptop, and every fresh CI runner all start
untrusted — by design, since consent is per person, per machine. See
[share one setup with your team](team-setup.md) and [use it in CI](ci.md).

- [Concepts](../concepts.md) — trust, gateway, consent digest, drift
- [Reference: the zero-files gateway](../reference.md#the-zero-files-gateway---auto-project--trust)
- [Enforcement: what "trusted" means](../ENFORCEMENT.md#what-trusted-does-and-does-not-mean)
