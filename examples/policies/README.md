# Machine-policy presets

Four ready-to-use **machine policies** — the standing firewall AgentStack loads
from `~/.agentstack/agentstack.toml` and applies to every project on the
machine. The machine layer merges **under** each project's own `[policy]` with
**deny precedence**: a project can only narrow it, never loosen it. A cloned
repo therefore cannot escape what you set here.

Each file is a complete, parseable machine manifest (`version = 1` plus one or
more `[policy.*]` tables). They are validated by the CLI's own loader in
`crates/cli/tests/policy_presets.rs`, so what's here is exactly what
`agentstack` will accept.

| Preset            | Machine-policy summary | Use it when…                                             |
|-------------------|--------------|----------------------------------------------------------|
| `compatible.toml` | restrictive¹ | You've never set a machine policy; block only the obviously destructive, stay out of the way otherwise. |
| `developer.toml`  | restrictive  | Everyday dev box: destructive + secret-exfil denied, egress sharp edges blocked, sandbox writes allowed. |
| `locked-down.toml`| restrictive  | Deny-by-default everywhere; allow only what you name. Pair with `agentstack run --lockdown`. |
| `ci.toml`         | restrictive  | Unattended build agent: read + build tools, registry/model egress, no ambient secrets, read-only workspace. |

¹ All four read as **restrictive** in `agentstack doctor` because each uses at
least one rename-proof `"*"` rule. "restrictive" means *a `"*"` rule binds every
server* — not that the policy is tight; `compatible.toml`'s `"*"` is a short
deny-list, which is still broad. The doctor line never overstates.

## Install

```sh
# pick one, then:
cp examples/policies/developer.toml ~/.agentstack/agentstack.toml
agentstack doctor        # confirm it loads; read the "Machine policy" line
```

The layer is read once per gateway launch, so tightening it takes effect on the
next session. A valid load refreshes a secret-free last-known-good policy
snapshot. A later broken edit runs **DEGRADED** with that snapshot; broken
first-run state (or a corrupt snapshot) is **BLOCKED**, never project-only.
No machine manifest at all is the benign **UNCONFIGURED** state. `agentstack
doctor` names the state and repair details.

## The grammar (quick reference)

Full details in `docs/reference.md` (MCP firewall + machine layer). In short:

- **`[policy.tools]` / `[policy.egress]` / `[policy.secrets]`** map a **server
  name** to a list of glob patterns. Plain globs **allow**; a `!` prefix
  **denies**. Any allow pattern turns the list into an **allowlist** (must match
  an allow and no deny); with only deny patterns, everything else is allowed.
- The **`"*"` key is rename-proof** — it binds every server whatever a manifest
  calls it. A named key (`github = [...]`) only guards *your* naming, since a
  repo picks its own server names. Prefer `"*"` for machine-wide rules.
- **`[policy.egress]`** globs match hostnames, with an optional `:port` suffix
  (`api.anthropic.com:443` scopes the port; a bare host means any port).
- **`[policy.secrets]`** globs match `${REF}` names.
- **`[policy.filesystem]`** has `read` / `write` path globs and is
  manifest-global (no server key). The `write` scope is enforced in `--sandbox` /
  `--lockdown`: the workspace mounts **read-only unless** a write scope covers
  its root (`./**`). `read` scopes are informational today; **host-mode runs
  enforce neither** (policy is advisory outside the sandbox — the run banner and
  doctor say so).

## Editing safely

- Start from the closest preset and **add allows** as you hit real walls; that's
  how you learn what a workflow actually needs.
- Keep machine-wide rules under `"*"`. Use named-server keys only for servers
  whose names *you* control (profile/library servers) — `agentstack doctor`
  lints a named deny that has no `"*"` companion as rename-dodgeable.
- Grant secrets to **specific named servers in the project policy**, never
  broadly at the machine floor — that keeps the machine layer a true floor.
