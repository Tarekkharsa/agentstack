# Cloudflare house rules

> Unofficial, agentstack-authored. Not affiliated with or endorsed by Cloudflare.

Conventions to follow when working with Cloudflare (Workers, Pages, KV, R2, D1,
DNS) on the user's behalf.

## Configuration hygiene

- Keep `wrangler.toml` / `wrangler.jsonc` the source of truth. Every binding the
  Worker uses (KV, R2, D1, Queues, Durable Objects, service bindings, vars) must
  be declared there, not assumed.
- Always set `compatibility_date`; set `compatibility_flags` deliberately. Keep
  both under version control so deploys are reproducible.
- Use named environments (`[env.staging]`, `[env.production]`) and promote the
  same code across them rather than editing one shared config per deploy.

## Deploy conventions

- Build-check with `wrangler deploy --dry-run` before a real deploy; do this in
  CI on every PR.
- Name the target environment explicitly on deploy (`--env <name>`); never let
  ambiguity decide whether production ships.
- After deploying, verify with the live URL and `wrangler tail` before calling it
  done. Prefer `wrangler rollback` over hot-patching a bad deploy.

## Secrets & vars

- Set secrets only with `wrangler secret put`; store API tokens for the MCP/CLI
  in the user's secret manager. Plaintext `[vars]` are for non-sensitive config
  only.
- Scope Cloudflare API tokens to the minimum (account, zone, permissions) the
  task needs; do not reuse a Global API Key.

## Boundaries

- Never print, commit, log, or paste secret values, API tokens, or credentials
  into code, config, issue bodies, or chat.
- Do not run destructive or irreversible actions — deleting KV namespaces, R2
  buckets, D1 databases, Workers, Pages projects, or DNS records, or D1
  statements that DROP/DELETE data — without confirming the exact target and
  environment first.
- Never perform bulk deletes, bulk DNS changes, or bulk reassignments across
  resources without explicit confirmation.
- Confirm the environment before any action that touches production traffic, DNS,
  or live bindings.
