# docs/archive/

Superseded records, kept for lineage. **History, never direction.** The
operative documents are [`STRATEGY.md`](../../STRATEGY.md) for direction,
[`TODO.md`](../../TODO.md) for the work queue, and
[`ARCHITECTURE.md`](../ARCHITECTURE.md) / [`ENFORCEMENT.md`](../ENFORCEMENT.md)
for what the code does and enforces. Consult this directory (or git history)
only when researching how something came to be; do not treat anything here as a
second roadmap.

**One exception, and it is why this directory is not simply deleted.** A few
files here are still *cited* by the current documentation, because they hold
material the operative pages deliberately do not restate — a threat model's
residual-risk column, an accepted ADR's rationale, operational field notes. A
citation from a current page is what keeps a file here tracked:

| File | Cited by | What it carries |
|---|---|---|
| `design/reference-field-notes.md` | `docs/reference.md` (8×) | Operational edge cases and crate-level caveats behind the feature reference |
| `design/tools-execute-threat-model.md` | `docs/ARCHITECTURE.md`, field notes | Threats, mitigations and **residual risks** for the experimental execution boundary |
| `design/adr-tools-execute-runtime.md` | `docs/ARCHITECTURE.md`, field notes | The accepted runtime/ownership decision, including the prohibited `executor → policy` edge |
| `design/workflows-capability.md` | `docs/workflows.md` | The implemented authoring, authority and evidence contract |
| `design/workflow-scaling.md` | `docs/workflows.md` | Active scaling design, and the two things it deliberately did not build |
| `design/consent-card.md` | `docs/design/automatic-delivery.md` | The consent-card contract, cited normatively |
| `design/ui-control-plane.md` | `docs/design/automatic-delivery.md` | The panel authority boundary, cited normatively |
| `design/t3code-mcp-bridge-research.md` | `design/ui-control-plane.md` | Bridge research the boundary document builds on |

Everything else that once lived here is untracked: still on the maintainer's
disk and in git history, but no longer published, because nothing current
depends on it. Removing a citation above is what makes a file free to go the
same way.
