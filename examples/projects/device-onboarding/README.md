# device-onboarding — will setup work on *their* machine?

The onboarding matrix: every scenario is a fresh fake device (stripped PATH,
synthetic HOME, isolated AGENTSTACK_HOME) modeling a real user's starting
point — which CLIs they have, what their native configs already hold, and how
odd their environment is.

## What `assert.sh` proves

**A. CLI presence.** A device with zero CLIs gets the honest fallback and a
starter manifest, with `apply`/`doctor` still green. One CLI with an empty
config imports nothing and targets correctly. Three CLIs across three native
formats (Claude JSON, Codex TOML, Cursor JSON) import together — and an
inline bearer token is lifted to a `${REF}`: the manifest never holds the
plaintext, a blocked `apply` exits nonzero until the ref resolves, and a
server imported from one CLI fans out to the others.

**B. Config safety.** Conflicting definitions of the same server name are
surfaced, never silently picked. Re-`init` preserves a hand-edited manifest.
Hand-written `.mcp.json` entries and `CLAUDE.md` prose survive `apply` *and*
`restore` (which removes only the managed region), and the managed gitignore
never hides hand-written files. Pruning a de-manifested server keeps both the
still-managed and the hand-written entries. `apply` is idempotent,
`doctor --ci` is green after a write, and `restore` reverts it.

**C. Environment quirks.** Paths with spaces and unicode (through
`lock → trust → run --locked --plan`), the legacy root-manifest layout, a
project with no git, and an `AGENTSTACK_HOME` containing spaces — with the
guard still denying `.env` through it.

## Run it

```bash
cargo build --release            # or AGENTSTACK_BIN=/path/to/agentstack
bash examples/projects/device-onboarding/assert.sh
```

## Known gaps this example found (tracked, not asserted)

Filed as tasks from the first round — see `../FINDINGS.md`
("Device-onboarding round"): subdirectory walk-up for manifest discovery,
`adopt` on hand-*edited* values, project-scope pending-removal warnings, and
the `apply` default-scope vs. quickstart decision.
