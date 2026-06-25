# agentstack Project Review

Date: 2026-06-25

## Executive Verdict

agentstack is useful for real developers, especially developers who regularly use more than one AI coding harness, move between machines, or work in teams where MCP servers, skills, instructions, and permissions need to be reproducible. The idea is not just "sync config files." The stronger framing is:

> agentstack is a compiler and auditor for agent capability setup.

That framing is credible because the implementation already has the hard parts that sync tools usually skip: dry-run by default, adapter descriptors, non-destructive merges, state-based pruning, secret references, init/adopt flows, profile selection, lockfiles, and doctor checks.

The project is not yet obviously useful for every developer. A solo developer using exactly one agent CLI and two MCP servers may not feel enough pain to adopt another tool. The best first users are power users, AI-heavy teams, consultants, platform/devex teams, and companies trying to standardize agent setup across repos without committing secrets.

My recommendation: continue. The idea has real value. Tighten the product wedge, harden a few trust gaps, and ship a small sharp workflow before broadening into marketplace/provider features.

## What The Product Is Really Solving

The problem statement is valid. Maintaining agent setup is becoming a real developer operations problem:

- MCP config formats are fragmented across Claude Code, Codex, Cursor, Windsurf, Gemini, and VS Code.
- Secrets are easy to leak or hard to reproduce.
- Skills and instructions are portable in theory but not easy to activate selectively.
- Team setup docs rot quickly.
- New machines, devcontainers, and repos require repeated manual configuration.
- AI agents themselves need a safe way to propose tools without directly mutating live harness configs.

The durable value is not format translation by itself. If the ecosystem converges on `mcp.json`, a pure converter becomes less valuable. The durable value is the layer above that:

- one committed manifest
- machine-local secret resolution
- profile and project scoping
- lockfile-backed skill installation
- drift detection
- non-destructive writes
- policy/doctor gate
- safe agent-operable manifest edits

That bundle is useful even if every CLI eventually agrees on MCP syntax.

## Who This Is For

Strong early users:

- Developers using 2 or more AI CLIs.
- Teams with shared MCP servers and internal skills.
- Platform/devex teams publishing a standard agent setup per repo.
- Consultants switching between clients and toolchains.
- Security-conscious teams that want agent capability review in git.
- Developers who use project-specific profiles, for example backend, frontend, data, research, incident response.

Weak early users:

- Single-agent users with minimal configuration.
- Users who expect a marketplace/app-store experience immediately.
- Users who are uncomfortable letting any third-party tool touch live CLI config files.
- Teams that already manage everything through dotfiles and do not care about agent-specific semantics.

The first landing message should speak to the strong early users. Do not over-optimize for everyone.

## Product Positioning

The current README phrase "Dotfiles for your AI agents" is good, but the more differentiated pitch is:

> Reproducible agent capabilities: one manifest, every harness, no leaked secrets.

I would avoid leading with "every CLI" too strongly. It is attractive, but it can create a support burden. The safer first promise is:

> Make your agent setup reproducible, reviewable, and restorable across the harnesses you actually use.

The key trust story should be visible everywhere:

- `init` imports what you already have.
- `diff` shows exactly what changes.
- `apply` is read-only unless `--write`.
- writes are atomic and backed up.
- only agentstack-owned entries are pruned.
- secrets stay as references in git.
- `doctor --ci` gives teams a gate.

That is the adoption path.

## Codebase Strengths

### 1. The architecture matches the problem

The adapter descriptor model is the right call. Files like `adapters/codex.yaml`, `adapters/claude-code.yaml`, and `adapters/vscode.yaml` encode target quirks as data. That keeps the core renderer from becoming a pile of `if target == ...` branches.

This is especially important because CLI config formats will keep changing. A data-driven adapter model gives users and contributors a low-friction path to add or patch harness support.

### 2. Dry-run planning is a strong core abstraction

`src/render/apply.rs` separates planning from writing. That is exactly the right shape for a tool touching live config files. `TargetPlan` carries existing content, proposed content, managed entries, removals, unresolved secrets, and diff output.

This enables `apply`, `diff`, `doctor`, dashboard previews, and future CI checks to share behavior instead of reimplementing risky file logic.

### 3. Non-destructive merge behavior is treated as a first-class feature

