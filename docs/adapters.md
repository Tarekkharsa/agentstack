<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# AgentStack — Adapter support matrix

Thirteen adapters ship today. They are **not equally verified**, and saying
"supported" thirteen times would be the configuration equivalent of claiming
enforcement we do not have. This page answers one question per adapter: what is
actually tested, how often, and what should you expect when the CLI on the other
end changes its config schema?

Every row below is derived from something in this repository — the nightly
workflow's job matrix, the committed render snapshots, and the adapter
descriptors themselves. Where there is no evidence, the cell says so instead of
guessing. This is the same claim discipline the
[enforcement matrix](ENFORCEMENT.md#claim-discipline) applies to security: never
claim more than is verified.

Audience: anyone deciding whether to point AgentStack at a particular CLI.

**Contents**

- [How the tiers are defined](#how-the-tiers-are-defined)
- [The matrix](#the-matrix)
- [What each adapter manages](#what-each-adapter-manages)
- [When an upstream CLI changes its schema](#when-an-upstream-cli-changes-its-schema)
- [Why conformance runs nightly, not on pull requests](#why-conformance-runs-nightly-not-on-pull-requests)
- [Adding or overriding an adapter](#adding-or-overriding-an-adapter)
- [See also](#see-also)

## How the tiers are defined

Two independent checks exist. A tier is just which of them an adapter gets.

**Render snapshot** — a fixed manifest (one stdio server, one HTTP server with a
header secret) is rendered to the adapter's native format and compared byte for
byte against a committed snapshot in
[`crates/cli/tests/snapshots/`](../crates/cli/tests/snapshots). It runs on every
pull request, in ordinary CI. It proves *we still write what we meant to write*,
including that adapter's quirks. It proves nothing about the tool that reads the
file.

**Nightly live check** — the [`conformance`](../.github/workflows/conformance.yml)
workflow installs the real CLI from its own registry at `latest`, renders a config
into a fenced `HOME`, and asks that CLI to read the config back
(`mcp list`, or — for the one CLI with no MCP support — a skills-directory
read-back). An unknown nonzero exit is a failure, not a skip: only a recognized
authentication or onboarding gate downgrades to a spoken skip. It proves the
upstream tool still accepts our output.

From those two facts, three tiers:

- **Tier 1 — nightly-verified.** Both checks. If a vendor ships a schema change,
  the alarm goes off within a day, before you hit it.
- **Tier 2 — best-effort.** Render snapshot only. What we write is pinned and
  reviewed, but nothing has asked the real application whether it still accepts
  the file. Correct until that vendor changes something; you may find out before
  we do.
- **Tier 3 — community-reported.** Neither check. No adapter ships in tier 3
  today. The tier exists because a descriptor you drop into
  `~/.agentstack/adapters/` lands there by definition.

Tier is about *verification frequency*, not about how complete an adapter is.
A tier-2 adapter can manage more of your setup than a tier-1 one — see
[what each adapter manages](#what-each-adapter-manages).

## The matrix

| Adapter | id | Tier | Render snapshot | Nightly live check |
| --- | --- | --- | --- | --- |
| Claude Code | `claude-code` | 1 | yes | yes — `@anthropic-ai/claude-code`, `claude mcp list` |
| Codex CLI | `codex` | 1 | yes | yes — `@openai/codex`, `codex mcp list` |
| Gemini CLI | `gemini` | 1 | yes | yes — `@google/gemini-cli`, `gemini mcp list` |
| OpenCode | `opencode` | 1 | yes | yes — `opencode-ai`, `opencode mcp list` |
| Pi | `pi` | 1 | n/a — no MCP support | yes — `@mariozechner/pi-coding-agent`, skills read-back |
| Antigravity | `antigravity` | 2 | yes | no |
| Claude Desktop | `claude-desktop` | 2 | yes | no |
| GitHub Copilot CLI | `copilot-cli` | 2 | yes | no — see the note below |
| Cursor | `cursor` | 2 | yes | no |
| Junie | `junie` | 2 | yes | no |
| Kiro | `kiro` | 2 | yes | no |
| VS Code | `vscode` | 2 | yes | no |
| Windsurf | `windsurf` | 2 | yes | no |

Notes, all of them load-bearing:

- **Pi has no render snapshot and that is correct.** Pi has no MCP support by
  design, so there is no native server config to snapshot. Its nightly leg
  renders a skill instead and reads it back from `~/.pi/agent/skills`.
- **Copilot CLI is the one near-miss.** The smoke script already carries a
  Copilot-specific probe (it runs slash-style commands through `-i` rather than
  as a subcommand), but the adapter is not in the nightly job matrix, so nothing
  runs that probe on a schedule. Treat it as tier 2 until it appears in the
  matrix.
- **Tier 2 is where the GUI applications sit.** The nightly matrix covers the
  CLIs that install headlessly in CI; Claude Desktop, Junie, and Antigravity have
  no CLI binary at all (their descriptors detect them by config path), and the
  editors are not driven headlessly there either.
- **The nightly job also re-runs the whole test suite** on the latest stable
  toolchain, so every adapter's render snapshot is re-checked nightly even though
  only five adapters get a live check.
- **One shared negative case.** The nightly manifest carries a server named
  `slash/probe`. Codex validates server names at startup against
  `^[a-zA-Z0-9_-]+$`, so the check asserts that Codex's config is rendered
  *without* it and that the skip is spoken out loud — and that every other
  adapter receives it verbatim.

`agentstack adapters list` shows the same ids and marks which of these tools look
installed on your machine. `agentstack adapters show <id>` prints one descriptor.

## What each adapter manages

This is descriptor data, not a claim: each row is read out of that adapter's YAML
descriptor in
[`crates/adapters/descriptors/`](../crates/adapters/descriptors). "yes" means the
adapter has a declared location for that kind of configuration; "—" means the
tool has no such concept, or none we map.

| Adapter | Servers | Project scope | Skills | Instructions | Hooks | Settings | Extensions | Headless run |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| `claude-code` | yes | yes | yes | yes | yes | yes | — | yes |
| `codex` | yes | yes | yes | yes | yes | yes | — | yes |
| `gemini` | yes | yes | yes | — | — | — | — | — |
| `opencode` | yes | yes | — | yes | — | — | yes | — |
| `pi` | — | — | yes | yes | — | yes | yes | — |
| `antigravity` | yes | yes | — | — | — | — | — | — |
| `claude-desktop` | yes | — | — | — | — | — | — | — |
| `copilot-cli` | yes | — | yes | yes | — | — | — | — |
| `cursor` | yes | yes | — | — | — | — | — | — |
| `junie` | yes | yes | — | yes | — | — | — | — |
| `kiro` | yes | yes | — | — | — | — | — | — |
| `vscode` | yes | yes | — | — | — | — | — | — |
| `windsurf` | yes | — | — | — | — | — | — | — |

Column meanings:

- **Servers** — MCP servers render into that CLI's native config. `pi` is the
  exception: it has no MCP support by design.
- **Project scope** — the tool reads a per-repository config file as well as a
  global one. Without it, a project-scope apply falls back to the global file.
- **Skills** — a directory the tool loads skills from, which AgentStack
  materializes by symlink.
- **Instructions** — a global and/or project instructions file (`CLAUDE.md`,
  `AGENTS.md`, and friends) whose managed region AgentStack owns.
- **Hooks** — lifecycle hooks the tool runs. Two adapters expose them.
- **Settings** — a curated catalog of that tool's own settings keys AgentStack
  can write; every key it does not list is preserved and still hand-editable.
- **Extensions** — a plugin directory the tool loads code from. Declared for two
  adapters, discovered read-only.
- **Headless run** — a prompt-in/text-out invocation, which is what
  `agentstack run --locked --prompt` and workflow steps drive. Only two adapters
  declare one.

Two things this table deliberately does not tell you. It does not say a `yes`
cell is nightly-verified — cross-reference [the matrix](#the-matrix) for that.
And it does not say the tool is *confined*: what AgentStack enforces once a CLI
is running is a separate question, answered in the
[enforcement matrix](ENFORCEMENT.md).

## When an upstream CLI changes its schema

Thirteen adapters means thirteen vendors who can change a config format without
telling us. Here is the honest sequence.

**Tier 1.** The nightly run installs the vendor's new release, renders a config,
and the CLI rejects it — the job goes red, typically within a day of the release.
The fix is a descriptor change, not code. Until then, your `agentstack apply` is
still writing the old shape.

**Tier 2.** Nothing tells us. The render snapshot still passes, because it only
compares our output to our own committed expectation — that expectation is now
wrong. You will likely notice first, in one of these forms:

- the CLI reports an unknown field, refuses to start, or silently ignores a
  server it used to load;
- `agentstack doctor` reports drift, because the CLI rewrote the file itself;
- servers you expect are simply absent from that tool's server list.

In every case, AgentStack's own state is unharmed: the manifest is the source of
truth, the write is recorded, and `agentstack restore --last --write` puts the
native file back. What is broken is the translation, and translation lives in
one YAML descriptor.

**What we ask of a report.** The CLI's version, the config file it read, and its
exact error text. That is enough to fix a descriptor; a screenshot of a
"doesn't work" is not.

## Why conformance runs nightly, not on pull requests

This is a deliberate trade, and it has a cost worth stating.

The live check installs each CLI at `latest` from a vendor registry. Running it
on pull requests would mean a change touching none of this can fail because a
vendor shipped a release that morning, or because a registry was slow, or because
a tool started demanding authentication in a way its predecessor did not. That
turns the adapter-rot alarm into noise, and a noisy alarm gets ignored.

Running it nightly keeps the signal: a red `conformance` run means one specific
vendor changed something, and the run says which. The cost is latency — up to a
day between a vendor's release and the alarm, and no pull-request-time proof that
an adapter edit still satisfies the real CLI. The person who edits a descriptor
should run the smoke script locally; it fences `HOME` and never touches real
configs:

```sh
./examples/sandbox/conformance-smoke.sh codex codex .codex/config.toml
```

## Adding or overriding an adapter

Adapters are data, not code: one YAML descriptor each, embedded in the binary,
with additions and overrides loaded from `~/.agentstack/adapters/`. A descriptor
you drop in there is tier 3 by definition — no snapshot, no nightly check — and
it is part of the trusted computing base: it decides where AgentStack writes and
what shape it writes. Validate one before installing it:

```sh
agentstack adapters validate ./my-cli.yaml
```

More on the format and the rendering rules:
[reference — data-driven adapters](reference.md#data-driven-adapters).

## See also

- [Enforcement matrix](ENFORCEMENT.md) — what each execution mode actually
  enforces, and what it does not.
- [Concepts — CLI, adapter, target](concepts.md#cli-adapter-target) — the three
  words this page assumes.
- [Reference — data-driven adapters](reference.md#data-driven-adapters) — the
  descriptor format, the merge rules, and the per-CLI quirks.
- [FAQ — can my teammate use a different CLI than me?](faq.md#can-my-teammate-use-a-different-cli-than-me)
