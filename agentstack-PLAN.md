# agentstack — Full Implementation Plan (Rust)

> Working name: **agentstack**. A single-binary CLI that manages MCP servers + skills
> across multiple AI coding agents (Claude Code, Codex, Cursor, …) from one portable
> source of truth. Built for **really easy setup of anything, for any CLI, on any machine.**
>
> This document is the spec/kickoff. Drop it into the new Rust project and start from
> "Session 0 kickoff prompt" at the bottom.

---

## 1. Problem & vision

Configuring AI agents today is three separate pains tangled together:

1. **Format fragmentation** — same MCP server, different syntax per CLI
   (Codex = TOML `[mcp_servers.x]`, Claude Code = JSON `~/.claude.json`,
   Cursor = `type:"stdio"`, Windsurf = `serverUrl`, Codex has no `${VAR:-default}`).
2. **Selective loading** — sometimes load all skills, sometimes a few, sometimes none.
   This is a *context-economy + profiles* problem, NOT a sync problem.
3. **Secrets & per-machine drift** — real tokens differ per machine and must not be committed.

**Vision:** describe your agent capabilities **once** in a portable manifest; the tool
compiles that manifest into each CLI's native config, resolves secrets per-machine, lets
you scope capabilities per profile/project, and proves the whole thing works with a
`doctor` command.

**North star:** be **cargo/npm for agent capabilities** — a manifest + lockfile + local store +
(eventually) a registry that resolves and installs MCP servers, skills, plugins, and instruction
files, and **cross-compiles** them into every agent CLI's native format. The cross-compile is the
part cargo/npm never had to do. See **§9d**.

### Design principles (the DX bar)
- **Intent in, files out.** You say `add kibana`; you never think about TOML vs JSON.
- **Never a blank page.** `init` reverse-engineers the manifest from configs already on the machine.
- **Secrets are references, never literals in the manifest.** Manifest is commit-safe.
- **Compile, don't sync.** One source of truth + `diff`/drift detection. No "which file won last" wars.
- **Data-driven adapters.** Supporting a new CLI = adding one descriptor file, not editing core code.
- **Single binary.** `brew install agentstack`, zero runtime deps, auditable, trustable.
- **Non-destructive.** Only touch entries we own; preserve formatting/comments; `diff` before write.

---

## 2. Prior art — what we borrow vs. where we differ

| Tool | What we take | What we do differently |
|---|---|---|
| **sync-agents-settings** (Leoyang183) | Multi-agent targets list; CLAUDE.md→AGENTS.md idea; ship as plugin too | NOT "Claude Code is source of truth, one-way sync." We compile from a neutral manifest. We add **skills + profiles + secrets + doctor**. |
| **unified-mcp-manager** (0xRaghu) | **Profiles** ("backend"/"react"), **encrypted secrets**, duplicate detection | CLI-first single binary (not GUI/Bun server); add **skills** and many more agents via data-driven adapters. |
| **mcp.directory converter** | The exact per-format quirks table (JSON↔TOML, `mcpServers`→`servers`, `type:stdio`, `serverUrl`/`httpUrl`) | We encode these as **adapter descriptors**, applied automatically — not a manual paste tool. |
| **chezmoi** approach | Per-machine "machine facts" → our **machine-local overlay** | Explicit `agentstack.local.toml` overlay instead of templating; domain-specific, not generic dotfiles. |
| **SKILL.md / AGENTS.md** standards | Skills are already portable files; reuse the standard | We manage **which** skills are active **where** (the unsolved part). |

**Our wedge (what nobody combines):** MCP **+** skills **+** profiles **+** selective loading
**+** secrets-by-reference **+** cross-machine migration **+** `doctor`, in one single binary,
extensible to any CLI via data descriptors.

---

## 3. Architecture (Rust)

### 3.1 Crate layout
Start as a **single binary crate** with clean modules; split into `core` lib + `cli` bin
later if we ship a GUI/plugin. (Decision D1.)

```
agentstack/
├── Cargo.toml
├── README.md
├── LICENSE-MIT  LICENSE-APACHE         # dual license (Rust convention)
├── adapters/                            # embedded descriptors (include_dir!)
│   ├── claude-code.yaml
│   └── codex.yaml
├── tests/                               # integration tests w/ temp HOME + insta snapshots
│   └── golden/
└── src/
    ├── main.rs                          # clap entry + dispatch
    ├── cli.rs                           # clap derive arg structs
    ├── manifest/
    │   ├── mod.rs
    │   ├── model.rs                     # Manifest, Server, Skill, Profile structs (serde)
    │   ├── load.rs                      # layered load: agentstack.toml + agentstack.local.toml
    │   └── validate.rs                  # profile→server/skill references, schema checks
    ├── adapter/
    │   ├── mod.rs
    │   ├── descriptor.rs                # AdapterDescriptor (serde from YAML)
    │   ├── registry.rs                  # embedded + ~/.agentstack/adapters/ override
    │   └── render.rs                    # generic renderer → JSON / TOML value tree
    ├── secret/
    │   ├── mod.rs
    │   ├── resolver.rs                  # Resolver trait + chain
    │   ├── keychain.rs                  # keyring crate
    │   ├── env.rs                       # $ENV + .env file
    │   └── vault.rs                     # op:// / pass / vault (phase 4)
    ├── render/
    │   ├── apply.rs                     # orchestrate render+merge+write per target
    │   ├── merge_json.rs                # non-destructive JSON merge (serde_json preserve_order)
    │   └── merge_toml.rs                # non-destructive TOML merge (toml_edit, keeps comments)
    ├── state.rs                         # ~/.agentstack/state.json: which entries WE manage + hashes
    ├── discover/
    │   └── init.rs                      # detect installed CLIs, import existing MCP/skills, lift secrets
    ├── doctor/
    │   ├── mod.rs
    │   ├── checks.rs                    # static checks
    │   └── live.rs                      # --live MCP handshake (http + stdio)
    ├── commands/
    │   ├── init.rs add.rs apply.rs diff.rs doctor.rs
    │   ├── use_profile.rs               # activate a profile
    │   ├── export.rs import.rs          # encrypted bundle (age)
    │   ├── adapters.rs                  # list / show / add descriptor
    │   └── secret.rs                    # secret set/get/rm
    └── util/ (paths.rs, color.rs, prompt.rs)
```