The tests cover preserving unrelated JSON/TOML content, preserving floats, idempotency, and pruning only previously managed entries. This is one of the most important trust factors for this product.

The integration test `non_destructive_merge_preserves_other_content_and_is_idempotent` is exactly the kind of test this project needs more of.

### 4. The "never a blank page" workflow is valuable

`init` and `adopt` are major adoption unlocks. Tools like this fail when users must manually rewrite their existing setup into a new format. Importing existing configs, lifting secrets, and preserving comments turns migration into a reversible workflow.

### 5. Secret handling is directionally right

The chain of env, varlock, keychain, and `.env` is pragmatic. Keeping `${REF}` in the manifest is the right default. The code also avoids silently blanking missing secrets, which matters.

### 6. The MCP server mode is a smart differentiator

`agentstack mcp` is a strong idea because it lets agents propose setup changes without directly applying them to live configs. This could become the cleanest way for agents to help users install the tools they need.

The rule should remain strict: agent writes manifest-only changes; humans or explicit trusted automation run `apply`.

### 7. Test suite is healthy

Current verification:

- `cargo test`: passed, 72 unit tests, 6 golden tests, 9 integration tests.
- `cargo clippy --all-targets -- -D warnings`: passed.

The existing coverage is focused on the right risks: rendering quirks, non-destructive merges, idempotency, secret lifting, settings merge/prune, instruction regions, skill materialization, and provider parsing.

## Main Risks And Findings

### Finding 1: Unresolved secrets should block writes

Severity: High

`apply --write` currently reports unresolved secrets but still writes if there are changes. In `src/commands/apply.rs`, unresolved secrets increment `error_count`, but `plan.write()` still runs when `will_write` is true.

For this product, unresolved secrets are not just warnings. Writing `${TOKEN}` placeholders into real CLI config can break harness behavior, confuse users, and weaken the central "safe secrets" promise.

Recommendation:

- Make unresolved secrets block `apply --write`, `use --write`, dashboard apply/toggle, and `doctor --fix`.
- Add `--allow-unresolved` only if there is a real use case.
- Add tests proving unresolved refs cannot reach live config writes by default.

### Finding 2: Validation warnings do not stop writes

Severity: Medium

Manifest validation currently prints warnings but does not prevent writes. Missing transport fields, unknown refs, or invalid profile members can produce surprising partial output.

Recommendation:

- Split validation into warnings and errors.
- Block writes on structural errors.
- Keep warnings for compatibility quirks.

### Finding 3: README and CLI comments disagree about dashboard mutability

Severity: Medium

The README says the dashboard can set secrets, apply, activate profiles, and install. The CLI arg help says `--read-only` is "reserved; the dashboard is read-only in this phase." The server actually exposes mutation endpoints when not in read-only mode.

This is a trust problem. Users need exact language around what can mutate disk.

Recommendation:

- Update `DashboardArgs.read_only` help text.
- Add a dashboard safety section to README.
- Make the UI always show the target manifest dir, scope, and whether actions write the manifest, live harness config, keychain, or store.

### Finding 4: Dashboard mutation actions bypass the best CLI safety affordances

Severity: Medium

The CLI encourages diff-before-write. The dashboard has `/api/diff`, but mutation endpoints like `/api/apply`, `/api/toggle`, and `/api/use` can directly write after one request.

Recommendation:

- Require a generated plan token for live config writes: user previews diff, server returns `plan_id`, apply uses that exact plan.
- At minimum, surface a clear "this writes to X files" confirmation in the UI and reuse unresolved-secret blocking.

### Finding 5: Project/global scope default is inconsistent with README wording

Severity: Medium

The README says scope defaults to project when a manifest is in the working dir, else global. The `apply` implementation currently uses `args.scope.unwrap_or(Scope::Global)`.

Recommendation:

- Either implement the documented default or update the docs.
- I would keep global as explicit and make project activation a deliberate `--scope project` until users trust the tool.

### Finding 6: Official registry support is useful but still shallow

Severity: Medium

The registry provider currently picks the first remote or first package and normalizes a limited subset. That is fine for MVP search, but it is not enough for a high-confidence "install anything from registry" promise.

Recommendation:

