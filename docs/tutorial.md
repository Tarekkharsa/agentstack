<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Tutorial

One project, start to finish, in six steps: install AgentStack, import what
your CLIs already have, add a capability, review it, activate it, and take it
all back. Each step is one command, and together they are the everyday
product.

Every command here was run against `agentstack 0.18.0-rc.2`. The transcripts
are real output, abridged, with paths shortened to `~/your-project`.

Read it through, or run it in a scratch repository as you go.
[Get started](start.md) is the same journey with less explanation;
[Concepts](concepts.md) defines every word used here; [Every
command](reference.md) is the whole surface.

## The problem this solves

Your coding CLIs each spell the same setup differently: the same MCP servers,
the same skills, the same house rules, copied into incompatible files that
drift apart. AgentStack keeps one reviewed manifest —
`.agentstack/agentstack.toml` — and delivers it to every CLI you have.

Underneath that there is a quieter problem. Adopting agent configuration from
a repository is `npm install` with an agent attached: anything the repository
declares would otherwise load into your tools unread. So the same manifest is
also what you consent to, byte for byte, before any of it activates. That
gate is step 3, and it is the part readers most often get wrong.

Four ideas cover the product, and this tutorial does all four: **setup** (what
you have), **toolset** (what this task needs), **status** (is it ready), and
**undo** (how to take it back).

## Step 1 — Install, then import what you already have

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
agentstack --version
```

Then, in the project you want to set up:

```sh
cd your-project
agentstack init
```

`init` is a guided wizard: it scans before it asks. It detects the agent CLIs
on this machine, imports the MCP servers and settings they are already
configured with, lifts any inline token into a `${REF}` placeholder, asks
where those values should live, and previews everything before it writes.

```text
$ agentstack init
🔍  Found 5 coding tools and their native configs:
      Claude Code          binary on PATH — no config files found
      Codex CLI            binary on PATH — no config files found
📦  Files agentstack will manage:
      .agentstack/agentstack.toml   the manifest — written by this import
🚚  How each tool gets them:
      Claude Code          skills + MCP servers planned live (not connected) · house rules + settings + hooks written to files
      Pi                   skills + house rules + settings + extensions written to files — this tool reads files only
✅  Wrote ~/your-project/.agentstack/agentstack.toml

Import complete.
  Undo:      agentstack x restore --last --write
  Next:      agentstack doctor          (check the result)
```

What it leaves behind is small, and all of it is yours to commit:

```text
your-project/
├── .agentstack/
│   ├── agentstack.toml   # the manifest: servers, skills, instructions, toolsets
│   └── .env              # token values lifted out of your configs (only if init found any)
└── .gitignore            # a managed block, so .env is never committed
```

The manifest holds `${REF}` placeholders and never a secret value. The real
values go to the OS keychain or that gitignored `.env`. Scripting this, or
running it in CI? `agentstack init --yes --secrets skip` writes the manifest
and asks nothing. Add `--connect` and the same run also registers the bridge
your CLIs need, so the setup delivers the moment `init` returns; without that
flag `init` writes nothing outside this project.

## Step 2 — Add a capability

Search once across your library, the bundled catalog, and the official MCP
registry:

```text
$ agentstack search postgres
4 of 5 results for 'postgres', most relevant first:

catalog
  postgres PostgreSQL — query and inspect a database (read-only)
    trust: ⚠ runs code (npx) · needs secret
    ↳ agentstack add from postgres

MCP registry
  postgres-mcp PostgreSQL MCP server - query, schema introspection, explain…
    io.github.YawLabs/postgres-mcp
    trust: ✓ verified namespace · ⚠ runs code (npx)
```

Adding shows you the exact manifest change first:

```text
$ agentstack add from postgres --write
found postgres (catalog) — postgres
→ add 'postgres' in ~/your-project/.agentstack/agentstack.toml
  + [servers.postgres]
  + command = "npx"
  + args = ["-y", "@modelcontextprotocol/server-postgres"]
  + [servers.postgres.env]
  + POSTGRES_URL = "${POSTGRES_URL}"
