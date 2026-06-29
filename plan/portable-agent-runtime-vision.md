# AgentStack Vision Plan: Portable Agent Runtime

Date: 2026-06-29

## North Star

AgentStack should become the portable operating layer for AI coding agents:

> One reviewed, versioned workspace spec that lets a developer or team run the
> same agent capabilities across Claude Code, Codex, Cursor, VS Code, Gemini,
> OpenCode, and other harnesses, on any machine, without leaking secrets or
> hand-rebuilding setup.

The project should not be positioned as only "sync MCP config files." The
stronger product is a reproducible, auditable, cross-harness agent environment:

- Portable across computers.
- Reviewable in git.
- Safe with local secrets.
- Able to launch agents with explicit capability profiles.
- Able to install vendor/team capability packs with clear trust signals.
- Able to prove that setup is healthy through `doctor`, tests, and CI.

## Product Promise

A repo can declare its agent setup once:

```toml
[profiles.backend]
servers = ["github", "postgres", "linear"]
skills = ["api-design", "sql-review"]

[profiles.frontend]
servers = ["figma", "linear"]
skills = ["pixel-perfect", "frontend-review"]
instructions = ["frontend-rules"]
```

Then any developer can run:

```bash
git clone <repo>
agentstack bootstrap
agentstack doctor
agentstack run codex --profile backend
agentstack run claude-code --profile frontend
```

The machine-local part is secrets and installed native apps. The committed part
is the manifest, lockfile, skills, instructions, policies, and adapter intent —
all kept together under a single `.agentstack/` folder at the repo root (legacy
root-level `agentstack.toml`, `agentstack.lock`, `skills/`, and `instructions/`
are still discovered for back-compat).

## Target Users

Strong first users:

- Developers using two or more AI coding harnesses.
- Teams standardizing AI setup per repo.
- DevEx/platform teams shipping project-specific agent capabilities.
- Consultants moving across clients and machines.
- Security-conscious teams that want agent capability changes reviewed in git.
- Vendors that want "continue in your own agent" without owning the user's agent.

Weak first users:

- Single-agent users with only one or two simple MCP servers.
- Users expecting a polished public marketplace before trust and portability are
  solved.
- Teams that already manage everything with dotfiles and do not care about
  agent-specific audit semantics.

## Core Workflows

### 1. New Machine Bootstrap

Goal: make a fresh laptop or devcontainer usable quickly without copying secrets
or manually editing multiple agent configs.

Desired flow:

```bash
git clone <repo>
agentstack bootstrap
agentstack secret set GITHUB_TOKEN
agentstack secret set LINEAR_PACK_TOKEN
agentstack doctor --live
agentstack apply --write
```

Must-have behavior:

- Shows which harnesses are installed.
- Shows which configured harnesses are missing.
- Installs or verifies skill sources.
- Explains every missing secret by name and source.
- Blocks writes if required secrets are unresolved by default.
- Shows exactly which files would be written.
- Leaves user-owned config untouched.
- Produces a clear "ready" state.

Checklist:

- [ ] `bootstrap` has one obvious happy path for a repo checkout.
- [ ] `bootstrap` can run read-only as a preflight.
- [ ] `bootstrap --write` only writes after showing target files.
- [ ] `doctor --ci` can gate a repo setup in CI.
- [ ] Docs include "new machine" and "new teammate" recipes.

### 2. Shared Team Setup

Goal: make agent capabilities a normal code-reviewed repo artifact.

Desired repo files:

```text
.agentstack/agentstack.toml
.agentstack/agentstack.lock
.agentstack/agentstack.md
.agentstack/skills/
.agentstack/instructions/
.github/workflows/agentstack-doctor.yml
```

(Legacy root-level `agentstack.toml`, `agentstack.lock`, `skills/`, and
`instructions/` are still discovered, so existing repos keep working until they
migrate.)

Rules:

- Manifest and lockfile are committed.
- Secrets are never committed.
- Team-owned skills and instructions can live in the repo.
- Optional vendor packs must be visible in the manifest and lockfile.
- Changes should be easy to review as diffs.

Checklist:

- [ ] `agentstack init` can import existing local setup cleanly.
- [ ] `agentstack adopt` can pull hand-added server config back into the repo.
- [ ] `agentstack diff` is readable enough for code review.
- [ ] `agentstack.lock` covers every fetched or bundled skill.
- [ ] Policy can require or forbid capabilities per repo.
- [ ] A CI sample is documented.

### 3. Runtime Control

Goal: AgentStack does not only configure agents; it launches and tracks them with
known capability boundaries.

Desired flow:

```bash
agentstack run claude-code --profile design
agentstack run codex --profile backend --scope project
agentstack runs
agentstack kill <id>
```

The dashboard should answer:

- Which agents are running?
- Which profile is each run using?
- Which servers, skills, instructions, hooks, and settings can that run access?
- Which files were changed for that run?
- Can this run be killed and reverted cleanly?

