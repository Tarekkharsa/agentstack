# Design: source-aware secret rendering (capability model)

Status: design only — nothing here is implemented. Written 2026-07-10 after an
external review of the "emit `${REF}` instead of literals where the target
supports it" proposal. The proposal is directionally right and currently
over-simplified; this document pins the model that would make it safe.

## Today

Every adapter declares `secret_mode: literal`: `apply` resolves `${REF}`s and
writes the values into native configs (atomic, backed up, gitignored by
default). The renderer already has a `passthrough` mode
(`crates/adapters/src/render.rs`) but no adapter uses it. The gateway path avoids the
problem entirely — secrets resolve at call time and never touch disk — which
is one reason it's the preferred mode.

## Why "env source ⇒ reference, keychain ⇒ literal" is not enough

1. **Launch-environment mismatch.** agentstack resolving `GH_PAT` from its own
   process env (or varlock, or a `.env` it read) says nothing about whether
   the *target CLI's* later launch will have that variable. A rendered
   `${GH_PAT}` that the CLI expands at ITS startup fails in every terminal
   that didn't export it. Emitting a reference is only correct when the
   variable's availability at CLI-launch-time is the user's explicit contract
   — which is a per-machine decision, not derivable from where agentstack
   happened to find the value.

2. **Target capabilities are field-specific, not boolean.** Verified against
   official docs (2026-07):
   - **Claude Code** `.mcp.json`: generic `${VAR}` / `${VAR:-default}`
     expansion in `command`, `args`, `env`, `url`, `headers`. A rendered
     reference is *exactly equivalent* to the manifest's intent. Full
     passthrough is possible.
   - **Codex** `config.toml`: NO generic interpolation. Field-specific
     mechanisms only: `env_vars` (forwards a same-named local env var),
     `bearer_token_env_var` (Authorization bearer), `env_http_headers`
     (header name → env var name). Crucially there is no rename: the manifest
     idiom `env = { GITHUB_TOKEN = "${GH_PAT}" }` CANNOT be expressed — the
     server reads `GITHUB_TOKEN`, Codex would forward `GH_PAT`. Only the
     same-name case (`env = { GH_PAT = "${GH_PAT}" }`) maps to
     `env_vars = ["GH_PAT"]`.
   - Other adapters: unaudited; assume literal until verified.

## The model

Extend the adapter descriptor with per-field secret capabilities:

```yaml
mcp:
  secret_capabilities:
    env: interpolate          # generic ${VAR} in env values (claude-code)
    url: interpolate
    headers: interpolate
    command: interpolate
    args: interpolate
# vs codex:
    env: forward-same-name    # env_vars — only when manifest name == ref name
    headers: env-map          # env_http_headers
    bearer: env-var           # bearer_token_env_var, Authorization only
    url: none
    command: none
    args: none
```

Rendering rule: **emit a reference only when the native representation is
exactly equivalent to the manifest's declared intent; otherwise fall back to
literal.** Equivalence is judged per (adapter, transport, field, shape):

- `interpolate` fields: any `${REF}` passes through verbatim.
- `forward-same-name`: pass through only if the env KEY equals the REF name.
- `env-map` / `env-var`: rewrite to the field-specific mechanism only for the
  exact shapes those mechanisms cover (a bearer Authorization header; a
  header-per-env-var map).
- `none`, or any shape outside the above: resolve to literal (today's
  behavior), never guess.

Additionally, reference emission is **opt-in per machine** (a setting, not a
default) until launch-env verification exists, because of problem 1. A doctor
check should verify referenced vars are present in the login shell env
(best-effort) and warn otherwise.

## Phasing

1. **Design sign-off** (this doc).
2. **Claude Code passthrough** behind an opt-in flag: all five fields are
   `interpolate`, so the change is small — but ship only with tests that
   launch Claude with/without the variable exported to pin the failure mode
   and the doctor warning.
3. **Codex safe subset**: `env_vars` for same-name env entries,
   `bearer_token_env_var` for `Authorization: Bearer ${REF}` headers,
   `env_http_headers` for other header refs. Everything else stays literal.
4. **Audit the remaining 11 adapters** before granting any of them
   capabilities (each claim needs the official doc citation in the
   descriptor comment).

## Non-goals

- Changing the gitignore-by-default posture: rendered artifacts stay
  machine-local; the manifest + lock remain the committed source of truth.
- Inventing interpolation for targets that lack it (no wrapper scripts that
  inject env — that changes the execution model and belongs to a different
  discussion).