✓ added 'postgres'.
↳ review secrets with `agentstack secret list`, then `agentstack apply`.
```

Skills work the same way: `agentstack add skill owner/repo --write` installs
one from any repository. To write your own, drop a folder holding a
`SKILL.md` under `.agentstack/skills/` and run `agentstack yes` — one review
card, and it is pinned and live everywhere. `yes` needs a terminal; the
headless equivalent is `agentstack adopt --write` followed by step 3.

Nothing you have added is active yet. Adding changed the bytes of your setup,
and nothing activates until those bytes are pinned and approved.

## Step 3 — Lock, then trust (in that order)

This is the step to get right, because the order is not interchangeable.

`agentstack lock --write` resolves every reference and pins it — server
definitions, skill bytes, instruction bytes — to SHA-256 digests in
`agentstack.lock`:

```text
$ agentstack lock --write
✓ pinned 1 skill + 1 server from the implicit default (no toolsets declared) in ~/your-project/.agentstack/agentstack.lock
  no configs rendered, no skills materialized — that stays `agentstack use --write`.

Next: `agentstack trust .` to review and consent.
```

`agentstack trust .` then shows you what the project runs and asks:

```text
$ agentstack trust .
Reviewing ~/your-project — approving this lets its capabilities activate.

This project will:
  run 1 command on your machine — postgres
  be able to read 1 secret — POSTGRES_URL
  add 1 file to every agent's context
  …using exactly the content shown below, pinned to these bytes.

This project declares — review what auto-mode may run/contact:
  servers (spawned or contacted over MCP):   [+1 added]
+ ▶ postgres: runs `npx -y @modelcontextprotocol/server-postgres`
+ secrets referenced: POSTGRES_URL
  skills loadable over MCP:   [+1 added]
+ · deploy-checklist   [inline, pinned]
  machine policy ceiling: ~/.agentstack/agentstack.toml — the repo can only narrow it, never loosen it

✓ trusted at sha256:eb29b1b8…
Editing the manifest or lockfile invalidates this — re-run `agentstack trust` after reviewing changes.
```

**Lock first, then trust.** The grant is bound to a digest of the manifest
layers *and* the lockfile, so a later `agentstack lock --write` invalidates
the grant you just gave — the project drops back to untrusted and you review
again. Trusting before locking wastes the review. A `git pull` that touches
either file has exactly the same effect.

The review is not only about servers. **Five kinds are gated**, because each
one either runs code or puts words directly into an agent's context:

| Kind | Why it is gated |
| --- | --- |
| servers | spawned as a process, or contacted over MCP |
| skills | their words enter every agent's context |
| house rules | written into the region a harness reads into context |
| hooks | a hook is a command the harness runs |
| native extensions | extensions run code inside the CLI |

Trust says *you read this*. It is not a claim that the code is safe, and it
does not confine a server once it runs — see [what trusted does and does not
mean](ENFORCEMENT.md#what-trusted-does-and-does-not-mean). To vet one
capability in depth before consenting, use `agentstack x explain postgres`.

## Step 4 — Activate it

Two commands deliver a trusted setup. `agentstack apply --write` renders what
belongs in files; `agentstack use --write` (or `agentstack use backend
--write` when you have named toolsets) activates a toolset's servers and
skills.

Both refuse on a project you have not reviewed, or have edited since:

```text
$ agentstack use --write
Activating toolset 'default' (scope: project) — 1 server, 1 skill

Claude Code
  ✗ refusing to materialize skills: project at ~/your-project changed since it was trusted — review and `agentstack trust .` before putting its words into an agent's context ('deploy-checklist')
  ✗ skills not materialized — the project has not been trusted for this content
error: 3 targets blocked — each ✗ above names the blocker
```

That is exit 1, on your own project, because you edited it. The refusal reads
`changed since it was trusted` rather than `is not trusted`, and it is the
same gate. After the review it lands:

```text
$ agentstack use --write
Activating toolset 'default' (scope: project) — 1 server, 1 skill
  ↩ undo: `agentstack x restore --last --write`

Claude Code
  · MCP servers are planned live (not connected), not written — nothing for `use` to render here
  ✓ 1 skill → ~/your-project/.claude/skills
