<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Which mode do I need?

AgentStack has two decisions to make: where the rendered files should live,
and how much protection you want. This page picks both from what you are trying
to do. New to a word below? Every term is defined in [concepts](concepts.md).

Your *CLIs* are the agent tools you run — Claude Code, Codex, Cursor, and the
rest.

## First: how do capabilities reach your CLIs?

**You do not have to choose.** Since 2026-08-03 delivery is *routed*, not
picked: AgentStack decides per capability, from what kind it is and which CLI it
is going to, and says what it decided. `agentstack x delivery` shows the routing
for your project.

| Capability | Where it goes | Why |
|---|---|---|
| Skills · MCP servers, on a CLI that speaks MCP | served live, on demand | brokered, policy-checked, digest-verified, recorded — and nothing generated lands in your repo for them |
| House rules · settings | written into native files | a setting only a file carries; house rules because no live channel a CLI is *known* to consume varies by model |
| Hooks · extensions | written into native files, reviewed in full every time | they run code |
| Anything, on a CLI that reads files only | written into native files | that CLI has no live channel |

A project is normally in **both** lanes at once, and that is the ordinary case,
not a compromise.

**The one escape hatch: render locally.** `agentstack x delivery render-locally
--write` (add `--harness <id>` for a single CLI) writes files even where the
live channel would have worked. Pick it for offline work, deterministic native
files, inspection with ordinary filesystem tools, a rule against a persistent
background process, debugging without another runtime dependency, or testing a
CLI's own behaviour. Switching it changes only *where the bytes go* — never what
you trust or what your policy allows.

The older per-project delivery modes (`static`, `clean-at-rest`, `zero-files`)
are readings of a project's current shape, not choices: `agentstack set-mode`
is retired and delivery is routed for you. See
[delivery modes in concepts](concepts.md) for what each still means, and
[ARCHITECTURE — operating model](ARCHITECTURE.md#operating-model--choose-the-boundary-you-need)
for how delivery sits beside selection and isolation.

## Then: how much protection?

Find the row that sounds like you. The last column says how strongly each
option is *actually* enforced, in the [enforcement matrix's](ENFORCEMENT.md)
own words.

| You are… | You need | Command | What it actually does |
|---|---|---|---|
| just syncing config across your CLIs | config sync | `init` then `apply --write` | Copies your reviewed config into each CLI. No runtime check — nothing is blocked once an agent is running. |
| worried about `rm -rf` or `.env` accidents | the guard | `guard install` | **Cooperative**: catches an agent's *accidents* through each CLI's own hook. Not a determined attacker. |
| cloning repos you didn't write | the trust gate | `gateway connect` then `trust .` | A repo's servers, skills, and secrets stay **inert** until you trust it. Trust gates whether they load — it does not sandbox the code. |
| launching a frozen, verified surface, no Docker | a Protected run — already the default | `run <cli>` | Fail-closed trust and pin checks before launch, then a frozen surface. This is what a plain `agentstack run` does now; `--unprotected` opts out. Labelled `HOST / PROTECTED`. Not kernel isolation — the agent still runs as you. |
| running sensitive work that must not leak | Lockdown (Docker) | `run <cli> --sandbox --lockdown` | Container with no route out; egress is **enforced**. Unapproved egress is blocked — that never means exfiltration is impossible. Labelled `LOCKDOWN / ENFORCED`. |

The rows stack: guard, the trust gate, and a protected or lockdown run each add
protection the one above does not, so most people end up combining several.
The legend words — **cooperative**, **enforced**, coarse, unsupported — are
defined once in the [enforcement matrix](ENFORCEMENT.md), which spells out
exactly what each mode does and does not stop. `--lockdown` needs Docker; the
protected default does not.

**Not sure?** Let delivery stay automatic and add `guard install` — that is the
right answer for almost everyone. [Get started](start.html) sets both up.

