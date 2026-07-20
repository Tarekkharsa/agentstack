<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Which mode do I need?

AgentStack has two decisions to make: how much protection you want, and where
the rendered files should live. This page picks both from what you are trying
to do. New to a word below? Every term is defined in [concepts](concepts.md).
For the architect-grade version of these same two decisions, see
[ARCHITECTURE — operating model](ARCHITECTURE.md#operating-model--choose-the-boundary-you-need).

Your *CLIs* are the agent tools you run — Claude Code, Codex, Cursor, and the
rest.

## First: how much protection?

Find the row that sounds like you. The last column says how strongly each
option is *actually* enforced, in the [enforcement matrix's](ENFORCEMENT.md)
own words.

| You are… | You need | Command | What it actually does |
|---|---|---|---|
| just syncing config across your CLIs | config sync | `init` then `apply --write` | Copies your reviewed config into each CLI. No runtime check — nothing is blocked once an agent is running. |
| worried about `rm -rf` or `.env` accidents | the guard | `guard install` | **Cooperative**: catches an agent's *accidents* through each CLI's own hook. Not a determined attacker. |
| cloning repos you didn't write | the trust gate | `gateway connect` then `trust .` | A repo's servers, skills, and secrets stay **inert** until you trust it. Trust gates whether they load — it does not sandbox the code. |
| launching a frozen, verified surface, no Docker | a Protected run | `run <cli> --locked` | Fail-closed trust and pin checks before launch, then a frozen surface. Labelled `HOST / PROTECTED`. Not kernel isolation — the agent still runs as you. |
| running sensitive work that must not leak | Lockdown (Docker) | `run <cli> --sandbox --lockdown` | Container with no route out; egress is **enforced**. Unapproved egress is blocked — that never means exfiltration is impossible. Labelled `LOCKDOWN / ENFORCED`. |

The rows stack: guard, the trust gate, and a locked or lockdown run each add
protection the one above does not, so most people end up combining several.
The legend words — **cooperative**, **enforced**, coarse, unsupported — are
defined once in the [enforcement matrix](ENFORCEMENT.md), which spells out
exactly what each mode does and does not stop. `--lockdown` needs Docker;
`--locked` does not.

## Then: where do the rendered files live?

This is your *delivery mode* — how your chosen capabilities reach each CLI.

| Delivery mode | Pick it when | Where capabilities live |
|---|---|---|
| **static** (default) | you want zero setup, and it must work with every CLI | Rendered files sit on disk, ready the moment a CLI starts. Zero moving parts. |
| **clean-at-rest** | the repo must stay pristine between sessions | Files exist only between `session start` and `session end`. Nothing is left behind at rest. |
| **zero-files** | you juggle many repos and your CLI speaks MCP (Model Context Protocol) | Nothing on disk. The gateway serves each trusted repo's capabilities live. |

The wizard defaults to **static**, and you can switch delivery mode any time —
switching changes only how files are delivered, never what you trust or what
your policy allows. Not sure? Stay on static. See
[delivery modes in concepts](concepts.md) for the fuller definitions, and
[ARCHITECTURE — operating model](ARCHITECTURE.md#operating-model--choose-the-boundary-you-need)
for how delivery sits beside selection and isolation.
