# device-onboarding — will setup work on *their* machine?

The onboarding matrix: every scenario is a fresh fake device (stripped PATH,
synthetic HOME, isolated AGENTSTACK_HOME) modeling a real user's starting
point — which CLIs they have, what their native configs already hold, and how
odd their environment is.

## This fixture rides the RENDERED lane, on purpose

Delivery is routed, not chosen. Under the default (Automatic) routing an
MCP-capable harness is served its servers **live** through the gateway, so a
project holds only `.agentstack/` and no native MCP file is written at all.
That is the right default — and it is not what this fixture is for.

Almost every assertion below reads a *rendered native file*: three native
formats side by side, a render that must land at the project root and not in a
nested subdirectory, a spaced path, a unicode path, the legacy root layout, a
non-git project. So every manifest `assert.sh` writes carries the single
supported override:

```toml
[delivery]
render_locally = true
```

(`agentstack delivery render-locally --write` records the same block.) Without
it these scenarios would not fail loudly — they would pass *vacuously*, because
there would be no file to contradict them. The live lane has its own witnesses
elsewhere; this one is the file-rendering matrix.

One consequence worth knowing: on the default routing `doctor --ci` is red on a
device with no bridge registered ("no bridge for Claude Code — nothing routed
live is reaching it"). Opting into the rendered lane is what makes B6's
`doctor --ci` green, because nothing is waiting on a bridge any more.

## What `assert.sh` proves

**A. CLI presence.** A device with zero CLIs gets the honest fallback and a
starter manifest, with `apply`/`doctor` still green. One CLI with an empty
config imports nothing and targets correctly. Three CLIs across three native
formats (Claude JSON, Codex TOML, Cursor JSON) import together — and an
inline bearer token is lifted to a `${REF}`. Import is library-first, so the
definition (and the `${REF}`) lands in the library while the manifest only
references the server by name and holds no secret material at all; a blocked
`use` exits nonzero until the ref resolves, and a server imported from one CLI
fans out to the others.

**B. Config safety.** Conflicting definitions of the same server name are
surfaced, never silently picked. Re-`init` preserves a hand-edited manifest.
Hand-written `.mcp.json` entries and `CLAUDE.md` prose survive `apply` *and*
`restore` (which removes only the managed region), and the managed gitignore
never hides hand-written files. Pruning a de-manifested server keeps both the
still-managed and the hand-written entries. `use` is idempotent,
`doctor --ci` is green after a write, and `restore` reverts it.

**C. Environment quirks.** Paths with spaces and unicode (through
`lock → trust → run --locked --plan`), the legacy root-manifest layout, a
project with no git, and an `AGENTSTACK_HOME` containing spaces — with the
guard still denying `.env` through it.

**D. Discovery & adoption.** From a nested `src/deep/` subdirectory, bare
`agentstack`, `doctor`, and `apply` all walk up to the project root (the render
lands at the root, never nested), and a nested `init` refuses to silently
create a second manifest. `adopt` pulls a hand-*edited* field of a
manifest-known server into the manifest, and the adopted value survives the
next `apply`.

## Run it

```bash
cargo build --release            # or AGENTSTACK_BIN=/path/to/agentstack
bash examples/projects/device-onboarding/assert.sh
```

## What the first round found (now fixed and asserted)

The first round of this example surfaced four gaps — subdirectory walk-up for
manifest discovery, `adopt` on hand-*edited* values, project-scope
pending-removal warnings, and the `apply` default-scope vs. quickstart
decision. All four are now fixed and covered above (sections A, B, and D); the
original filings are in the dogfooding report, kept in git history.
