# Restricted folders

A runnable, recordable proof that AgentStack's **guard** keeps specific folders
off-limits to agents. The repo is a fake `acme-billing` service: agents may
read and edit `src/` and `docs/` freely, but `secrets/`, `personal-notes/`, and
`infra/prod/` are declared off-limits — and the guard refuses every read and
write to them, in whichever CLI dialect the harness speaks.

```bash
bash assert.sh                 # fast, asserting; exits nonzero on any mismatch
DEMO_PAUSE=2.5 bash assert.sh  # paced, for an asciinema recording
```

Requires `agentstack` on `PATH` (or `AGENTSTACK_BIN=/path/to/agentstack`, or a
built `target/release/agentstack` in this repo) and `python3`. It runs entirely
in an isolated temp `HOME`/`AGENTSTACK_HOME` — nothing touches your real config.

## The repo

A realistic repo tree with two allowed areas and three off-limits ones:

```
bundle/
  .agentstack/agentstack.toml   # declares the off-limits folders
  src/            index.ts, lib/invoice.ts   ← agents may read + edit
  docs/           architecture.md            ← agents may read + edit
  secrets/        api-keys.env, service-account.json   ← OFF-LIMITS (fake)
  personal-notes/ diary.md                             ← OFF-LIMITS (fake)
  infra/prod/     main.tf, variables.tf                ← OFF-LIMITS (fake)
```

The off-limits files hold **fake** content (`sk_live_FAKE_…`) — they exist only
to give the guard something concrete to refuse.

## How the guard decides

`agentstack guard` wires a **cooperative** pre-tool-use hook into the agent CLIs
that support one (Claude Code, Codex, Gemini, Cursor, Windsurf, Copilot CLI,
Antigravity, OpenCode, Pi; VS Code agent mode reads Claude-format hooks). Before
the harness runs a tool it hands the pending call to `agentstack guard check`,
which decides allow/deny from the machine's own config:

- `[policy.filesystem] deny` globs — never readable or writable. Each glob is
  matched against a path's **workspace-relative** form, its **absolute** form,
  AND its **bare file name**, so `secrets/**` blocks `secrets/api-keys.env`
  however the agent spells it.
- `[guard] allow_roots` — the write roots allowed beyond the workspace (empty
  here, so writes are confined to the workspace and temp dirs).

The effective deny set is the **union** of the machine manifest and the project
manifest — a repo can only ever *add* restrictions, never remove the machine's.

## What the demo proves

`assert.sh` feeds realistic pre-tool-use payloads into `guard check` and asserts
each outcome (`PASS`/`FAIL`, exits nonzero on any mismatch):

| pending tool call                       | outcome | why                                   |
|-----------------------------------------|---------|---------------------------------------|
| `Read secrets/api-keys.env`             | DENY    | off-limits folder                     |
| `Read secrets/service-account.json`     | DENY    | off-limits folder                     |
| `Write personal-notes/diary.md`         | DENY    | off-limits folder                     |
| `Read infra/prod/main.tf`               | DENY    | off-limits folder                     |
| `Read`/`Write src/index.ts`             | ALLOW   | allowed code — guard stays out of way |
| `Write /opt/acme/data/out.txt`          | DENY    | outside the workspace + allow_roots   |
| `Bash rm -rf .`                         | DENY    | deletes the workspace root            |
| `Bash ls src`                           | ALLOW   | an ordinary command                   |

It then proves three more things:

1. **Multi-CLI coverage.** The same policy is re-run through Codex's dialect
   (`--protocol codex`), which answers with the **same stdout decision envelope**
   as Claude: a deny is `hookSpecificOutput.permissionDecision = "deny"` on
   stdout with **exit 0** (the JSON body is the signal, not the exit code), and
   an allow exits 0 with no deny envelope.
2. **Denials are auditable.** Every refusal — reads, writes, and shell commands
   alike — is recorded to `$AGENTSTACK_HOME/audit/calls.jsonl` as a
   `host-guard` / `denied` entry. `guard status` reflects the live config.
3. **`guard test`** (the human entrypoint) exits nonzero on a denied command.

The script ends with `PASS`/`FAIL` assertions on every outcome and exits nonzero
if any fails, so it doubles as a CI-grade regression check.

## Project-layer deny enforces from both layouts (F1, resolved)

> **History (F1 / issue #11), fixed as of v0.15.0.** In the v0.10.1 baseline,
> `agentstack guard check` loaded the project manifest only from the legacy root
> `<repo>/agentstack.toml` and never consulted the `.agentstack/` subdirectory,
> so a project-layer `[policy.filesystem] deny` at the *preferred*
> `.agentstack/agentstack.toml` was silently ignored — only the machine layer
> enforced. That contradicted the "a repo can only *add* restrictions (union)"
> story in `CLAUDE.md` and `examples/guard-demo/README.md`, so the demo mirrored
> the three folder globs into the machine manifest as a workaround.

That gap is closed. `agentstack guard check` now resolves the project manifest
through the shared path logic, so a project-layer deny takes effect from **both**
layouts — the preferred `.agentstack/agentstack.toml` and the legacy root
`<repo>/agentstack.toml`. The demo's final section proves this directly with an
isolated sub-sandbox whose machine deny list is **empty**: a lone project-layer
deny at `.agentstack/agentstack.toml` blocks the read (`union works`), and a
control asserts the same deny at `<repo>/agentstack.toml` enforces too. Both are
`PASS`.

`assert.sh` still mirrors the three folder globs into the machine manifest for
the main body of the demo — that layer is the user's own machine floor and the
globs belong there anyway — but a repo no longer *needs* the machine layer for a
project-declared deny to bite.

## What this does NOT claim

The guard is **cooperative**, not kernel-enforced. It catches an agent's
*accidents* because the harness chooses to consult its own pre-tool-use hook. A
harness that ignores its hook protocol — or hostile code running by some other
path — bypasses it entirely. The kernel-enforced story is `agentstack run
--sandbox` / `--lockdown` (mask mounts, egress proxy). See
`examples/guard-demo/` for the destructive-command angle and
`docs/ENFORCEMENT.md` for the full claim discipline.
