# guard-demo — the destructive-command guard, as a proof

`agentstack guard` wires a **cooperative** pre-tool-use hook into the agent CLIs
that support one (Claude Code, Codex, Gemini, Cursor, Windsurf, Copilot CLI,
Antigravity, OpenCode, Pi; VS Code agent mode reads Claude-format user hooks).
Before the harness runs a tool, it hands the pending call to `agentstack guard
check`, which decides allow/deny from the machine's own config — and records
every denial to `~/.agentstack/audit/calls.jsonl` as a `host-guard` entry.

`run-demo.sh` spins up an isolated `HOME`/`AGENTSTACK_HOME`, writes a machine
manifest with `[guard] enabled` and a `[policy.filesystem] deny` list, then
feeds realistic Claude-Code-format pre-tool-use payloads into `guard check` and
asserts each outcome:

| pending tool call            | outcome  | why                                        |
|------------------------------|----------|--------------------------------------------|
| `rm -rf /opt/acme/data`      | BLOCKED  | a write outside the workspace              |
| `git reset --hard HEAD~3`    | BLOCKED  | discards uncommitted work irrecoverably    |
| `cat .env`                   | BLOCKED  | a `[policy.filesystem]` deny glob          |
| `ls -la`                     | ALLOWED  | the guard stays out of the way             |

It finishes by grepping the audit log to prove the three denials were recorded.
Like the other examples it prints `PASS`/`FAIL` and exits nonzero on any
mismatch, so it runs unattended in CI.

## Run it

```sh
# uses target/release/agentstack if present, else builds it (minutes),
# else falls back to `agentstack` on PATH
./run-demo.sh

# override the binary
AGENTSTACK_BIN=/path/to/agentstack ./run-demo.sh

# pace it for a screen recording (asciinema)
DEMO_PAUSE=2.5 ./run-demo.sh
```

## What this proves

- The guard blocks a real set of destructive shell commands (`rm -rf` outside
  the workspace, `git reset --hard`) and reads of a machine-denied path
  (`cat .env`) — while leaving ordinary commands untouched.
- The decision comes from the **machine's** config (`~/.agentstack/agentstack.toml`),
  which no cloned repo can loosen; a project manifest may only *add* deny globs.
- Every denial is written to the audit log as a `host-guard` entry, so blocks
  are observable after the fact.

## What this does NOT prove (read this)

The guard is **cooperative**, not enforced. Be precise about its limits:

- **It catches accidents, not malice.** It works because the harness *chooses*
  to consult its own pre-tool-use hook before acting. A harness that ignores
  its hook protocol — or hostile code running by some other path — bypasses the
  guard entirely.
- **Harnesses without a hook surface are uncovered.** The guard can only wire
  into CLIs that expose a pre-tool-use hook. Anything else on the machine runs
  unchecked.
- **The command tokenizer is conservative, not a shell.** It judges a bounded
  set of destructive shapes; it does not emulate the shell, so obfuscated or
  novel constructions can slip past.

For **kernel-enforced** confinement — where the boundary holds regardless of
whether the code cooperates — use `agentstack run --sandbox` (Docker isolation)
or `--lockdown`. That is the enforced primitive; the guard is the everyday
accident net that runs on the host with no container. See `docs/ENFORCEMENT.md` for
the full matrix of which dimensions are enforced, cooperative, or coarse under
each mode.
