# The dashboard — a read-only lens

The dashboard is the at-a-glance view of your stack for people who don't live in
a terminal. It is **read-only**: it shows state, previews diffs, runs doctor, and
watches live runs and audited calls — but it never writes. Every change happens
through the CLI. Where a control would live, the dashboard shows the exact command
to copy. Secret values never reach the browser.

## Launch

```sh
agentstack dashboard            # opens a token-gated, localhost-only view
agentstack dashboard --no-open  # print the URL, don't open a browser
```

It binds 127.0.0.1 only and prints a token-gated URL. The server exposes read
(GET) routes only — the snapshot, diff previews, doctor, runs, audited calls, and
search. There is no write endpoint at all: a POST to any path simply 404s. The
read-only property is a property of the router, not the UI, and a route-matrix
test pins it.

## First run — no manifest yet

On a machine with no `agentstack.toml`, the dashboard opens a welcome screen: it
lists the agent CLIs it detected and the MCP servers already in their configs, and
where those tools disagree today. To reverse-engineer a manifest from what's on
disk (lifting inline secrets into `${REF}`s), run the command it shows:
`agentstack init`.

## The tabs

- **Overview** — stat tiles, next-actions, stack summary, the zero-files bridge
  (connected harnesses + this repo's trust state), profiles, and usage. Each
  next-action links to the relevant tab or opens a read-only diff.
- **Runs** — live agent processes `agentstack run` launched, with uptime, profile,
  reachable capabilities, and per-run **Calls** (the audited tool-call footprint,
  digests only). Stop one with the shown `agentstack kill <id>`.
- **Discover** — search the embedded catalog and the official MCP Registry. Each
  result shows its trust signals and the `agentstack add <id>` command to add it.
- **Servers** — the cross-harness matrix: where each server is enabled, per CLI and
  scope (global/project switch at the top). Click a name for its config and the
  trust lens (**Explain trust ⓘ**). The **context** column shows each server's
  per-session token cost; click the header to sort.
- **Skills** — the same matrix for skills, plus skills discovered on disk but not in
  the manifest, each with the `agentstack adopt <name>` command to register it.
- **Settings** — each tool's current settings, read from its real config file, and
  which keys agentstack manages. Edit `[settings.<tool>]` in the manifest, then
  `agentstack apply --write`.
- **Hooks / Instructions / Extensions** — read-only inventories of lifecycle hooks,
  CLAUDE.md/AGENTS.md fragments, and content-pinned native harness add-ons.
- **Secrets** — every `${REF}` the manifest mentions, whether it resolves on this
  machine and from which layer (env / varlock / keychain / .env). Missing ones show
  the `agentstack secret set <REF>` command. Values are never shown.
- **Activity** — every apply, with the files it touched. Roll one back with the
  shown `agentstack restore`.
- **Health** — the standing summary plus **Run doctor**: the same checks as
  `agentstack doctor` (manifest validation, adapters, secrets, drift, quirks,
  skills, content scan, reproducibility, policy), rendered as the familiar ✓/⚠/✗
  report.
- **Proxy** — the wire lens, the same ranked report as `agentstack report wire`:
  per-turn tools and token weight, and a per-capability table so you can see which
  loaded tools earn their context. Observe-only.
- **Insights** — three read-only reports: **Optimize** (recommendations, each with
  its evidence and the exact command/TOML to act on it), **Analyze** (runtime call
  activity and library dead weight), and **Stats** (per-capability activations and
  context cost).

## Preview, then apply from the CLI

Anywhere drift exists — the pending bar, an Overview next-action, the Health tab —
**Review** opens a real diff of every native config that would change. It's a
viewer: it shows the diff and the `agentstack apply --write` command to reconcile
it. The write happens in your terminal.

The complete inventory is in [reference.md](reference.md).
