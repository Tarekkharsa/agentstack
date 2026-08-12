<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Tutorial: one library, two projects, another machine

This tutorial shows the complete everyday workflow. It assumes AgentStack is
installed and `agentstack init` has completed on this machine.

## Step 1 — Link your reusable library

```bash
agentstack lib link ~/GitHub/ai-setup --name central --first
agentstack lib link ~/GitHub/ai-setup --name central --first --write
agentstack lib sources
```

The first command previews. The linked folder can be any normal Git checkout.
`lib sources` shows this source together with the built-in local library and
reports names that appear in more than one place.

## Step 2 — Put reusable capabilities there

Create or import a skill:

```bash
agentstack lib new api-review
agentstack lib add ./api-review
agentstack lib add ./api-review --write
```

Add an MCP server definition containing `${REF}` placeholders, never secret
values:

```bash
agentstack lib add-server github --file ./github-server.toml
agentstack lib add-server github --file ./github-server.toml --write
agentstack lib list
```

Commit and push the library as you would any Git repo.

## Step 3 — Select names in a project

Create `.agentstack/agentstack.toml` with one default toolset:

```toml
version = 1
default_toolset = "backend"

[toolsets.backend]
servers = ["github"]
skills = ["api-review"]
```

The project stores names, not copied skill folders or MCP configs. If your
project already contains several capabilities, the command form is:

```bash
agentstack toolset create backend --server github --skill api-review
agentstack toolset default backend
agentstack toolset default backend --write
```

`toolset create` is interactive at a terminal. The default command has an
explicit preview and write form.

## Step 4 — Pin and review it

```bash
agentstack lock          # preview
agentstack lock --write
agentstack trust .
agentstack status
```

The lock freezes the exact library content this project selected; `trust .` is
the local human review of it, so every machine reviews for itself. See
[Trust a project](howto/trust-a-repo.md) for why the lock comes first.

## Step 5 — Start your agent normally

Open Codex, Claude Code, OpenCode, or another supported CLI as usual. You do
not need to run `agentstack use` for a live-capable CLI.

A trusted connection automatically opens the project's default toolset. The
agent sees skill names and descriptions first, so “review this API change” can
make it load `api-review` without the user typing that name — the description is
the routing hint. See
[Dynamic skill loading](concepts.md#dynamic-skill-loading).

If no default exists and several toolsets are declared, the gateway stays on
control-plane tools until the agent asks which toolset to use. A modern
connection picks up a changed default on its next request; only a legacy
connection keeps its opening selection and needs a reconnect.

## Step 6 — Use the same library in another project

The second project references the same reusable names but chooses a different
small set:

```toml
version = 1
default_toolset = "docs"

[toolsets.docs]
servers = ["upstash/context7"]
skills = ["technical-writing"]
```

Run the same lock and trust loop. The two projects share the library but keep
independent locks, defaults, and trust decisions.

## Step 7 — Bootstrap another machine

Clone the project, then preview and apply one bootstrap:

```bash
agentstack up --library https://github.com/you/ai-setup.git
agentstack up --library https://github.com/you/ai-setup.git --write
agentstack status
```

AgentStack detects the CLIs present on that machine and connects only what it
finds. The project and library content travel; secrets and trust do not.

Add each missing secret locally:

```bash
agentstack secret set GITHUB_TOKEN
```

Then run the `trust .` command named by status.

## Update something later

Edit and push the central library. In a project that should accept the new
version:

```bash
agentstack up            # preview
agentstack up --write
agentstack lock          # preview
agentstack lock --write
agentstack trust .
```

The important guarantee is that syncing the library does not silently change a
locked project. The `lock` preview is where you see and accept the change.

## Take back a managed write

```bash
agentstack undo
```

Choose a recorded point and add the exact `--write` command it shows. Removing
an AgentStack-managed setup also removes empty managed parent folders; it keeps
user-owned files and non-empty folders.

Next: [Get started](start.md) · [Central library](library.md) ·
[FAQ](faq.md) · [Full reference](reference.md)