### 3.2 Dependencies (Cargo)
- **clap** v4 (derive) — command surface
- **serde**, **serde_json** (feature `preserve_order` → indexmap), **toml** + **toml_edit** — read/write configs non-destructively
- **serde_yaml** (or `serde_yml`) — parse adapter descriptors
- **include_dir** — embed adapters/ in the binary
- **keyring** — cross-platform OS keychain for secrets
- **inquire** — interactive prompts (Decision D2; alt: dialoguer)
- **reqwest** (blocking, rustls) + JSON-RPC — `doctor --live` HTTP MCP handshake
- **directories** / **dirs** — resolve `~`, config dirs
- **anyhow** + **thiserror** — errors
- **owo-colors** / **console** — colored doctor/diff output
- **age** (rage) — encrypted export/import (phase 3)
- **insta** (dev) — snapshot tests; **assert_fs**/**tempfile** (dev) — temp HOME integration tests

---

## 4. Data model

### 4.1 Manifest — `agentstack.toml` (portable, committable, NO secrets)
```toml
version = 1

[meta]
name = "tarek-agent-setup"

# ---- MCP servers ----
[servers.kibana]
type = "http"                                   # http | stdio
url = "https://kibana-mcp.ghaloyalty.com/mcp"
headers = { Authorization = "Bearer ${KIBANA_TOKEN}" }   # ${REF} resolved per machine

[servers.github]
type = "stdio"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-github"]
env = { GITHUB_TOKEN = "${GITHUB_PAT}" }

# ---- Skills (portable SKILL.md dirs) ----
[skills.jira-triage]
path = "./skills/jira-triage"

[skills.deep-research]
path = "./skills/deep-research"

# ---- Profiles = named bundles (THIS is selective loading) ----
[profiles.backend]
servers = ["kibana", "github"]
skills  = ["jira-triage"]

[profiles.research]
servers = ["kibana"]
skills  = ["*"]                                  # all skills

[profiles.minimal]
servers = ["kibana"]
skills  = []                                     # nothing

# ---- Targets ----
[targets]
default = ["claude-code", "codex"]               # where `apply` writes when unspecified
```

### 4.2 Machine overlay — `agentstack.local.toml` (gitignored, per-machine)
Same schema; deep-merged over the shared manifest at load time. Used for machine-only
servers, path differences, target subsets. Empty by default.

### 4.3 State — `~/.agentstack/state.json` (managed-entry tracking)
```json
{
  "targets": {
    "claude-code": { "managed_servers": ["kibana","github"], "managed_skills": ["jira-triage"], "last_hash": "…" },
    "codex":       { "managed_servers": ["kibana","github"], "managed_skills": ["jira-triage"], "last_hash": "…" }
  }
}
```
Lets `apply` add/update/**remove only entries we own**, and lets `doctor`/`diff` detect drift
without polluting the target configs with markers. (Decision D4.)

---

## 5. Adapter descriptors (the "any CLI" unlock)

A new CLI = one YAML file. Embedded ones ship in the binary; users drop overrides/new ones
in `~/.agentstack/adapters/`. These descriptors grow over time: a `scopes:` block (global vs
project locations) and a skills `strategy:` (§9b, Phase 2), and an `instructions:` block for
CLAUDE.md / AGENTS.md (§9c, Phase 3). The renderer stays generic; new capabilities = new
descriptor fields, not new core code.

```yaml
# adapters/codex.yaml
id: codex
display: Codex CLI
detect:
  bin: codex                          # on PATH ⇒ installed
  config: ~/.codex/config.toml
config:
  path: ~/.codex/config.toml
  format: toml
mcp:
  location: mcp_servers               # table keyed by server name: [mcp_servers.<name>]
  fields:
    url: url
    command: command
    args: args
    headers: http_headers             # Codex names headers "http_headers"…
    env: env
  headers_as_subtable: true           # …as a nested table, not inline
  secret_mode: literal                # Codex has no ${VAR:-default}; write resolved value
skills:
  dir: ~/.codex/skills
```

```yaml
# adapters/claude-code.yaml
id: claude-code
display: Claude Code
detect:
  bin: claude
  config: ~/.claude.json
config:
  path: ~/.claude.json
  format: json
mcp:
  location: mcpServers
  fields:
    url: url
    headers: headers
    command: command
    args: args
    env: env
  transport: { key: type, http_value: http }   # Claude needs "type":"http" for http servers
  secret_mode: literal
skills:
  dir: ~/.claude/skills
```

**Generic renderer** reads the descriptor, builds a value tree for each managed server
(applying field renames, transport key, header nesting, secret substitution), then the
format-specific merger writes it:
- `merge_json.rs` — parse existing file → navigate `location` → upsert managed keys → write
  pretty, preserving key order (serde_json `preserve_order`/indexmap).
- `merge_toml.rs` — `toml_edit::Document` → upsert under `location` → write, **preserving
  existing comments/formatting** of untouched sections.

**Escape hatch:** descriptor may declare `hook: <path/to/script>` for the ~10% that can't be
pure data (e.g. OAuth flows). 80% data, 20% code.

---

## 6. Secrets — resolver chain

Manifest holds `${NAME}` (or `${op://vault/item/field}`). At `apply`/`doctor`, resolve via an
ordered chain; first hit wins:
1. **Process env** (`$NAME`) and project `.env`
2. **OS keychain** (keyring crate, service `agentstack`, account `NAME`) ← default store
3. **Vault refs** (`op://…`, `pass`, HashiCorp) — phase 4

`agentstack secret set KIBANA_TOKEN` → prompts (hidden) → stores in keychain.
On render, `secret_mode: literal` writes the resolved value into the target config (the target
configs already hold plaintext tokens today; the **manifest** stays clean — that's the win).
Unresolved refs are a `doctor` error, never a silent empty string.

---

## 7. Command surface

```
agentstack init            # discover installed CLIs, import existing MCP+skills+instructions, lift inline secrets→keychain, write manifest

# ---- package-manager surface (north star, §9d) ----
agentstack add <name>[@ver] # add a capability (server|skill|plugin|instructions) from registry|git|path → resolve, lock, store, render
agentstack add             # interactive variant: type → source → which CLIs → which profile
agentstack install         # install everything per agentstack.lock (reproducible). --locked (CI), --frozen
agentstack update [<name>] # bump within version constraints, rewrite the lock
agentstack remove <name>   # drop a capability + prune from targets
agentstack search <query>  # search the registry (v2+)
agentstack publish         # publish a capability/bundle to the registry (v3)

agentstack apply           # render manifest → each target's native config (non-destructive). --profile, --target, --dry-run, --scope global|project
agentstack diff            # show drift between manifest and on-disk configs (per target)
agentstack doctor          # verify everything is wired up. --live (handshake), --fix (repair safe), --ci (nonzero on error)
agentstack use <profile>   # set active profile (materializes which servers/skills are live), then apply. --scope global|project
agentstack scope <name> --to global|project   # move a skill/server between the global and project manifests, then re-materialize
agentstack skills list     # show the skill library + which are active, per scope
agentstack adopt           # pull hand-edits from a target config back into the manifest
agentstack secret set|get|rm <NAME>
agentstack adapters list|show <id>|add <file>
agentstack search <query>  # find skills/servers/plugins (local now, registry later) (§9g)
agentstack stats           # usage analytics: activations + transcript-mined invocations (§9g)
agentstack skill analyze <name>   # mine past conversations → improvement report for the agent (§9g)
agentstack dashboard       # open the local web dashboard (§9f). --port, --no-open, --read-only
agentstack export [--encrypt] > setup.age      # manifest (+optionally secrets) bundle
agentstack import setup.age                    # restore on a new machine
agentstack init --from <git-url|dir>           # clone a manifest then apply (migration)
```

### First-run experience (the "wow")
```
$ agentstack init
🔍  Detected 3 CLIs:  Claude Code · Codex · Cursor
📥  Imported 5 MCP servers, 12 skills from existing configs
🔐  Found 1 inline secret (kibana token) → moved to keychain, replaced with ${KIBANA_TOKEN}
✅  Wrote agentstack.toml
```
```
$ agentstack add
? Type            › MCP server
? Name            › kibana
? URL             › https://kibana-mcp.ghaloyalty.com/mcp
? Auth            › Bearer ${KIBANA_TOKEN}     (stored in keychain)
? Which CLIs?     › ◉ Claude Code  ◉ Codex  ◯ Cursor
? Which profile?  › backend
✓ Wrote ~/.claude.json (JSON) + ~/.codex/config.toml (TOML)
```

---

## 8. `doctor` — the trust layer

```
$ agentstack doctor
Adapters & CLIs
  ✓ Claude Code   installed · ~/.claude.json parses
  ✓ Codex         installed · ~/.codex/config.toml parses
  ⚠ Cursor        config present but binary not on PATH
Secrets
  ✓ KIBANA_TOKEN  resolved (keychain)
  ✗ GITHUB_PAT    referenced by 'github' — not found ↳ agentstack secret set GITHUB_PAT
Drift
  ⚠ Codex         'kibana' header edited on disk ↳ agentstack apply (or adopt)
Skills
  ✓ jira-triage   path exists · SKILL.md frontmatter valid
  ✗ deep-research profile 'research' references it, but ./skills/deep-research missing
Quirks
  ✓ no unsupported secret syntax for any target
MCP connectivity (--live)
  ✓ kibana        handshake OK · 14 tools
  ✗ github        401 Unauthorized
2 errors, 2 warnings.  Run `agentstack doctor --fix` to repair 1 safe issue.
```

Check categories: **CLIs/adapters** (installed, config parses) · **secrets** (every ref
resolves on THIS machine — key migration check) · **drift** (on-disk vs manifest via state
hash) · **quirks** (unsupported syntax per target *before* it breaks) · **skills** (paths,
frontmatter, profile refs, symlinks) · **connectivity** (`--live` MCP `initialize` handshake,
tool count).

Modes: default = static+offline (fast); `--live` = network handshakes; `--fix` = repair safe
class only (re-apply drift, re-link skills, prompt missing secrets — never destructive);
`--ci` = exit nonzero on any error (team gate).

---

## 9. Migration (cross-machine)

Falls out of secrets-by-reference + portable manifest:
```
# new machine
brew install agentstack
agentstack init --from git@github.com:you/agent-setup.git   # or import setup.age
agentstack apply          # renders only to CLIs THIS machine has; prompts for missing secrets
agentstack doctor         # confirm everything wired
```
Secret transport options (progressive, additive — none requires refactor):
1. **Re-prompt** (default, zero infra) — `apply` asks for missing secrets, stores in keychain.
2. **Vault reference** (`op://…`) — nothing to migrate; the reference *is* portability.
3. **Encrypted export** (`export --encrypt`/`import`, age) — one passphrase-protected bundle.

---

## 9b. Capability scoping: global vs project (+ skill materialization)  *(added)*

Agent harnesses already split capabilities two ways: **global** (personal — loaded in every
project you open) and **project** (loaded only inside that repo). agentstack should manage both
axes and let a capability **move between them**.

### Scope = which manifest the entry lives in
- **Global manifest** `~/.agentstack/agentstack.toml` → renders to each CLI's **global**
  locations (`~/.claude/skills`, `~/.claude.json`, `~/.codex/…`). Active everywhere.
- **Project manifest** `<repo>/agentstack.toml` → renders to each CLI's **project** locations
  (`<repo>/.claude/skills`, `<repo>/.mcp.json`, project settings). Active only in that repo —
  and **any agent that opens the folder inherits it, even without agentstack installed**,
  because we write the CLI's own native project files.

No per-entry `scope=` field: scope is *implied by which file the entry is in*. "Switch a skill
global→project" = move its `[skills.x]` block from the global manifest into the project one (the
`scope` command does this and re-materializes). Load order: when both exist, the **project
manifest overrides/extends the global** one (project wins) — same layering as
`agentstack.local.toml`.

### Adapter descriptors gain `scopes`
Each adapter declares the locations for every scope it supports. A CLI with no project concept
simply omits `project`, and a project `apply` skips it with a clear message. (Note: Claude Code
has real project files — `.mcp.json`, `.claude/skills/`; Codex today only has *global* config +
per-path trust entries, so it starts global-only.)

```yaml
# claude-code.yaml (excerpt)
scopes:
  global:
    config: ~/.claude.json
    skills_dir: ~/.claude/skills
  project:
    config: .mcp.json            # project-scoped MCP servers
    skills_dir: .claude/skills   # project-scoped skills
skills:
  strategy: symlink              # native | symlink | copy   (see D9)
```

### Skill materialization — the "only N of my skills" mechanism
Skills are *directories*, so selective loading means controlling which dirs are present in a
scope's `skills_dir`:
1. **Library** = skill sources in the manifest (`[skills.x] path=…`) plus skills already on disk.
2. **Active set** = the selected profile's `skills` list (or `["*"]` for all).
3. `apply` / `use <profile>` makes exactly the active set present in the scope's `skills_dir`,
   and removes **only the links/dirs agentstack owns** (tracked in `state.json` `managed_skills`).
   Hand-added skills are never touched.

Strategy is **adapter-declared** so it's easy, flexible, and works everywhere:
- `native` — flip the CLI's own enable/disable toggle where one exists (no file moves). Preferred.
- `symlink` (default) — symlink active library skills into `skills_dir`; no duplication, trivially
  reversible.
- `copy` — physically copy (Windows / sandboxes where symlinks are awkward); `doctor` flags when a
  copy has drifted from its source.

agentstack picks the most native strategy a target supports, with `symlink → copy` fallback; the
user can pin one per adapter.

### Selective loading, end to end
> "I want only 2 of my 10 skills active this session."

```toml
[profiles.focus]
servers = ["kibana"]
skills  = ["jira-triage", "deep-research"]   # just these two
```
`agentstack use focus` materializes those two into the active scope and prunes the other managed
ones. In a repo, the **project manifest's profile** governs what's active there — so opening that
folder in any agent yields exactly that set.

---

## 9c. Instruction files (CLAUDE.md / AGENTS.md) as a managed capability  *(added)*

Beyond MCP servers and skills, every harness reads a **system-instructions / memory** file, and
you want the same "write once, render everywhere" treatment — **plus** the ability to add
**harness-specific instructions** ("for Claude, also do X"):
- Claude Code → `CLAUDE.md` (can `@import` other files, e.g. `@AGENTS.md`)
- Codex & the emerging cross-tool standard → `AGENTS.md`
- Cursor → `.cursor/rules/*.mdc`; Windsurf → `.windsurfrules`

**Model — named instruction fragments, each tagged with the harnesses it targets:**
```toml
[instructions.house-style]
path    = "./instructions/house-style.md"   # shared prose
targets = ["*"]                             # → every harness

[instructions.claude-extras]
path    = "./instructions/claude-only.md"
targets = ["claude-code"]                   # harness-specific block
```
`apply` concatenates the fragments applicable to each harness into that harness's instruction file,
**inside a managed region**:
```md
<!-- agentstack:start -->
…compiled shared + harness-specific instructions…
<!-- agentstack:end -->
```
Prose files are heavily hand-edited and have no structured keys to "own", so the managed-marker
approach is the right tool here — a deliberate **exception to D4** (which keeps *config* files
marker-free). Everything outside the markers is preserved untouched.

**Scope** reuses §9b: global → `~/.claude/CLAUDE.md` / `~/.codex/AGENTS.md`; project →
`<repo>/CLAUDE.md` / `<repo>/AGENTS.md`. Adapters declare include support so we can exploit the
`@AGENTS.md` idiom — render a canonical shared `AGENTS.md` once and have `CLAUDE.md` `@`-import it
instead of duplicating text.

**Adapter descriptor extension:**
```yaml
instructions:
  scopes: { global: ~/.claude/CLAUDE.md, project: CLAUDE.md }
  format: markdown
  managed_region: true
  includes: { syntax: "@{path}", canonical: AGENTS.md }   # CLAUDE.md @import
```

---

## 9d. North star: a package manager for agent capabilities  *(added)*

agentstack is a **package manager that cross-compiles**: the manifest is `Cargo.toml`; the unique
step is rendering one resolved dependency set into *every* CLI's native format.

| cargo / npm | agentstack |
|---|---|
| `Cargo.toml` (deps + constraints) | `agentstack.toml` — servers, skills, plugins, instructions with `version` + `source` |
| `Cargo.lock` (exact pins) | **`agentstack.lock`** — reproducible, shareable, checksum-verified |
| `~/.cargo/registry`, `node_modules` | **`~/.agentstack/store/`** — content-addressed fetched capabilities |
| crates.io / npm | **capability registry** (git-index → hosted) |
| `cargo add` / `npm i <pkg>` | `agentstack add <name>@version` → resolve, lock, store, render |
| `cargo build` | `agentstack apply` — but to N CLIs |
| `cargo update` | `agentstack update` |
| cargo features | **profiles** (selective loading) |
| workspaces | **global vs project scope** (§9b) |
| `cargo publish` | `agentstack publish` |

**Capability sources (cargo-style):**
```toml
[skills.deep-research]
source = "registry"
version = "^1.2"                                   # from the index

[skills.house-thing]
source = "git"
git = "https://github.com/me/skill"
rev = "…"

[skills.local]
source = "path"
path = "./skills/local"
```

**Registry path — earn the service (cargo had git/path deps before crates.io; Go modules are just git):**
1. **v1 — sources + lockfile + store, no server.** `git`/`path` capabilities; `agentstack.lock`
   pins commit/hash; `~/.agentstack/store/` caches. Reproducible & team-shareable today, zero infra.
2. **v2 — git index.** A lightweight index repo maps `name → source + versions` (original
   crates.io-index / Homebrew-tap style). `agentstack add kibana` resolves through it. No service.
3. **v3 — hosted registry.** Search, ownership, signing, stats — same `add <name>` UX. Only when
   adoption justifies building **and moderating** it.

**Trust is first-class, not polish.** A skill/plugin is *instructions and tools an agent will act
on autonomously* — a supply-chain surface closer to a browser extension than a library. So:
lockfile **checksums**, **signing / provenance**, pinned versions, `agentstack install --locked`
for CI, and a "review before install" prompt. Positioning: **the security-first agent package
manager.**

---

## 9e. Teams & collaboration  *(added)*

The manifest is commit-safe and the lockfile pins exact versions, so **the team's agent setup
travels in the repo via git**. Most team value falls out of that; the pieces below make
onboarding, shared secrets, and governance first-class.

### Onboarding (the payoff)
```
git clone … && cd repo
agentstack install        # fetch locked skill/server versions into ~/.agentstack/store
agentstack apply          # render only to the CLIs THIS person has; materialize project skills
agentstack doctor         # lists missing secrets/CLIs with how-to-fix
```
The lockfile guarantees identical capability versions; `apply` adapts to each machine; `doctor`
makes "what do I still need" obvious. Clone → the team's exact agent setup, on Claude Code / Codex /
Cursor alike.

### Secrets contract — personal vs shared (D17)
The manifest never holds secret values, only `${REF}`s. A `[secrets]` block documents each ref so
`doctor` can coach onboarding and so shared team tokens resolve for everyone:
```toml
[secrets.KIBANA_TOKEN]
scope = "personal"                       # each teammate sets their own
hint  = "Kibana → Stack Management → API keys"

[secrets.TEAM_DEPLOY_TOKEN]
scope  = "shared"
source = "op://team-vault/deploy/token"  # committed vault REF — value never is
```
- **personal** → each person stores their own (keychain/varlock); `doctor` shows `hint` when missing.
- **shared** → a vault reference (1Password/Infisical/…); the *reference* is committed, the value
  never is, and it resolves for any teammate with vault access (varlock handles the providers).

No secret material is ever committed. Teams without a vault fall back to `export --encrypt` /
`import` (age, Phase 3).

### Governance / policy — opt-in (D18)
Teams/orgs set guardrails enforced by `doctor --ci`:
```toml
[policy]
require = ["security-review", "audit-logging"]                    # must be active
forbid  = ["shell-exec-unrestricted"]                             # must NOT be installed
allowed_sources = ["git:github.com/acme/*", "registry:acme-index"]  # supply-chain allowlist
```
`agentstack doctor --ci` fails the build if a required capability is missing, a forbidden one is
present, or any capability comes from a disallowed source — a real supply-chain gate for the
*executable-intent* skills/plugins (D16). Off by default; lives in the committed manifest, so the
policy travels with the repo.

### Contribute-back loop
A teammate who hand-tweaks a config locally runs `agentstack adopt` to pull the change into the
manifest, then opens a PR. "Compile, don't sync" → everyone re-renders from the merged manifest, no
"which machine won" conflicts.

### CI & devcontainers
Headless setups use `agentstack install --locked` with secrets from CI env/vault (no keychain, no
prompts). Ships well as a **devcontainer feature / Codespaces** step so cloud dev environments get
the same agent setup as laptops.

### Private team catalog
The v2 **git-index registry** (§9d) can be a *private* repo — the company's blessed capability
catalog. `agentstack add @acme/kibana` pulls the team-standard setup; `[policy].allowed_sources`
pins installs to it.

---

## 9f. Local dashboard (`agentstack dashboard`)  *(added)*

One command spins up a **local web dashboard** to see — and manage — every skill, server,
instruction, and secret across all harnesses in one place:
```
agentstack dashboard          # serves 127.0.0.1:<port>, opens the browser
  --port <n> --no-open --read-only
```

**A web UI without betraying the single-binary promise.** Unlike unified-mcp-manager's Bun/GUI
server (§2), the dashboard is **embedded in the same binary** — a tiny localhost HTTP server +
self-contained HTML/vanilla-JS assets baked in via `include_dir`. No Node, no separate service, no
runtime deps: still `brew install agentstack`, still one auditable binary. CLI and dashboard are
two thin front-ends over the **same core library** — this is the GUI trigger flagged in **D1**
(split into `core` lib + `cli` + `web`).

**The centerpiece — a cross-harness matrix** (the one view the CLI can't match): capabilities ×
harnesses, showing at a glance what's active where, in which scope, with drift + secret health.
```
                     Claude Code   Codex    Cursor    Scope      Health
kibana       (mcp)       ✓           ✓        –        global    ● secret ok
github       (mcp)       ✓           ✓        ✓        project   ✗ GH_PAT missing
jira-triage  (skill)     ✓           –        ✓        project   ↪ symlinked
deep-research(skill)     ✓           ✓        ✓        global    ↪ symlinked
house-style  (instr)     ✓           ✓        ✓        project   ✓ in sync
```
Click a cell to toggle a capability for that harness, flip global↔project, or open the rendered
diff. Side panels: **secrets** (resolved/unresolved + source, never the value), **doctor** live,
**profiles** (activate one), **drift** (apply / adopt).

**Phasing:** a **read-only viewer first** (cheap — it just renders existing core results: manifest,
registry, `plan_target`, `doctor`, secret *status*), then editing actions (toggle, scope,
`secret set`, `apply`) behind the same local JSON API.

**Security — non-negotiable for a local server over configs + secrets:**
- bind **127.0.0.1 only**, random port, one-time token in the launch URL; reject cross-origin.
- **never expose secret values** — only resolved/unresolved + which source.
- mutating actions are explicit and show the diff before writing; honor `--read-only`.

---

## 9g. Capability lifecycle: discover · observe · improve · agent-operable  *(added)*

The tool shouldn't only *install* capabilities — it should help you **find** the right ones,
**observe** how they're used, and **improve** them, and be operable by the **agent itself**, not
just humans. This turns the package manager (§9d) into a full **capability lifecycle manager**:
discover → install → scope → observe → improve.

### Search & discovery
`agentstack search <query>` finds skills/servers/plugins — locally (installed + library + manifest)
now, across the git-index/registry later (§9d). Results are pickable: choose what matters for *this
project* and `add`/`use` it. Pairs with profiles for context economy ("load only what this task
needs").

### Agent-operable — the agent provisions itself
agentstack is exposed **to the agent**: a CLI it already runs, and (Phase 4) an **MCP server**
surfacing `search` / `add` / `use` / `list` as tools. Mid-session the agent can discover a
capability it needs and load it into the session/project, managing its own context economy — a
first-class operator of the package manager.

> **Trust gate (D20, ties D16/§9e):** an agent autonomously installing *executable-intent*
> skills/plugins is the supply-chain risk. Agent-initiated installs are **policy-gated** — free to
> toggle within an allowlisted/curated catalog (`[policy].allowed_sources`), human-confirm for
> anything outside it. The agent proposes; the human (or policy) disposes.

### Usage analytics — "how often is this loaded/used?"
A `usage`/`stats` view shows which capabilities earn their context:
- **Activation counts** — agentstack records each time it materializes/activates a capability
  (cheap, exact).
- **Invocation counts** — mined from harness transcripts/logs (skill invocations, MCP tool calls);
  adapters declare a `transcripts:` location per harness.
Surfaced in the CLI (`agentstack stats`) and as **status lines / sparklines per capability in the
dashboard**. Stored in `~/.agentstack/usage.json`; opt-in, **local-only** (never leaves the
machine). (D21)

### Skill-improvement loop
`agentstack skill analyze <name>` mines past conversations where the skill was used and extracts
friction signals (errors, retries, user corrections, abandons), emitting a **structured report**.
agentstack stays LLM-free: it produces the *evidence*; the **agent** (or Anthropic's skill-creator)
reads the report and rewrites `SKILL.md`, then agentstack re-materializes. The discover→install→
observe→improve loop closes the feedback cycle cargo/npm never had. (D22)

### Dashboard as full control center (elevates §9f)
The dashboard is a full GUI over the core library, **peer to the CLI** — not just a viewer: edit the
manifest, toggle capabilities per harness/scope, set secrets (to keychain), switch profiles, preview
the diff, run apply/use/instructions/doctor with one click, and read the usage status lines above.
Anything the CLI can do, the dashboard can do.

---

## 9h. Industry landscape & the meta-layer thesis  *(added — 2026 reframe)*

The ecosystem moved while we built. The strategic update:

- **Official MCP Registry** (`registry.modelcontextprotocol.io`, launched Sep 2025; Anthropic +
  GitHub + Microsoft + PulseMCP). REST API, reverse-DNS namespaces (`io.github.user/server`). A
  *canonical* source for MCP servers now exists.
- **Plugins/skills exploded.** Claude Code plugins (stable Oct 2025) bundle skills + agents + hooks
  + MCP servers, distributed via git **marketplaces**. ~21k skills, ~2.5k marketplaces; a competing
  CLI package manager already exists (`ccpi`). Org governance is appearing (LiteLLM gateway as a
  Claude-only governed registry).
- **Fragmentation is multiplying, not converging.** Each CLI ships its own marketplace/plugin/skill
  system. The official registry unifies *MCP servers only* — not skills, plugins, instructions, or
  cross-CLI install.

**Thesis: don't build a marketplace — be the cross-CLI meta-layer.** A universal *client* that
**consumes** the official registry + existing marketplaces and **compiles** a chosen set into every
CLI's native format, adding what the vendors won't (cross-tool, selective loading, secrets, scope,
reproducibility, governance). Like `mise`/`asdf`/Homebrew above many sources — not a source.

### Providers abstraction (supersedes "build our own registry")
A `Provider` trait with pluggable backends, queried together by `search` / `add` / the Discover UI:
- `registry` → the **official MCP Registry API** (`/v0/servers`), namespace-aware.
- `marketplace` → git-based skill/plugin marketplaces (Claude's marketplace format + community).
- `catalog` → our embedded starter set (built).
- `git` / `path` → direct sources (built).
Results are normalized to a common shape and `add` renders to **all** target CLIs at once.

### Trust & security layer (source-agnostic) — the real lesson from skills.sh
skills.sh (Vercel Labs) runs per-skill security audits (Socket, Snyk, Gen Agent Trust Hub,
Runlayer, ZeroLeaks → `pass|warn|fail` + `riskLevel`) — pointing at the #1 risk: installing a
skill/MCP is **running executable intent** (D16). But its API is **Vercel-OIDC-auth-gated** (401
anonymously), and skills.sh skills are just GitHub `owner/repo` — which agentstack already installs
via its **git source**. So: don't depend on skills.sh's audits; build a **neutral trust layer** from
signals we can always get, with skills.sh audits as *optional* token-gated enrichment:
- **MCP registry**: namespace verification (reverse-DNS = owner-verified) + `status`.
- **install-risk heuristics**: runs arbitrary `npx`/`uvx`? pinned in the lock? demands secrets?
- **governance**: the **`[policy]` block (require / forbid / `allowed_sources`)** enforced by
  `doctor --ci` — *built*. allowed_sources matches a capability's source label
  (`git:host/owner/repo`, `path:…`, `registry:…`) against globs.
- positioning: **cross-source trust gating** — skills.sh audits Vercel-only, LiteLLM governs
  Claude-only; agentstack gates *every* source across *every* CLI. (D16/D18)

### Dashboard "Discover" (two-pane curation — user idea)
A two-pane browser: **left = catalog** (all providers, searchable, categorized) · **right = your
stack** (manifest / active-per-CLI). Check items on the left, choose which CLIs + profile, apply →
they move right. Selective loading becomes a checklist, not config editing. This is the "choose
across Claude plugins + Codex plugins + skill sites in one place, the easy way" experience.

---

## 10. Per-directory auto-activation
`direnv`-style: entering a repo activates its **project manifest** (`<repo>/agentstack.toml`)
across all CLIs — no explicit command. A shell hook (zsh/bash/fish) runs `agentstack use --scope
project` on `cd`; a `.agentstack` pointer file can override which profile to activate. This is the
payoff of project scoping above. The "tweet-worthy" feature; phase 3+.

---

## 11. Phased roadmap

- **Phase 0 — skeleton (compiles, read-only)**  ✅ *done*
  manifest model + layered load; adapter descriptors (claude-code, codex); generic renderer;
  `apply --dry-run` + `diff` against real configs (no writes).
- **Phase 1 — MVP (genuinely better than incumbents)**  ✅ *done (interactive `add` deferred to P2)*
  `init` (discover+import+lift secrets); `apply` (non-destructive write, state tracking);
  `secret set/get/rm/list` (keychain **+ varlock** resolvers); `doctor` (static checks, `--ci`).
  Targets: Claude Code + Codex.
- **Phase 2 — trust + profiles + scoping**
  `doctor --live` + `--fix`; interactive `add`; `use <profile>`; **capability scoping (global vs
  project)** + **skill materialization** (`native`/`symlink`, `copy` fallback) + `scope <name>
  --to …` + `skills list` (§9b); `agentstack.local.toml` overlay; `adopt`.
- **Phase 3 — package-manager core + teams + reach**
  **PM core (§9d): `agentstack.lock` + `~/.agentstack/store/` + `add`/`install`/`update`/`remove`
  with `git`/`path` sources** (reproducible, team-shareable, no hosted registry); **team
  onboarding (§9e): clone→install→apply→doctor**, `[secrets]` contract (personal/shared via vault)
  + `doctor` coaching, CI/devcontainer (`install --locked`); **instruction files (§9c): CLAUDE.md
  / AGENTS.md** with shared + harness-specific blocks; `export/import` (age); `init --from <git>`;
  more adapters (Cursor, Gemini, Windsurf, Kiro); per-directory auto-activation.
- **Phase 4 — registry + governance + lifecycle + ecosystem**
  **git-index registry (v2): `name → source` resolution + `search` (§9g)** (incl. **private team
  catalog**); **`[policy]` governance (§9e) enforced by `doctor --ci`**; capability **signing /
  provenance** + `install --locked`; **agent-operable MCP server** surfacing search/add/use as
  tools (policy-gated, D20); **usage analytics** (`stats`, transcript mining, dashboard status
  lines, §9g); **plugins as a managed capability** (Codex `[plugins."x@y"]` + marketplaces, Claude
  marketplaces); vault resolvers; **local web dashboard (§9f) as a full control center** —
  read-only matrix first, then full editing/control (§9g). *(Splitting into `core` lib + `cli` +
  `web` workspace happens here — see D1.)*
- **Phase 5 — hosted registry (v3) + self-improvement**
  hosted service (`agentstack publish`, search, ownership, stats, curation/moderation) behind the
  same `add <name>` UX; **skill-improvement loop** (`skill analyze` → evidence report → agent
  rewrites SKILL.md, §9g) — only once adoption justifies it.

---

## 12. Testing strategy
- **Unit:** renderer golden tests per adapter (manifest → expected JSON/TOML) via `insta`.
- **Integration:** `tempfile`/`assert_fs` temp HOME; run `init`→`add`→`apply`→`doctor` end-to-end;
  assert non-destructive merge (untouched keys/comments preserved).
- **Quirk regression:** explicit cases for Codex no-`${:-default}`, Claude `type:"http"`,
  header nesting/rename.
- **Safety:** every write path has a `--dry-run` covered by tests; `apply` never writes secrets
  into the manifest.

---

## 13. Open decisions (pick before/early in build)

| # | Decision | Options | Recommended |
|---|---|---|---|
| D1 | Crate structure | single crate · workspace (core lib + cli bin) | **single crate now**, split when GUI/plugin lands |
| D2 | Interactive prompt lib | inquire · dialoguer | **inquire** (nicer multiselect) |
| D3 | Secret default store | OS keychain (keyring) · encrypted file · plaintext .env | **keychain**, with .env fallback for CI |
| D4 | Managed-entry tracking | sidecar `state.json` · in-file markers | **sidecar state.json** (keeps target files clean) |
| D5 | `doctor --live` v1 scope | HTTP only · HTTP+stdio | **HTTP first**, stdio in phase 2 |
| D6 | Name | agentstack · mcpx · agentctl · rig · conductor | TBD (check crates.io / GH availability) |
| D7 | License | MIT · Apache-2.0 · dual | **dual MIT/Apache-2.0** (Rust norm) |
| D8 | Config write reformatting | preserve exactly (toml_edit/indexmap) · normalize | **preserve exactly** (non-destructive promise) |
| D9 | Skill materialization strategy | native toggle · symlink · copy | **adapter-declared**: `native` when the CLI supports it, else `symlink`, with `copy` fallback (Windows/sandbox) |
| D10 | Scope representation | per-entry `scope=` field · separate global vs project manifests | **separate manifests** (scope = which file the entry lives in; project overrides global) |
| D11 | Project config locations | confirm per CLI | Claude Code: `.mcp.json` (servers) + `.claude/skills/` (skills); Codex: **global-only initially** (no per-dir config file) |
| D12 | Reproducibility | manifest only · manifest + lockfile + store | **lockfile + content-addressed store** (`agentstack.lock`, `~/.agentstack/store/`) — reproducible, checksum-verified |
| D13 | Registry model | build our own (git-index/hosted) · **consume the official MCP Registry + aggregate marketplaces** | **CONSUME, don't build** (§9h): a Provider trait over the official MCP Registry API + git marketplaces + our catalog + git/path. Our value is cross-CLI compile, not being a registry. *(Updated 2026 — supersedes the earlier "earn the service" plan.)* |
| D14 | Instruction-file ownership | own whole file · managed marker region | **managed marker region** (`<!-- agentstack:start/end -->`) — prose is hand-edited; explicit exception to D4 |
| D15 | Instructions model | one source w/ conditional blocks · named fragments tagged by target | **named fragments + `targets`** (data-driven; composes shared + harness-specific cleanly) |
| D16 | Capability trust | trust-on-fetch · checksums + signing/provenance | **checksums in lock now; signing/provenance before any registry** (skills/plugins are executable-intent) |
| D17 | Team secrets | personal only · personal + shared-via-vault · + encrypted bundle | **personal + shared-via-vault**, documented in a `[secrets]` contract (vault refs committed, values never) |
| D18 | Team governance | convention only · opt-in `[policy]` enforced by `doctor --ci` | **opt-in `[policy]`** (require/forbid/allowed_sources), off by default, gated in CI |
| D19 | Dashboard delivery | embedded server + self-contained HTML/vanilla-JS (single binary) · separate SPA build/toolchain | **embedded, self-contained, localhost-only + token** (keeps the single-binary, no-runtime promise); read-only viewer first, full control later |
| D20 | Agent-initiated install autonomy | free install · propose-only · policy-gated (free in allowlist, confirm outside) | **policy-gated** — agent toggles freely within `[policy].allowed_sources`, human-confirms outside it (executable-intent risk) |
| D21 | Usage analytics source | activation counts only · + transcript mining | **both**: exact activation counts now, transcript-mined invocations later; local-only, opt-in (privacy) |
| D22 | Skill improvement | agentstack calls an LLM · agentstack emits evidence, agent rewrites | **evidence-only** (`skill analyze` → report); the agent/skill-creator rewrites — keeps agentstack LLM-free and single-binary |

---

## 14. Session 0 kickoff prompt (paste into the new session)

> We're building **agentstack**, a Rust single-binary CLI that manages MCP servers + skills
> across AI agent CLIs (Claude Code, Codex, …) from one portable `agentstack.toml`. Read
> `PLAN.md` in this repo for the full spec. Start with **Phase 0**: `cargo init`, add deps
> (clap, serde, serde_json[preserve_order], toml + toml_edit, serde_yaml, include_dir, keyring,
> inquire, anyhow, thiserror, owo-colors; dev: insta, assert_fs, tempfile), implement the
> manifest model + layered load, the embedded adapter descriptors for claude-code and codex,
> the generic renderer, and `apply --dry-run` + `diff` running **read-only** against my real
> `~/.claude.json` and `~/.codex/config.toml`. Do NOT write to those files until I approve a diff.
> My real Kibana server (for test fixtures): url `https://kibana-mcp.ghaloyalty.com/mcp`,
> header `Authorization: Bearer ${KIBANA_TOKEN}`. Confirm decisions D1–D8 (defaults in PLAN.md)
> then begin.

### Reference: current real configs to import/test against
- Claude Code: `~/.claude.json` — top-level `mcpServers` (currently `tldraw`, `kibana_mcp`),
  HTTP servers use `"type":"http"` + `headers`.
- Codex: `~/.codex/config.toml` — `[mcp_servers.<name>]`, headers under
  `[mcp_servers.<name>.http_headers]`; also has `figma`, `miro`, `chrome-devtools`, `kibana_mcp`.
- Both already contain the same Kibana MCP — the canonical "configure once, render to both" case.