- Present registry additions as "candidate import" rather than guaranteed install.
- Show multiple install choices when available.
- Preserve provider metadata in the manifest or lockfile.
- Add trust/audit fields: package manager, package id, version, source URL, declared inputs, and whether it runs local code.

### Finding 7: Lockfile integrity is not security-grade

Severity: Medium

`Store::dir_digest` uses FNV-1a and comments clarify it is integrity, not security. That is okay internally, but if the README says checksum-pinned/reproducible, users may infer tamper resistance.

Recommendation:

- Use SHA-256 for lockfile checksums.
- Keep FNV only for non-security local change detection if desired.
- Document that git sources should be pinned to commits for teams.

### Finding 8: The product may feel broad before it feels complete

Severity: Product risk

The feature set is wide: adapters, skills, settings, instructions, profiles, dashboard, registry, bundle export/import, MCP server, hooks, policy, stats. The breadth is impressive, but users need one obvious path that works perfectly.

Recommendation:

Optimize the first-run funnel:

1. `agentstack init --dry-run`
2. `agentstack init`
3. `agentstack doctor`
4. `agentstack diff`
5. `agentstack apply --write`
6. `agentstack doctor --ci`

Everything else should be secondary until this path feels boring and safe.

## Feature Ideas Worth Considering

### 1. Capability profiles as repo contracts

Add a `agentstack doctor --profile <name> --ci --scope project` workflow designed for repos. This lets a repo declare "this project needs these agent capabilities" and CI can verify the manifest is valid without checking user secrets.

### 2. `agentstack explain`

Developers will ask "why is this MCP server in Codex but not Claude?" Add:

```sh
agentstack explain github
agentstack explain --target codex
```

It should explain source, profiles, targets, scopes, current state, lock entry, secret refs, and rendered destination.

### 3. Risk scoring for capabilities

Not a scary security score, just useful labels:

- remote HTTP vs local stdio
- runs `npx`, `uvx`, or other executable
- needs secrets
- has filesystem access
- source is pinned or floating
- source is from catalog, official registry, local path, or git

This reinforces the trust/governance story.

### 4. `agentstack plan`

A command that produces a structured JSON plan of all intended writes:

```sh
agentstack plan --scope project --format json
```

This would help the dashboard, CI, agents, and tests all consume the same write intent.

### 5. Team bootstrap bundles

The `export`/`import` feature is useful, but for teams the better primitive may be:

```sh
agentstack bootstrap
```

It would read the manifest, install skills, show missing secrets, optionally open docs for where to get them, and stop before live config writes.

### 6. Adapter conformance tests

For each adapter, keep fixtures for:

- global MCP config
- project MCP config if supported
- native settings import
- native settings render
- instruction file location
- skill dir behavior

This will matter as harnesses change.

## Suggested Roadmap

### Now

- Block writes on unresolved secrets and structural validation errors.
- Fix dashboard read-only/mutation wording.
- Align scope defaults between README and implementation.
- Add a short demo GIF or terminal cast showing init, diff, apply, doctor.
- Add "what files will this touch?" to README.

### Next

- Add `agentstack explain`.
- Add structured `agentstack plan --format json`.
- Upgrade lockfile checksums to SHA-256.
- Harden dashboard write flow with diff-confirmed plan IDs.
- Add more integration tests around dashboard actions and unresolved-secret blocking.

### Later

- Rich registry install choices and metadata preservation.
- Team bootstrap workflow.
- Policy rules for local executable capabilities.
- Signed/trusted catalogs if you build a marketplace layer.
- Plugin management as a first-class capability only after MCP/skills/settings are stable.

## Final Opinion

This is a real product, not just a utility script. The idea is useful because the AI tooling ecosystem is fragmenting faster than normal developer setup workflows can absorb. Developers do not want to remember six config formats, manually copy MCP servers between harnesses, or explain to every teammate how to wire secrets into every agent.

The strongest version of agentstack is a boring, trustworthy compiler:

- manifest in git
- secrets out of git
- deterministic rendered files
- readable diffs
- reversible writes
- clear ownership
- CI-friendly doctor checks

The project already has much of that foundation. The next step is to reduce adoption risk: block unsafe writes, tighten documentation, and make the first successful workflow extremely clear. After that, the dashboard, MCP server mode, registry integration, and team policy features become meaningful multipliers rather than distractions.

