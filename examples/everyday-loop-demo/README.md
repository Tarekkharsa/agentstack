# Everyday-loop demo — yes · undo · share · receive · up

The five verbs strategy v2 added, proven end to end on two isolated machines.
**All five require v0.18.0 or newer.**

`first-value-demo` proves the *import* story: many CLIs, one manifest. This one
proves the loop you live in afterwards — drop a file in, activate it, take it
back, hand it to someone else, and have their machine come up on one command.

1. **Start** from a project with one manifest and a `.mcp.json` it wrote by
   hand, months ago, outside AgentStack.
2. **Drop** a skill folder into `.agentstack/skills/`, then activate it. The
   skill is declared, pinned in `agentstack.lock`, and — because this demo asks
   for files with `use --write` — rendered into `.claude/skills/`, where the CLI
   reads skills off disk. That render is the ask, not the automatic path:
   delivery is routed, and on an MCP-capable tool skills are served live over a
   lease instead (`agentstack delivery` prints the routing;
   `examples/projects/skills-workout` proves both lanes deliver identical
   bytes). The rendered lane is the right one here — `undo`, `share`,
   `receive`, and `up` are all about files, and files are what this demo
   compares byte-for-byte.
3. **Undo** with `agentstack undo --to 1 --write`: `.mcp.json` returns
   byte-for-byte to where it started, hand-written server and all — while the
   dropped file itself is left alone.
4. **Share** with `agentstack share`: a signed `.astack` bundle carrying the
   manifest, the lock, and the pinned skill content.
5. **Receive** on a second machine — its own `HOME`, its own `AGENTSTACK_HOME`,
   its own trust store. The bundle is staged inert; nothing activates until the
   receiver decides. Then `agentstack up` verifies against the lock and renders
   that machine's native config.

## Run it

```sh
./run-demo.sh
```

Self-contained: two isolated temp `HOME`s and `AGENTSTACK_HOME`s, a stub
`claude` on a controlled `PATH`, nothing touches your real configuration, and
the sandbox is deleted on exit. Every step asserts an **on-disk effect** — file
contents, `cmp` against a byte-exact snapshot, JSON fields read out of the
bundle — never a command's own summary line. The script exits nonzero on any
mismatch, so it is a CI-runnable witness that the loop keeps working against
the current binary.

## What each step asserts

| Step | Assertion |
| --- | --- |
| `yes` (headless) | refuses without a terminal, names the explicit path, and leaves **nothing** declared or pinned |
| activate | the skill is in the manifest, in the lock, and readable at `.claude/skills/sql-review/SKILL.md` |
| activate | the render merged the managed server in beside the hand-written one |
| `undo --to 1 --write` | `.mcp.json` is byte-identical to the pre-activation snapshot; the dropped source file is untouched |
| `share` | the `.astack` exists and carries a publisher key, a signature, the manifest and the lock |
| `receive --yes` | the skill lands byte-identical, nothing is active, and the bundle's manifest is **not** merged |
| `up` | reports the harnesses found, verifies the skill source against the lock, and renders `.mcp.json` |

## Two things this demo does not pretend

**`agentstack yes` is TTY-only, by design.** `--yes` answers the confirmation
prompt; it is not a substitute for the terminal. A headless caller is refused
outright — letting a flag alone assert a review nobody was shown is exactly
what the consent design forbids (`crates/cli/src/commands/yes.rs`). So this
demo does not fake a terminal. It **asserts the refusal** — that gate is a
security property worth a witness — and then runs the explicit four-step path
the error message itself names:

```sh
agentstack adopt --write
agentstack lock
agentstack trust --yes --consented-digest "$(agentstack trust --preview | ...)"
agentstack use --write
```

`yes` collapses those four into one reviewed step for a human at a terminal. It
does not do anything different, so the effects asserted here are the effects
`yes` produces.

**`share` signs *inside* the bundle.** There is no detached `.sig` sidecar next
to the `.astack` — the publisher key and signature are fields within the bundle
itself, and that is what the demo reads. (`agentstack sign` writes a detached
`agentstack.lock.sig`; that is a different artifact for a different job.)

One further note on `up`: it renders each CLI's *config* and verifies the
environment against the lock, but materializing skill folders remains
`agentstack use --write`'s job. The demo runs both on the second machine and
asserts the received skill is live there, byte-identical to the original.

## Record it

vhs stalls on this machine; use asciinema:

```sh
DEMO_PAUSE=2.5 asciinema rec everyday-loop.cast --window-size 108x30 -c ./run-demo.sh
```

`DEMO_PAUSE` paces the narration lines for a watchable recording; the default
(0.6s) is for humans running it live, and `DEMO_PAUSE=0` for CI.
