# Competitive Landscape

> Snapshot from a hands-on review of the two closest projects (repos cloned,
> read, and removed June 2026). Goal: know exactly what to build to be the
> **most full-featured, easiest-to-integrate, most powerful** tool in the space.

The "manage MCP + skills + instructions across every AI coding agent" space is
now crowded. Adjacent tools include SkillKit (28+ agents), skills-sync,
ai-config-sync-manager, mcporter, MCP Market Hub, and Skilldex (academic skill
package manager). The two that matter most are profiled below.

## The two closest projects

### amtiYo/agents — `@agents-dev/cli`
- **Stack:** TypeScript/Node (≥20), npm-distributed, ~v0.8.9.
- **Shape:** a focused, polished **sync tool**. Onboarding-first (`agents start`
  wizard, `agents watch` auto-sync). Narrow scope, broad reach.
- **Integrations (11):** Codex, Claude Code, Claude Desktop, Gemini CLI, Cursor,
  Copilot VS Code, Copilot CLI, Antigravity, Windsurf, OpenCode, Junie.
- Our closest *peer in spirit*.

### Microsoft/APM — Agent Package Manager
- **Stack:** Python/uv. A serious **dependency manager** ("npm/pip/Cargo for
  agent context"). `apm.yml` manifest + `apm.lock.yaml`.
- **Shape:** much larger scope — transitive deps, lockfile integrity hashes,
  marketplaces, pack/publish, content-security scanning, SBOM, policy hierarchy,
  a formal conformance spec, a GitHub Action.
- **Integrations (8):** Copilot, Claude, Cursor, OpenCode, Codex, Gemini,
  Windsurf, Kiro.
- Our closest *competitor in ambition* — the one to track.

## What they have that we do NOT

### From APM (the big gaps)
| Feature | What it is | agentstack |
|---|---|---|
| Transitive dependency resolution | Packages depend on packages; full tree resolved like npm | ❌ |
| Install from any git host | GitHub, GitLab, Bitbucket, Azure DevOps, Gitea, … | ⚠️ catalog + MCP Registry only |
| Pack & distribute / publish | `apm pack` → zip or standalone `plugin.json`; `apm publish` | ⚠️ planned (vendor-packs.md) |
| Content-security scanning | Hidden-Unicode / prompt-injection scan on every install; `apm audit` | ❌ (we plan the *gate*, not the scanner) |
| SBOM export | `apm lock export --format cyclonedx\|spdx` | ❌ (we have a lockfile to build on) |
| Marketplaces | Install from curated registries in one command | ❌ |
| Policy hierarchy | Tighten-only inheritance enterprise→org→repo + bypass contract + audit CI | ⚠️ flat `[policy]` + `doctor --ci` |
| Runtime provisioning | `apm runtime` manages node/python/docker runtimes for MCP | ❌ |
| Richer primitives | prompts, `.agent.md`/chatmodes, runnable prompts (`apm run`), commands | ⚠️ we have MCP/skills/instructions/hooks/settings |
| Conformance spec | `CONFORMANCE.json` — a formal standard others can conform to | ❌ (a standards-play moat) |
| CI/CD action | Official GitHub Action | ❌ |

### From amtiYo/agents (smaller, cheaper gaps)
| Feature | What it is | agentstack |
|---|---|---|
| Integration breadth | + Claude Desktop, Copilot VS Code, Copilot CLI, Antigravity, OpenCode, Junie | ⚠️ we have 6 |
| `watch` auto-sync | Re-syncs on source-file change | ❌ |
| Secret-arg inference | Auto-detects which CLI args are secrets, auto-splits to local override | ⚠️ we do secrets-by-ref, no auto-infer |
| Auto-trust | Writes `trust_level = "trusted"` into Codex config to pre-trust the project | ❓ verify |

## What we have that NEITHER has (the moat — keep leaning in)
- **Single static Rust binary, zero runtime deps.** APM needs Python+uv; amtiYo
  needs Node ≥20. Our strongest "easiest to integrate" card.
- **A real dashboard** (local web UI, cross-harness matrix). Neither has any GUI.
- **Secret resolution chain** (env → varlock → keychain → .env) — richer than
  amtiYo's local-override file or APM's token manager.
- **Native settings management** (`[settings.<cli>]`: permissions, feature flags,
  hooks per CLI). Both competitors manage only *context primitives* — neither
  touches the CLI's own settings. Uniquely ours.
- **`explain` trust lens**, `adopt`/`restore`, `stats`, profiles / selective
  skill loading.

## Prioritized roadmap to "full-featured + easiest + most powerful"

1. **Integration breadth (cheapest, highest visible ROI).** Adapters are YAML
   data descriptors, so adding **Claude Desktop, Copilot (VS Code + CLI),
   OpenCode, Antigravity, Junie, Kiro** is mostly data, not code. 6 → 12
   leapfrogs amtiYo and matches APM on coverage. Do first.
2. **Transitive dependencies + install-from-any-git-host.** APM's core power
   feature; directly upgrades vendor-packs (a pack depends on packs, resolved
   from any git host, pinned in the lockfile). Without it we look like a syncer,
   not a package manager.
3. **Content-security scanning on install** (hidden-Unicode / prompt-injection) +
   an `audit` command. The missing half of the vendor-packs Phase 3 trust gate —
   APM scans, we only gate. Pair them.
4. **`publish`/`pack`** (already planned). Consider emitting standard
   `plugin.json` for interop with the Copilot/Claude plugin ecosystems.
5. **SBOM export from the lockfile** — cheap, enterprise-friendly, lockfile
   already exists.
6. **`watch` auto-sync** — small lift, DX parity with amtiYo.
7. **Policy hierarchy** (tighten-only enterprise→org→repo) — extends `[policy]`
   into team/enterprise territory.
8. Lower priority / bigger lifts: marketplaces, runtime provisioning,
   prompts/agents primitives, a conformance spec.

**Bottom line.** APM is the real threat — broader and more rigorous, but
heavyweight (Python runtime, enterprise-flavored, complex). Our wedge is the
same power in **one local zero-dependency binary, with a dashboard and
native-settings control no one else has.** Items 1–4 reach parity on what
matters while keeping the moat (binary + dashboard + settings) they can't easily
copy.
