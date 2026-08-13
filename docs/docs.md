<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py

     This page is the documentation hub, and it is generated like every other
     page: make-docs-pages.py fails the build when a page it generates is not
     linked from here, so the index cannot silently fall behind the docs. -->

# Documentation

AgentStack keeps your reusable capabilities — MCP servers, skills, and
instructions — in one central library, lets each project select a small toolset
by name, pins exactly what it selected, and serves that to every supported
agent CLI without copying configuration into every repo.

```text
your library repo  →  each project's manifest + lock  →  every agent CLI
```

Install it in about four seconds — the download is verified against the
checksums published with the release:

```bash
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
```

Then [Get started](start.md), which also covers building from source and what
that costs you.

## Grow into it

Start with configuration portability. Add toolsets, sharing, and stronger
governance only when you need them.

| Step | You run | You get |
| --- | --- | --- |
| [1 — Unify](start.md) | `agentstack init` → `more gateway connect --all --write` | import once, delivered everywhere |
| [2 — Switch](howto/name-a-toolset.md) | toolsets · `session start/end` | toolsets and temporary sessions |
| [3 — Diagnose](start.md#commands-you-will-use-most) | `agentstack doctor` · `diff` | doctor and diff explain drift |
| [4 — Recover](howto/undo.md) | `adopt` · `apply` · `restore` · `uninstall` | keep an edit, or undo the write |
| [5 — Share](howto/team-setup.md) | the setup file · its lock · the library | locked, secret-free setups |
| [6 — Govern](howto/trust-a-repo.md) | trust · policy · lockdown | trust, policy, confined runs |

Everything published here is linked from this page: the guides, the how-tos,
the reference, and the enforcement record.

## Start here

These pages take you from an empty machine to a working, reviewed setup. You do
not need the full reference to begin.

- **[Get started](start.md)** — install, set up the first machine, add project
  files, lock and trust them, and repeat on a second machine.
- **[Central library](library.md)** — the folder layout, several linked
  libraries, precedence, shared instructions, and syncing.
- **[Tutorial](tutorial.md)** — the step-by-step map of the walkthrough, and
  where each step is taught.
- **[Concepts](concepts.md)** — the three layers, delivery modes, dynamic
  skills, tool discovery, trust, and policy.
- **[Which protection do I need?](choose.md)** — how capabilities are routed
  for you, and how much enforcement to ask for.

## Most-used commands

| Command | Why you run it |
| --- | --- |
| `agentstack status` | See whether this project is ready, and get one next action. |
| `agentstack lib sources` | See every central-library source, its order, and any collisions. |
| `agentstack lock` | Preview the exact library content this project would accept. |
| `agentstack trust .` | Review by hand after the manifest or the lock changes. |
| `agentstack up` | Preview syncing the library and reconciling this machine. |
| `agentstack doctor` | Run the deep check when status points at a problem. |
| `agentstack undo` | Review and reverse AgentStack-managed writes. |

Every command, flag, and exit code is in the [feature reference](reference.md).

<a id="howtos"></a>

## How-to guides

One task per page, start to finish.

- **[Add an MCP server](howto/add-a-server.md)** — store one reusable
  `${REF}`-safe definition and select it by name.
- **[Add a skill](howto/add-a-skill.md)** — write a description that retrieves
  well, validate it, and make it loadable.
- **[Name a toolset](howto/name-a-toolset.md)** — bundle the smallest set of
  servers and skills a task needs, and pick the default.
- **[Run a multi-agent workflow](howto/run-a-workflow.md)** — fan work out to
  narrow roles, verify the result, and read one evidence tree.
- **[Trust a project](howto/trust-a-repo.md)** — why a fresh clone is inert,
  and what the consent ceremony shows you before you accept it.
- **[Lock down a run](howto/lock-down-a-run.md)** — confine an agent run to the
  files and destinations you approved.
- **[See what your agents did](howto/see-what-happened.md)** — inspect what was
  run, called, and spent, from recorded evidence.
- **[Undo AgentStack changes](howto/undo.md)** — the timeline, and how to
  reverse managed writes byte for byte.
- **[Share one setup with a team](howto/team-setup.md)** — what travels in Git,
  what stays local, and how each teammate starts.
- **[Use it in CI](howto/ci.md)** — gate a repository's agent setup in
  continuous integration.

## When something is wrong

- **[Troubleshooting](troubleshooting.md)** — search for the text your terminal
  printed, and run the repair command it names.
- **[FAQ](faq.md)** — the questions people ask before and after adopting it.

Common ones:

- [What does the lock mean?](faq.md#what-does-the-lock-mean)
- [How does the agent see my skills if they are not copied into the repo?](faq.md#how-does-the-agent-see-my-skills-if-they-are-not-copied-into-the-repo)
- [Can I link my GitHub library and keep my old local library?](faq.md#can-i-link-my-github-library-and-keep-my-old-local-library)
- [How do CLI- and model-specific instructions work?](faq.md#how-do-cli--and-model-specific-instructions-work)

## Bring the setup you already have

- **[Migration recipes](migrations.md)** — import existing MCP entries and
  native skills, each recipe starting from a read-only plan.

## Reference

- **[Feature reference](reference.md)** — every command, flag, and behaviour
  that is implemented and tested.
- **[Adapter support matrix](adapters.md)** — which CLIs are supported, and how
  far each one is verified.
- **[Integrations](integrations.md)** — how AgentStack sits under t3code and
  other callers, and what stays standalone.
- **[Automation contract](automation.md)** — the stable JSON surface, so a
  program can read everything a person can.

## Deeper

- **[How it works](ARCHITECTURE.md)** — crates, boundaries, and the runtime
  path a call takes.
- **[Governed workflows](workflows.md)** — the workflow engine, its evidence,
  and its limits.
- **[Enforcement matrix](ENFORCEMENT.md)** — every enforcement claim, what
  backs it, and what it deliberately does not cover.
- **[Demos](examples.html)** — scripts that check the claims on this site and
  fail loudly.