Checklist:

- [x] Foreground tracked runs exist.
- [x] Runs can be listed and killed.
- [x] Profile-bound runs revert on exit.
- [ ] Dashboard shows exact trust footprint per run.
- [ ] Dashboard distinguishes permanent apply from temporary run/session apply.
- [ ] Run registry cleanup is documented.
- [ ] Windows behavior is either supported or clearly excluded.

### 4. Vendor And Team Packs

Goal: install a useful capability as one unit: MCP server, skills, instructions,
hooks, and settings, with trust signals.

Desired flow:

```bash
agentstack search linear
agentstack add from linear-pack --write
agentstack add from linear-pack --with-instructions --write
agentstack upgrade linear-pack --write
agentstack remove linear-pack --write
```

A pack must answer:

- What server does this add?
- Does it run local code?
- What secrets does it need?
- What skills and instructions does it install?
- Who authored it?
- Is it official, community, or agentstack-authored?
- What changed during upgrade?
- Can it be removed and reinstalled cleanly?

Checklist:

- [x] Pack model exists.
- [x] Starter packs exist for Linear, Cloudflare, and PostHog.
- [x] Instructions are opt-in and provenance-stamped.
- [x] Upgrade command exists.
- [x] `remove <pack>` deletes pack-owned skill assets (contained, prunes empty parents).
- [x] `upgrade <pack>` has containment guards for skill paths.
- [x] README commands use the real CLI syntax: `agentstack add from <pack> --write`.
- [ ] Packs have stronger authorship and trust metadata.
- [ ] Pack upgrade can resolve from a versioned or remote source.
- [~] Tests cover remove/reinstall (linear-pack covered; cloudflare/posthog cover install+remove, not yet reinstall).

### 5. Cross-Harness Adapter Quality

Goal: "supports many harnesses" means exact, tested rendering, not only YAML
presence.

Adapter support levels:

- **Level 0: Listed** - descriptor parses.
- **Level 1: Rendered** - golden tests prove output shape.
- **Level 2: Applied** - integration tests prove non-destructive writes.
- **Level 3: Live Verified** - real harness docs or smoke checks prove it works.

Checklist:

- [x] Descriptor model exists.
- [x] Thirteen adapters are shipped.
- [x] Every MCP-capable adapter has golden render coverage (Pi ships no MCP, so it
  has descriptor-load coverage only).
- [ ] Every adapter has a support-level badge in docs.
- [ ] Adapter descriptors cite primary docs.
- [ ] CI fails if a new adapter has no golden fixture.
- [ ] Dashboard shows adapter support level and last verification date.

### 6. Trust, Safety, And Audit

Goal: users trust AgentStack because it is explicit, reversible, and boringly
predictable.

Rules:

- Dry run by default where live config is touched.
- Atomic writes with backups.
- No unresolved secrets in live config by default.
- Only AgentStack-owned regions are pruned.
- User-authored files are not deleted.
- Packs and adapters cannot delete paths outside the manifest-owned area.
- Dashboard mutations require clear file-level previews.

Checklist:

- [x] Atomic writes and backups exist.
- [x] Unresolved secret blocking exists.
- [x] Pack instructions have deletion guards.
- [x] Pack skill assets get the same ownership/containment treatment.
- [ ] Dashboard write actions use an explicit preview/confirm flow.
- [ ] `audit` or `explain --json` reports local code execution, secrets, sources,
  and target files.
- [ ] Security-sensitive behavior has regression tests.

## Current Review Findings To Fix First

These near-term correctness gates are now **resolved** (2026-06-29):

1. ~~`upgrade <pack>` deletes skill dirs from manifest paths without containment or
   ownership checks.~~ Fixed: `upgrade` reuses the contained `safe_skill_dirs`
   helper; absolute/`..` paths are never deleted.
2. ~~`remove <pack>` leaves extracted skill dirs behind, so remove/reinstall fails.~~
   Fixed: `remove` deletes pack-owned skill dirs (contained, prunes empty parents);
   remove→reinstall round-trip tested.
3. ~~README vendor-pack examples use invalid command syntax.~~ Fixed to
   `agentstack add from <pack> --write`.
4. ~~`cargo fmt --check` currently fails.~~ Fixed; `cargo fmt --check` is clean.
5. ~~New adapters do not all have golden rendering tests.~~ Added golden coverage
   for all MCP-capable adapters (copilot-cli, opencode, junie, kiro, antigravity,
   claude-desktop).

Bonus fix surfaced by the golden work: the stdio-only Claude Desktop adapter no
longer writes empty `{}` entries for HTTP servers — `render_server` flags
unrepresentable transports and `apply` skips them with a visible note.

## Roadmap

### Phase 0: Correctness Gate

Ship before more product expansion:

