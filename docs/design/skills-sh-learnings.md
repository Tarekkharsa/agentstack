# Learning from skills.sh — what to adopt, adapt, and reject

Status: **reviewed (two passes), decision-ready — next step is an
implementation design**. Source: a source-level read of
[vercel-labs/skills](https://github.com/vercel-labs/skills)
(MIT, TypeScript — the `npx skills` CLI behind the
[skills.sh](https://skills.sh) directory) on 2026-07-20, compared against our
shipped install flow. Revised same day after two review passes that verified
every claim about our own code; corrections are folded in below (notably §3,
§4, §6, §10). File references like `src/add.ts` point into their repo;
`crates/...` points into ours.

## Why this repo matters to us

skills.sh is becoming the npm of agent skills: one command
(`npx skills add owner/repo`), ~73 supported agents, a public directory with
hundreds of skills and 400k+ installs, and a spec so minimal (SKILL.md with
`name` + `description` frontmatter) that everyone already publishes in it —
including Anthropic (`anthropics/skills`).

Their trust model is: **none**. Arbitrary repo content flows into agent
context after a single confirm. Their own outro admits it:
*"Review skills before use; they run with full agent permissions."* That gap
is our product. But their acquisition UX is genuinely excellent, and because
the ecosystem publishes in their shapes, their parsing/discovery conventions
are de-facto standards we should speak natively.

The strategic frame: **their supply, our pipeline.** Everything installable by
`npx skills add` should be installable by us — scanned, pinned, and drift-gated
on the way in. The guiding correction from review: adopt their **source
compatibility and convenience**, but keep agentstack's
manifest/profile/delivery-mode model as the single configuration authority.
Their interactive pickers exist *because they have no manifest* — the prompts
are their configuration model. Re-asking questions our manifest already
answers would create two competing models.

---

## 1. Source grammar — the de-facto standard for "where skills live"

`parseSource()` (`src/source-parser.ts`) accepts, in resolution order:

| Input | Meaning |
|---|---|
| `./dir`, `../dir`, absolute path | local path (checked **first**, so a real dir named `owner/repo` wins) |
| `owner/repo` | GitHub shorthand |
| `owner/repo@skill-name` | shorthand + single-skill filter |
| `owner/repo/sub/path` | shorthand + subpath |
| `github.com/owner/repo[/tree/<ref>[/<subpath>]]` | full URL, with branch/tag and subdir |
| `gitlab.com/...` incl. `/-/tree/` and subgroups | GitLab equivalents |
| `github:` / `gitlab:` prefixes | explicit host disambiguation |
| `git@host:owner/repo.git`, `ssh://…` | any git remote (fallback) |
| `<source>#ref` / `#ref@skill` | ref pinning via fragment |
| any other `https://` URL | "well-known" hosted skill |

Worth adopting:

- **The grammar's core.** Our `add skill` takes only `--path`; git skills
  detour through `lib add --git` or need a `pack.toml` + version tags
  (`add from git:`). Plain skill repos — the ecosystem's dominant shape —
  have neither. Accepting `owner/repo`, full URLs (incl. tree paths), and
  local paths is the single highest-leverage change we can make.
- **`sanitizeSubpath()` rejects `..` segments** in user-supplied tree URLs —
  path-traversal guard at parse time, before any fetch. Mirror in any new
  skill-source parser.
- Their `#ref` fragment is only honored when the string *looks like* a git
  source — prevents misreading fragments on arbitrary HTTP URLs.

Adapt, don't copy (review corrections):

- **Compatibility aliases vs canonical flags.** Accept `owner/repo@skill` as
  an alias for humans, but scripts get unambiguous `--skill`, `--rev`,
  `--subpath` flags. Don't reproduce their full ambiguity space (shorthand
  vs subpath vs filter vs ref all packed into one string).
- **Defer "well-known" arbitrary-HTTPS sources.** Any random URL as a skill
  source expands the hostile-input surface for marginal supply; git + local
  covers the ecosystem.
- **No local-first guessing.** Their parser checks the filesystem *before*
  parsing, so `owner/repo` resolves differently depending on whether a
  coincidentally-named directory exists in the cwd. For us, `owner/repo`
  always means GitHub shorthand; a local source must be spelled `./dir`,
  `../dir`, or absolute. Same input, same meaning, on every machine.

## 2. Skill discovery inside a fetched repo — the conventional locations

`discoverSkills()` (`src/skills.ts`) scans a fixed priority list before any
recursive search:

```
<root>                          (SKILL.md at root = the repo IS one skill)
<root>/skills
<root>/skills/.curated
<root>/skills/.experimental
<root>/skills/.system
```

plus 24 agent-convention project dirs:

```
.agents/skills  .claude/skills  .cline/skills  .codebuddy/skills  .codex/skills
.commandcode/skills  .continue/skills  .github/skills  .goose/skills  .iflow/skills
.junie/skills  .kilocode/skills  .kiro/skills  .mux/skills  .neovate/skills
.opencode/skills  .openhands/skills  .pi/skills  .qoder/skills  .roo/skills
.trae/skills  .windsurf/skills  .zcode/skills  .zencoder/skills
```

plus any dirs declared in `.claude-plugin/marketplace.json` / `plugin.json`.

**Copy the locations, not the policies.** The location list *is* the interop
spec — hardcoding it (with attribution) makes
`agentstack add skill anthropics/skills` just work on every repo the
ecosystem publishes. Depth discipline is also right: one level under each
container dir, one extra for `skills/<category>/<skill>/` catalogs, never
descend past a found SKILL.md, prune `node_modules .git dist build
__pycache__`.

Their policies we replace (review corrections):

- **First-name-wins dedup → fail loudly.** Duplicate skill names across
  locations should be an error naming both paths, not a silent pick. A
  security tool doesn't guess which of two same-named payloads you meant.
- **`INSTALL_INTERNAL_SKILLS=1` env var → explicit flag** (if we honor
  `metadata.internal` at all). Hidden env-var behavior changes what gets
  installed; that's a flag's job.
- **Silent recursive fallback → announced and explicit.** Their depth-5
  recursive walk kicks in silently when the priority list finds nothing. If
  our fallback finds candidates, show where each came from and require
  explicit selection — no skill should enter the pipeline from a location
  the user never saw named.
- Frontmatter contract: string `name` + `description` required, otherwise
  the dir is silently not a skill. Keep our existing warning on missing
  `description` (`lib add`) — better than their silence.

## 3. Agent registry + detection — what transfers and what doesn't

`src/agents.ts` defines **73 agents** as a flat record:
`{ name, displayName, skillsDir, globalSkillsDir?, detectInstalled }`.
Detection is just `existsSync()` on a home-dir marker (`~/.claude`,
`~/.cursor`, …), all run in parallel.

What transfers:

- **Detection at install time, not just onboarding.** Our target-resolution
  contract already exists and the add flow should reuse it verbatim:
  `resolve_targets` (`render/apply.rs`) — explicit `--target` wins, else
  `[targets].default`, else the CLIs actually detected on this machine, and
  only when nothing is detected (a CI box) does it widen to every registered
  adapter. Detection **informs the preview — it does not drive a picker**
  (see §4).
- Env overrides respected per agent (`CLAUDE_CONFIG_DIR`, `CODEX_HOME`, …) —
  honor the same variables in our adapters where we don't already.
- Windows symlink handling: NTFS junctions, absolute targets; POSIX relative
  targets; and an actionable failure hint ("On Windows, enable Developer
  Mode for symlink support").
  **Correction from review:** our doc originally claimed we already have
  symlink-with-copy-fallback. We don't — `render/skills.rs:102` propagates a
  symlink failure as a hard error. Decide deliberately: either add a copy
  fallback (with the managed marker, so pruning still works) or keep
  fail-loud; either way the error should name the fix.
- Their skip-missing-config-root behavior (project-scope install into an
  agent whose root, e.g. `.windsurf/`, doesn't exist yet → silently skip)
  is **rejected** on second review: for them it's a sensible guess, but for
  us a resolved target is explicit or detected intent, and silently skipping
  it conflicts with manifest authority. Our existing behavior — report per
  target, never guess — stands.

What doesn't transfer (review correction):

- **Do not adopt `.agents/skills` as a universal broadcast target.** Their
  "universal dir" trick (agents sharing `.agents/skills` need no symlink)
  makes skills ambient across every agent that reads the dir — which would
  weaken profile fencing, our mechanism for controlling exactly which skills
  each activation exposes. If specific tools genuinely consume
  `.agents/skills`, support it **through an adapter** like any other CLI
  path, gated by the same profile membership.

## 4. The add flow — one preview, one write, on the existing `use` seam

Their full happy path is one command with ~5 interactions: parse spinner →
fetch (Trees-API fast path, clone fallback) → fuzzy skill multiselect
(auto-select when exactly one) → agent picker → scope picker → symlink/copy
picker → an Installation Summary note (canonical path, per-agent lines, an
`overwrites:` warning) → async third-party risk table → confirm → `✓` result
lines → *"Done! Review skills before use; they run with full agent
permissions."*

**Correction from review — our baseline was overstated.** The doc originally
claimed our flow is "four commands with four `--write`s." In fact
`use --write` already fetches git-backed sources as needed and records the
lockfile after materializing (`use_profile.rs:38`, `record_lock` at
`use_profile.rs:568`); the common path is two commands. The consequence is
structural, with a precision added on second review: the one-command install
**extracts and reuses `use`'s primitives** — skill resolution, verification,
materialization, lock recording — it does **not** invoke full profile
activation. Adding one skill must never rewrite unrelated server
configuration as a side effect; the broader activation runs only when that
effect is explicitly intended and previewed.

A shipped DX bug this analysis surfaced (fixed 2026-07-20): the MCP
`agentstack_add_skill` tool description and response
(`mcp_server.rs:1105`, `:1376`) told agents "a human runs
`agentstack install` then `apply`" — but `apply` never renders skills
(`apply.rs:772`); both now point at `agentstack use [<profile>] --write`.

What we adopt from their flow:

- **One preview.** A single pre-write summary showing: source and resolved
  commit, discovered skill(s), scan findings, the digest to be pinned, the
  manifest change, and the materialization destinations per target. Our
  dry-run-by-default *is* their confirm step — with real content behind it.
- **One confirmed write**, behaving per the current delivery mode:
  static → manifest + lock + activation; clean-at-rest → manifest + lock,
  no persistent rendering; zero-files → manifest + lock, current lease
  untouched.
- **Auto-select when exactly one skill is discovered**; interactive selector
  for several; `--skill` required in scripts.
- **`--list`** to inspect a source without adding anything.
- **Prompts that explain themselves** (their scope picker's plain-language
  descriptions — the *style* transfers even though the picker doesn't).
- Graceful non-TTY degradation: no prompts, explicit flags required —
  **but never an automatic `-y`.** Their agent-detection force-confirms
  writes; ours must not. An agent-driven or non-interactive invocation gets
  the dry-run/explicit-flag path, consistent with `trust` refusing
  non-TTY grants. Agents have the MCP path with its own gates.

**Preview semantics** (second review pass — the preview promises commit,
scan findings, and digest, which for an uncached repo requires fetching;
today `lib add --git` resolves through the *persistent* store even on a dry
run, `lib.rs:178`, which the new flow must not inherit):

- Preview may fetch into **transient staging**, but never mutates the
  manifest, lock, library, persistent store, or rendered targets.
- `--write` promotes the staged content; manifest/lock changes commit only
  after fetch, discovery, validation, and scan have all succeeded.
- Materialization failing halfway across targets has defined semantics:
  report per-target outcomes explicitly and leave a state `doctor` names
  precisely — never a silent partial success. (As built there is **no**
  cross-target rollback: symlinks/copies are additive, and `use --write`
  completes a half-materialized set. The manifest+lock *commit* is the
  all-or-nothing part; see the transaction note in
  add-skill-source-grammar §4.)

What we reject (review correction — these prompts are their substitute for a
manifest, and we have one):

- Agent picker → targets come from the existing `resolve_targets` contract
  (explicit `--target` → `[targets].default` → detected CLIs → all-adapters
  fallback, §3).
- Project/global scope picker → scope comes from which manifest is in play.
- Symlink/copy picker → strategy comes from the adapter.
- "Last selected agents" memory in the lock file → profile membership
  already encodes intent; UX state does not belong in a lock artifact.

Their step-8 risk table is an async third-party scorecard; our slot in the
flow is the **local content scan** — hidden-Unicode findings gate, injection
heuristics warn (`scan.rs:1`). Call it what it is: a local content scan, not
a "security assessment." The guarantees worth advertising in the preview are
the digest pin, drift refusal, provenance line, and the consent gate itself.

## 5. Lockfile & update design — hash-based, merge-friendly

Two lock files, deliberately different:

- **Global** (`~/.agents/.skill-lock.json`, v3): per-skill
  `source/sourceType/sourceUrl/ref/skillPath/skillFolderHash/installedAt/updatedAt`,
  plus UX state (`dismissed` prompt flags, `lastSelectedAgents`).
- **Project** (`skills-lock.json`, v1, git-committed): **timestamp-free and
  alphabetically sorted on write to minimize merge conflicts** — the comment
  says so explicitly. Hash is a local SHA-256 over sorted relative paths +
  contents.

Update (`skills update`) is content-hash resync, not version comparison:

- One GitHub **Trees API call per source repo** (`?recursive=1`) covers every
  skill from that repo — the folder's git tree SHA is compared against the
  locked hash. No clone unless the source isn't GitHub. Cheap mass
  update-checking; our `lock --update` re-resolves each git source
  individually and could batch the same way.
- **Skipped-with-reason reporting**: skills that can't be update-checked get
  an explicit reason (`Local path`, `No version tracking`, `Private or
  deleted repo`…) plus the exact command to refresh manually. Our update
  path should report the same way.
- **Upstream-deletion detection**: lock entries missing from the fresh tree
  are flagged and interactively offered for removal. We have nothing like
  this; it belongs in `lock --update` or `doctor`.
- Updates execute as `spawnSync(node, [cli, 'add', url, '-g', '-y'],
  {shell:false})` — never through a shell, because the URL comes from a lock
  file that could be attacker-influenced. Good instinct; we stay in-process,
  which is better still.

What *not* to copy: **UX state in the lock file** (dismissed flags, agent
selections — a lock is a security artifact, not a preferences store); schema
bumps that **wipe the lock** instead of migrating; and reinstalls that
**rm-rf the destination**, silently clobbering local edits (flagged only by
a pre-confirm `overwrites:` line). Our managed-vs-unmanaged conflict rule
(`render/skills.rs` leaves non-managed dirs untouched) is strictly better;
keep it, and keep same-name manifest collisions as a hard block that
explains itself — never a silent replace.

## 6. Security touches they DO have — now verified against our code

No trust model, but real hostile-input hygiene. Review verified the
load-bearing "our status" cells below against the shipped code; the rows
still marked "check" are implementation-time diligence, not verified claims:

| Their defense | Where (theirs) | Our status (verified) |
|---|---|---|
| Terminal-escape stripping on all remote-sourced names/descriptions before printing (CWE-150: a malicious SKILL.md can't inject CSI/OSC sequences) | `src/sanitize.ts` | **Confirmed gap.** `parse_frontmatter_description` (`library.rs:36`) returns raw remote text; `search` prints it with truncation only (`search.rs:62`). Fix before any broad remote ingestion lands. |
| `GIT_ALLOW_PROTOCOL=https:http:ssh:git:file` + explicit `ext::` transport block (RCE via crafted git URL) | `src/git.ts` | **Confirmed gap.** `run_git` (`store.rs:298`) is a bare `git` invocation — no protocol restriction. |
| Git-LFS smudge disabled on every clone (`GIT_LFS_SKIP_SMUDGE=1`) — skills are text; never hang on missing git-lfs | `src/git.ts` | **Confirmed gap** — same call site, no LFS suppression, no timeout. |
| Path-traversal guards on parse (`..` segments) and on every FS write (`isPathSafe`) | parser + installer | We guard extraction; verify parity when the new source grammar lands |
| Source/destination overlap guard (don't delete the source when it *is* the destination) | `src/installer.ts` | Check `render/skills.rs` for the same edge |
| Skill-name sanitization before any path use (lowercase, `[a-z0-9._-]`, 255 cap) | `src/installer.ts` | **Confirmed gap.** `valid_lib_name` (`lib.rs:1600`) only rejects empty, `/`, `\`, `.`, `..` — no charset or length rule. Remote frontmatter must not control manifest keys and destination paths under that contract; see the name contract in §10 priority 1. |
| `gh auth token` shell-out is last-resort **and pre-announced on stderr** because endpoint-security tools flag it as credential exfiltration | `src/skill-lock.ts` | A lesson in operational empathy |

The first three are small, self-contained hardening changes on code we
already ship, and they're prerequisites for opening the door to arbitrary
ecosystem repos.

## 7. Discovery & the directory

- `skills find` hits `GET https://skills.sh/api/search?q=<q>&limit=10[&owner=]`
  → `{skills:[{id,name,installs,source}]}`, sorted by installs. A
  `SkillsShProvider` in our provider chain is *possible* — but **deferred**:
  it's their private, undocumented API, and install counts are a
  popularity signal, not a quality signal; review flagged
  popularity-driven discovery as off-model for us. If we ever add it, it's
  clearly labeled unvetted and fails silent.
- The **search→install pipe** is the real learning: selecting a search
  result should flow into the add preview in one session, rather than
  printing a command to retype.
- **The `find-skills` meta-skill** is their cleverest distribution move: a
  skill that teaches the agent to search the directory when the user asks
  "how do I X", with quality heuristics and a one-time post-install upsell.
  We already ship `using-agentstack` in the embedded catalog — extend it (or
  add a sibling) so agents know to drive `agentstack search`/`add`
  themselves, with *our* quality signals: scan verdicts, pin status,
  provenance — not install counts.

## 8. Smaller ideas worth keeping

- **`skills use <src>@<skill> | claude`** — ephemeral, install-free skill
  execution: materialize to a temp dir, emit a wrapper prompt to stdout for
  piping (or spawn `claude`/`codex` directly). A natural fit for our
  clean-at-rest philosophy — a scan + pin to a session grant without
  touching the manifest. Note their precedent for risk-tiering sources:
  unverified community skills require a `--dangerously-…` flag with a blunt
  warning.
- **`skills init <name>`** scaffolds a SKILL.md template (frontmatter +
  "When to use" + "Instructions" sections) and prints publishing next-steps.
  Trivial `agentstack lib new <name>` addition; closes the authoring loop.
- **CI detection** (`CI`, `GITHUB_ACTIONS`, …) to auto-disable interactivity
  (not to auto-confirm — see §4).
- **Path shortening** in output (`$HOME`→`~`, cwd→`.`) everywhere.
- ANSI-aware column padding for tables containing colored text.
- Clone timeout with an env override (`SKILLS_CLONE_TIMEOUT_MS` equivalent)
  and a tailored error that suggests cloning manually + pointing at the
  local path.
- Auth failure ladder: HTTPS → `gh repo clone` → SSH `BatchMode=yes` → a
  SAML-SSO-specific error message with the exact fix commands.

## 9. What we deliberately do NOT copy

- **The trust model (absence of one).** Silent full-permission installs
  after one confirm is the anti-goal. Every convenience above lands *behind*
  our scan gate, digest pin, and drift refusal — never instead of them.
- **The interactive decision model** — agent/scope/method pickers and
  selection memory. Those prompts are skills.sh's substitute for a manifest;
  we have the manifest, profiles, `[targets]`, and adapters as the single
  authority (§4).
- **Universal `.agents/skills` fan-out** — ambient cross-agent skills would
  bypass profile fencing (§3).
- **Automatic `-y` on agent detection** — an embedded agent must never
  auto-confirm a write; it gets the dry-run path or the gated MCP tools.
- **Anonymous install telemetry / leaderboard**, and popularity as a
  discovery ranking. Off-brand for a security tool; our signals are scan
  results, pins, and provenance.
- **73-agent breadth.** Their "support" is a path mapping + `existsSync`.
  Our 13 adapters render real configs, hooks, and policy. Depth over
  breadth; add adapters when demanded, not for a number.
- **Wipe-on-schema-bump lockfiles**, **clobber-on-reinstall**, and **UX
  state in lock files**. Our lockfile is a security artifact and our
  materializer's never-touch-unmanaged rule is a feature.
- **Hardcoded owner allowlists for fast paths** (`vercel`, `vercel-labs`,
  `heygen-com` get the no-clone blob path). Trust tiers by hardcoded org
  name is exactly the shortcut we exist to replace.

## 10. The CLI to build

Post-review target shape:

```
agentstack add skill owner/repo              # discover, preview, one confirmed write
agentstack add skill owner/repo --skill pdf  # explicit selection for scripts
agentstack add skill ./local-skill
agentstack add skill owner/repo --list       # inspect only, adds nothing
```

Behavior:

- Implemented by **reusing `use`'s primitives** (resolve → verify →
  materialize → lock-record) — never by invoking full profile activation;
  adding one skill must not touch unrelated server configuration (§4).
- Local sources require `./`, `../`, or an absolute path; `owner/repo` is
  always GitHub shorthand (§1).
- Targets resolve through the existing `resolve_targets` contract (§3);
  a resolved target that can't be materialized is reported, never silently
  skipped.
- Preview stages transiently and mutates nothing; `--write` promotes, and
  commits manifest/lock only after fetch + discovery + validation + scan
  succeed; partial materialization failure has defined per-target reporting
  and is left doctor-diagnosable — not rolled back (§4).
- Profile targeting: no profiles → implicit default; exactly one → automatic;
  several → `--profile` required, or an interactive ask in a TTY.
- Exactly one discovered skill → auto-selected; several → selector in a TTY,
  `--skill` in scripts; duplicate names across locations → hard error naming
  both paths.
- Existing same-named skill → block and explain; never silently replace.
- One preview (source, resolved commit, skills, scan findings, digest,
  manifest diff, destinations), then one `--write` that is mode-aware:
  static → manifest + lock + activation; clean-at-rest → manifest + lock
  only; zero-files → manifest + lock, current lease untouched.
- The central library stays an explicit, separate destination — same
  simplified grammar: `agentstack lib add owner/repo --skill pdf`.

**Destination — decided (second review pass).** `agentstack add skill …`
modifies the **current manifest**; `agentstack lib add …` modifies the
**personal central library**. Each verb names its destination — no hidden
routing. The "cross-repo default" language in `docs/reference.md:611`
describes the library *mental model* for sharing skills across repos; it
does not require the project-level `add skill` verb to be secretly
library-backed. Users who live the library-first workflow keep `lib add` +
by-name profile references, unchanged.

Priority order:

1. **Hardening prerequisites** (§6): terminal-escape stripping on remote
   text; `GIT_ALLOW_PROTOCOL` + LFS smudge + timeout on `store.rs` git
   calls; and the **remote-name contract** — before frontmatter `name`
   controls manifest keys and destination paths, decide: is it required, can
   `--name` override it, the exact accepted grammar and length cap, and
   fail-on-invalid vs normalize (a security tool should fail, not guess).
   Small, self-contained, and required before inviting arbitrary repos in.
2. **Speak the grammar + locations** (§1, §2): `add skill
   owner/repo|URL|path`, the conventional location scan, `--skill`/`--list`,
   fail-loud duplicates.
3. **One preview, one write on the `use` seam** (§4, §10) — after the
   destination decision above.
4. **Update ergonomics** (§5): skip-reasons and upstream-deletion
   detection in `lock --update`. (The "Trees-API batch checking" idea —
   one API call per source instead of a fetch per skill — was **not**
   built: `lock --update` resolves each git source individually. Left as a
   future optimization; correctness doesn't depend on it.)
5. **Distribution loop** (§7, §8): finding-skills catalog skill, `lib new`
   scaffolding, ephemeral use.

**Status 2026-07-20: all five priorities implemented.** 1 →
hardening-remote-ingestion.md; 2 → add-skill-source-grammar.md; 3 →
add-skill-activation.md; 4 → update refresh semantics + upstream-deletion
detection + skip reasons (a review round additionally fixed branch-pin
re-tracking and restored preview containment) — **excluding** the
Trees-API batch optimization, which was not built (see priority 4 above);
5 → `lib add` gained the source grammar and the finding-skills catalog
skill shipped. The §8
smaller ideas landed too: `lib new` scaffolding and `agentstack try
<source> | <cli>` ephemeral use (staged, scanned, symlink-refusing,
manifest-free). The deliberate rejections (§9) stand.
