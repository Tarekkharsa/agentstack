# Example projects — realistic use cases, asserted

Seven self-contained fake-but-realistic projects, each exercising one agentstack
use case end-to-end the way a real user would. Every project has a README
explaining what it demonstrates and an `assert.sh` that proves it: isolated
temp `HOME` + `AGENTSTACK_HOME` (nothing touches your real config), PASS/FAIL
per claim, nonzero exit on any failure — safe to run unattended or in CI.

```bash
cargo build --release          # or AGENTSTACK_BIN=/path/to/agentstack
bash examples/projects/multi-cli-webapp/assert.sh
```

| Project | Demonstrates |
|---|---|
| [multi-cli-webapp](multi-cli-webapp/) | One manifest → Claude Code + Codex + Cursor: MCP server in three native shapes, house rules compiled into CLAUDE.md/AGENTS.md, a central-library skill referenced by name, secrets staying `${REF}` in the portable artifacts |
| [per-cli-instructions](per-cli-instructions/) | Instruction targeting: content only for Claude Code vs only for Codex from one manifest; hand-written prose survives the managed region; edit → re-lock → re-write loop |
| [policy-intersection](policy-intersection/) | The two-layer policy floor through the real gateway: a repo that tries to allow `delete_everything` and can't — invisible to discovery, refused with the machine layer named, audited in `calls.jsonl` |
| [restricted-folders](restricted-folders/) | `[policy.filesystem]` deny + guard hooks over a realistic tree (`src/`, `secrets/`, `infra/prod/`, `personal-notes/`): reads/writes into forbidden folders blocked across CLI hook dialects, allowed paths pass, denials audited |
| [skills-workout](skills-workout/) | Both skill delivery paths — static render (`use --write`) and the zero-files MCP lease (`lease_open`/`list_loadable`/`load`) — serve byte-identical content; profile fencing; never-clobber pruning |
| [locked-run](locked-run/) | The Protected host tier end-to-end: `--plan` mutates nothing; a clean `run --locked` freezes the grant and records evidence; the frozen bridge refuses mutating control-plane tools; a tampered artifact fails machine authentication; drift and a one-byte D3 executable edit refuse before launch; `--profile` fences the frozen grant |
| [device-onboarding](device-onboarding/) | The onboarding matrix on fake devices: zero/one/three CLIs across native formats; inline tokens lifted to `${REF}`s (never plaintext); conflicts surfaced; hand-written configs and prose survive apply/restore/prune; spaced + unicode paths, legacy layout, non-git, spaced machine home |

[FINDINGS.md](FINDINGS.md) is the dogfooding report these projects produced:
the skill-indexing investigation, the CLI-differences matrix, the device test
of `run --locked`, and the issues filed from what they caught.
