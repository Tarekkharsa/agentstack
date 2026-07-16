# Restricted folders

A runnable, recordable proof that AgentStack's **guard** keeps specific folders
off-limits to agents. The bundle is a fake `acme-billing` service: agents may
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

## The bundle

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
   (`--protocol codex`), where a deny is a **stderr reason + exit code 2** (not a
   JSON body). Claude's dialect answers a deny as a JSON envelope on stdout.
2. **Denials are auditable.** Every refusal — reads, writes, and shell commands
   alike — is recorded to `$AGENTSTACK_HOME/audit/calls.jsonl` as a
   `host-guard` / `denied` entry. `guard status` reflects the live config.
3. **`guard test`** (the human entrypoint) exits nonzero on a denied command.

The script ends with `PASS`/`FAIL` assertions on every outcome and exits nonzero
if any fails, so it doubles as a CI-grade regression check.

## Known limitation (F1) — why the deny list is mirrored into the machine layer

The repo declares its off-limits folders in `.agentstack/agentstack.toml`, the
documented preferred manifest location. **Today the guard does not enforce a
deny list from that location.** `agentstack guard check` loads the project
manifest with a helper that looks for `<repo>/agentstack.toml` (the legacy root
path) and never consults the `.agentstack/` subdirectory, so a project-layer
`[policy.filesystem] deny` at `.agentstack/agentstack.toml` is **silently
ignored** — even after `lock` and `trust`. Only the machine layer enforces.

That contradicts the "a repo can only *add* restrictions (union)" story in
`CLAUDE.md` and `examples/guard-demo/README.md`. Until it is fixed, restricted
folders must be declared in the **machine** manifest
(`~/.agentstack/agentstack.toml`) to take effect — which is exactly what
`assert.sh` does (it mirrors the three folder globs there). The demo also
includes an isolated **probe** that demonstrates F1 directly: a lone
project-layer deny is checked, prints a loud `SKIP`, and a control assertion
proves the *same* deny **does** enforce when placed at `<repo>/agentstack.toml`
— isolating the bug to `.agentstack/` path resolution, not the deny engine.

Once F1 is fixed, the probe flips from `SKIP` to `PASS` and the repo's own
declaration will enforce on its own.

## What this does NOT claim

The guard is **cooperative**, not kernel-enforced. It catches an agent's
*accidents* because the harness chooses to consult its own pre-tool-use hook. A
harness that ignores its hook protocol — or hostile code running by some other
path — bypasses it entirely. The kernel-enforced story is `agentstack run
--sandbox` / `--lockdown` (mask mounts, egress proxy). See
`examples/guard-demo/` for the destructive-command angle and
`docs/ENFORCEMENT.md` for the full claim discipline.