- [x] Fix pack skill asset ownership and cleanup.
- [x] Fix upgrade path containment.
- [x] Fix README command examples.
- [x] Run `cargo fmt`.
- [x] Add remove/reinstall tests.
- [x] Add golden tests for all shipped adapters.

Exit criteria (all met 2026-06-29):

- [x] `cargo fmt --check` passes.
- [x] `cargo clippy --all-targets -- -D warnings` passes.
- [x] `cargo test` passes.
- [x] Pack add/remove/upgrade tests cover asset cleanup and path safety.

### Phase 1: Team Portability

Make the "new machine/new teammate" workflow excellent:

- [x] Repo layout: prefer `.agentstack/`, discover legacy root for back-compat,
  docs updated (2026-06-29).
- [ ] Add `agentstack migrate-layout --write` to move a legacy root repo into
  `.agentstack/`. Design:
  - Dry-run by default; `--write` to apply. Refuse (no-op) if `.agentstack/`
    already holds a manifest.
  - Move `agentstack.toml`, `agentstack.local.toml`, `agentstack.lock`,
    `agentstack.md`, `skills/`, and `instructions/` from the repo root into `.agentstack/`,
    preserving relative skill/instruction paths (they stay `./skills/...` and
    just resolve under the new base — no manifest rewrite needed).
  - Use `git mv` when the repo is a git work tree (preserve history), else a
    plain move; atomic/reversible, with a clear preview of every path moved.
  - Leave unrelated root files untouched; never delete anything outside the
    six managed paths.
  - Tests: legacy→`.agentstack/` round-trip, refusal when already migrated,
    and that `apply`/`doctor` work identically afterward.
- [ ] Rewrite README around portable agent environments.
- [ ] Add a `docs/quickstart-team.md`.
- [ ] Add a CI example for `agentstack doctor --ci`.
- [ ] Improve `bootstrap` output around missing secrets and target files.
- [ ] Add `agentstack doctor --json` if not already sufficient for automation.

Exit criteria:

- A fresh checkout can become ready through one documented flow.
- A reviewer can understand every file AgentStack will touch.
- A legacy root repo can move to `.agentstack/` with one reversible command.

### Phase 2: Runtime Control Plane

Make live runs and dashboard central:

- [ ] Dashboard Runs panel shows active profile, scope, target files, servers,
  skills, instructions, hooks, and unresolved risks.
- [ ] Dashboard separates "temporary run profile" from "permanent apply."
- [ ] Add run history or last-run status if useful.
- [ ] Document Unix-only behavior and Windows plan.

Exit criteria:

- Users can answer "what can this running agent access?" from the dashboard.

### Phase 3: Trustworthy Packs

Turn packs into a credible install unit:

- [ ] Add pack metadata: author, official/community/agentstack-authored,
  license, source, version, checksum.
- [ ] Add `agentstack explain <pack>` or improve existing explain output for
  pack composition.
- [ ] Add remote/versioned catalog support.
- [ ] Add pack upgrade tests for changed server, added/removed skills, and
  changed instructions.
- [ ] Add content review guidance for bundled instructions.

Exit criteria:

- Installing a pack feels safer than copy-pasting vendor setup docs.

### Phase 4: Agent-Operable Setup

Let agents propose setup changes safely:

- [ ] Harden `agentstack mcp` around manifest-only edits.
- [ ] Make every agent-triggered mutation previewable and reversible.
- [ ] Add structured responses for "added members", "required secrets", and
  "next human action."
- [ ] Add dashboard approval flow for agent-proposed config changes.

Exit criteria:

- An agent can suggest needed capabilities without silently mutating live
  harness config.

### Phase 5: Distribution And Ecosystem

Only after portability and trust are solid:

- [ ] Support publishing or importing pack archives.
- [ ] Support internal registries or git-backed catalogs.
- [ ] Add SBOM/checksum/audit reports for packs.
- [ ] Support organization policy templates.
- [ ] Document vendor integration: "continue in your own agent."

Exit criteria:

- Teams and vendors can distribute capabilities without centralizing control of
  the user's agent.

## Product Principles

- Prefer reproducibility over magic.
- Prefer explicit previews over silent mutation.
- Prefer repo-local specs over global hidden state.
- Prefer adapter descriptors over hard-coded harness branches.
- Prefer local secret resolution over committed secrets.
- Prefer small, verified support levels over broad untested claims.
- Do not become a marketplace before becoming a trusted runtime manager.

## Definition Of Done For Major Features

Every major feature should include:

- CLI command and help text.
- README or docs workflow.
- Dry-run behavior where files are touched.
- Tests for success, refusal, idempotency, and rollback.
- `doctor` or `explain` visibility.
- Dashboard visibility if the feature affects live setup.
- Clear ownership rules for any files created or deleted.

## One-Line Positioning

> AgentStack makes AI agent environments reproducible, reviewable, and runnable
> across harnesses, repos, and machines.