✓ activated 'default' — wrote skills to 3 locations; no server configs changed.
```

### Delivery is routed, not chosen

Notice what `use` did not do: it wrote no MCP server config. Where each
capability lands is **not a mode you pick**. AgentStack routes every kind to
one of two lanes, per kind and per tool, from two facts — what the kind is,
and whether that tool speaks MCP.

- **The live lane** carries skills and MCP servers, served over the bridge to
  every tool that can take them, with no project artifacts written for them.
- **The rendered lane** carries house rules, settings, hooks and native
  extensions — those cannot travel live, because no live channel can vary a
  house rule per model, a settings file is only read as a file, and a hook
  runs code. A tool that reads files only gets every kind rendered.

Ask the product rather than guessing:

```text
$ agentstack x delivery
  Delivery  how capabilities reach each tool
  Claude Code          skills + MCP servers planned live (not connected) · house rules + settings + hooks written to files
                       hooks run code — reviewed in full every time
  Pi                   skills + house rules + settings + extensions written to files — this tool reads files only
  · register the bridge: agentstack x gateway connect --all --write
  · write files anyway: agentstack x delivery render-locally --write
```

Routing is the plan, not the delivery: a tool receives the live lane only
once its bridge is registered, which is why the line says *planned live (not
connected)* until you run `agentstack x gateway connect --all --write` — once
per machine, not once per project. There is exactly one override, and it goes
one way, towards files: `agentstack x delivery render-locally --write`. It is
recorded in the manifest, so every clone answers the same.

## Step 5 — Check it, then run

`agentstack status` is one screen: what is here, whether it is ready, and the
single next step. `agentstack doctor` is the deep check — adapters, secrets,
drift, skills, supply chain — and every finding names its own fix:

```text
$ agentstack doctor
Zero-files gateway
  ✗ no bridge for Claude Code, Codex CLI — nothing routed live is reaching them ↳ agentstack x gateway connect --all --write
  ✓ this project is trusted for auto mode
Secrets
  ✗ POSTGRES_URL         not found ↳ agentstack secret set POSTGRES_URL
Drift
  ✓ all targets in sync
2 errors, 0 warnings, 1 note.
  next: agentstack secret set POSTGRES_URL   the finding to start with
```

**Drift** is a mismatch between the manifest and what is on disk — usually a
hand-edit in a CLI's own config. `agentstack diff` shows the consequence
before anything changes, and then you answer one question: which side holds
the truth? If the hand-edit does, `agentstack adopt --write` pulls it into
the manifest (lifting any inline token on the way). If the manifest does,
`agentstack apply --write` re-renders over it. Either way, the setup changed,
so step 3 applies again: lock, then trust.

To launch a CLI as a tracked run:

```sh
agentstack run claude-code
```

That is the Protected tier by default — pre-launch gates for trust, lock and
policy, and a frozen tool surface. `--sandbox` moves it into a container
behind the egress proxy, `--lockdown` removes the container's direct route
out so the audited proxy is its only way to the network, and `--unprotected`
opts out of the gates entirely. Each tier prints its own posture label; the
four of them and what each actually enforces are in
[Concepts](concepts.md#posture-and-the-machine-policy-summary) and the
[enforcement matrix](ENFORCEMENT.md).

## Step 6 — Take it back, and set up the next machine

Rendered writes are recorded as you go, so nothing here is a one-way door.
`agentstack undo` is that record read as a timeline, newest first — with no
`--write` it only shows you the list:

```text
recent changes (newest first)
  1  2m ago      init         1 file · Claude Code, Codex CLI, OpenCode, Pi

  pick a point: agentstack undo --to <n> --write
```

`agentstack undo --to 1 --write` reverts to before that change — everything
newer comes off with it, because that is what "back to that point" means. The
revert is itself recorded, so going one step too far is recoverable.
`agentstack x restore --last --write` is the same record as a one-shot,
script-friendly command.

The limit is worth knowing: restore reverts AgentStack's own recorded writes.
A file some server deleted is not brought back, and a manifest edit you
regret is undone in your editor or in git, not here.

On a second machine that has the checkout and nothing else, there is nothing
to import — the setup already exists:

```sh
agentstack x up
```

It finds the CLIs on that machine, verifies the environment against
`agentstack.lock`, renders each CLI's config, and names what is left over,
which on a new machine is that machine's secrets. Trust is per person and per
machine: nobody else's yes carries over, so you review there too.

## Where to go next

- [Get started](start.md) — the same path, command by command, with expected output.
- [Concepts](concepts.md) — manifest, lockfile, trust, gateway, policy, posture, defined once.
- [Every command](reference.md) — the full surface, including everything under `agentstack x`.
- [Troubleshooting](troubleshooting.md) — when a command refuses and you want the reason.
- [Enforcement matrix](ENFORCEMENT.md) — what each protection really enforces, and where it stops.
