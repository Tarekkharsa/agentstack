# Changelog

User-facing changes per release. The [GitHub Releases
page](https://github.com/Tarekkharsa/agentstack/releases) carries the built
binaries, checksums, and provenance attestations for each entry.

## v0.14.0 — 2026-07-19

**The onboarding loop closes in both directions.** The docs now mirror the
product, and the product now hands you to the docs at the two moments
curiosity peaks.

### Added

- **Onboarding doorways.** Every wizard run — whichever delivery mode you
  choose — closes with a link to the getting-started walkthrough and the
  reminder that bare `agentstack` always names your next step. And the very
  first guard denial on a machine explains itself with a one-line pointer to
  how the guard works, exactly once (a marker file; fail-open, one
  `exists()` check on the hook hot path, and the audit log always records
  the original denial reason untouched).
- **The README hero is the wizard itself** — a generated terminal replay of
  the real first-run arc (plan → secret-storage choice → delivery-mode fork
  → machine-change summary), every line quoted from the binary and produced
  by the same generator as the other demos, so it cannot silently drift.

### Changed

- **The docs quality wave.** The getting-started walkthrough now forks on an
  accessible static / clean-at-rest / zero-files tab control at exactly the
  point the wizard asks, and you read only your path; the docs hub opens
  with a twelve-entry "I want to…" index that routes by job; the examples
  page gains category filter chips and job-stating titles; the feature
  reference gains a complete two-level table of contents, a doorway sentence
  on every section, and a journey-shaped order — with zero content loss.

**One command sets up everything.** Bare interactive `agentstack init` is now
the guided wizard: a plan of what will happen, import, a real choice of where
secrets live, guard seeding that explains itself, an optional deep scan, a
visible delivery-mode choice, and a closing "what changed on this machine"
summary with the undo command. Scripts keep the promptless primitive under
the same verb (any explicit flag opts in); `setup` still works as a hidden
alias but is no longer advertised. The Get Started walkthrough, README, and
the animated replay show the wizard's real captured output — shipped in the
same commits as the behavior.

### Added

- **Secret storage is a chosen destination.** A new `.env` writer (values
  land beside the manifest, auto-gitignored with a durable entry) joins the
  OS keychain: interactive init presents both plus skip, each option with
  plain-words help text; non-interactive runs use `--secrets
  env|keychain|skip` (default remains keychain — scripts never start writing
  plaintext by surprise). `agentstack secret set --env-file` writes there
  too. `--no-keychain` is a deprecated alias that now names every unstored
  ref and its store one-liner — the silent value-drop is gone.
- **The wizard steps**: opening plan; deep-scan offer (only when skills
  exist); machine-change summary built from the write ledger. Bare
  `agentstack` now shows the project's current mode.
- **The delivery-mode choice is a real fork** — asked before anything is
  written, as an arrow-key selection where every option carries its
  consequence, and it changes what the wizard does next: static renders into
  every CLI; clean-at-rest skips rendering, locks the pins, and teaches the
  session rhythm; zero-files offers the gateway registration and points at
  `trust .` (never run for you — trust stays a human decision). Interactive
  menus use `dialoguer` (new dependency, cli crate only, minimal features).
- **Guard teaches**: out-of-workspace denials print the exact
  `[guard] allow_roots` TOML line and the file to edit; `guard install`
  prints the seeded deny list and the machine/project layering; `guard
  status` labels every rule layer's source file. Cursor gains the
  `beforeReadFile` blocking hook; Windsurf gains `pre_mcp_tool_use`.
- `agentstack lock` warns before writing when new pins will re-gate trust,
  and doctor's lock-drift error explains why it is an error.

### Fixed

- **Sandbox confinement mounts the project root.** Under the recommended
  nested `.agentstack/` layout, `run --sandbox`/`--lockdown` mounted the
  manifest folder as `/workspace`, hiding the project's code from the
  confined agent. The mount, the banner, and both lockdown shadow checks now
  derive from the project root in lockstep.
- **Doctor and diff can no longer disagree about drift.** The
  edited-on-disk warning is gated on the same managed-content comparison
  `diff` uses, so configs that double as live state stores (Claude Code's
  `~/.claude.json`) no longer flap forever.
- **VS Code write-gate gap closed**: agent-mode's `replace_string_in_file` /
  `apply_patch` edits now classify as writes, so workspace confinement
  applies. Codex hooks register exactly once (the manifest renderer defers
  to `hooks.json`) and deny via the documented stdout decision envelope.
- Honest init wording ("N CLI binaries on PATH", correctly pluralized), VS
  Code hook support labelled Preview, and every doc/example that captured
  the old outputs updated and asserted.

## v0.13.0 — 2026-07-19

Tagged for the `init` wizard work the day it landed, then superseded hours
later by v0.14.0, which folded in the docs-quality wave and shipped both
under the one combined entry above. No separate v0.13.0 changes exist
beyond what v0.14.0's entry already carries — this entry exists for
tag↔changelog parity.

## v0.12.0 — 2026-07-18

**Breaking: the off-strategy surface is gone.** A full project review cut
~10,000 lines that worked against the product's own strategy, with every
kept feature re-verified against the docs. The plugin-recipe/marketplace
lane (`plugins` command, `[plugins.*]` recipes, `session start --plugin`)
is removed — `[extensions.*]` is the governed successor for native harness
add-ons, and the vendor-pack install ledger it hosted is renamed
`[plugins.*]` → `[packs.*]` (old ledgers are not recognized; re-run
`add from` for installed packs). The dashboard is now a **read-only lens**:
all 22 write endpoints and the `--read-only` flag are gone — the router has
no write arm, every change happens through the CLI. Verb moves: `audit` →
`doctor --deep` (with a new `doctor --json`), `proxy start|report` → bare
`agentstack proxy` (the relay) + `agentstack report wire` (the ranking),
`report calls --transcripts` and `lib consolidate` removed outright. The
visible surface grows 14 → 18: `explain`, `lock`, `lib`, and `adopt` are
promoted — they carry the inspect/reproduce/library/drift promises and
belong in `--help`.

### Added

- **`report wire`** — the observe-only wire relay's per-capability
  tokens-per-turn ranking, folded into the one "what happened" verb.
- **`doctor --json`** — the full structured doctor report (supersedes the
  removed `audit --json`).
- **Docs prose lint in CI** — every `agentstack <verb>` inside a code span
  anywhere in the docs must name a real subcommand, checked against the
  live clap tree; it caught three live doc bugs on its first run.
- **Interception map** (`docs/interception-map.svg`) — the four lanes
  (proxy observes; gateway, guard, egress enforce) at the top of the
  enforcement matrix.
- Reference coverage that was missing: `[policy.egress]` /
  `[policy.secrets]` / `[policy.filesystem]` authoring, the full MCP
  control-plane tool roster, a dedicated `session` section, and the
  varlock secrets story (activation via `.env.schema`, 1Password /
  AWS/Azure/GCP / Bitwarden providers, same fail-closed `${REF}` contract
  as the OS-keychain default).
- ARCHITECTURE gains the operating-model chapter (choose the boundary you
  need) ported from the site; ENFORCEMENT states "policy is authority, not
  isolation" explicitly.
- GitHub front door: status badges, issue forms (with a secrets-redaction
  warning), a PR template carrying the security-review checklist, and the
  CI trust-gate Action linked from the docs hub.

### Changed

- **README rewritten** (618 → 358 lines): leads with the security story
  ("Cloning a repo shouldn't hand your agent to a stranger"), a 60-second
  quickstart above the fold, and steps 4–6 as hooks into the reference —
  no feature lost its coverage.
- **One docs source of truth**: the five hand-written site pages that
  mirrored markdown (how-it-works, primitives, library, strategy,
  mcp-capability-layer) are redirect stubs; unique content was ported into
  the markdown first. The site keeps the landing page, walkthrough,
  examples, and hub.

### Fixed

- Conformance smoke test: the sandbox now strips the `XDG_*` family so
  HOME-fencing actually fences opencode (an ambient `XDG_CONFIG_HOME` on
  the runner let it escape and read the empty machine config), and pins
  `--scope global` explicitly so the context-derived default scope can't
  silently break the whole matrix.
- Stale commands in docs: `stats` → `report usage`, bare
  `connect`/`disconnect` → `gateway connect|disconnect`, the nonexistent
  `report <run-id>` form → `report run <id>`, and every reference to the
  removed `agentstack codemode --write` (bindings come from the
  `tools_bindings` MCP tool response).
- The GitHub Action's usage example pinned a nine-releases-old tag.

## v0.11.0 — 2026-07-17

**Breaking: the CLI surface was rewritten.** Two simplification rounds since
v0.10.x collapse the 48-command surface to 14 visible commands, zero
features lost. Retired verbs and where they went: `bootstrap` → `setup`
(scripted path: `init` → `apply --write` → `use --write`);
`update`/`upgrade` → `lock --update` / `lock --upgrade`;
`runs`/`stats`/`analyze` → `report runs|usage|calls`;
`connect`/`disconnect` → `gateway connect|disconnect`; `pack init` →
`lib pack-init`. The broken or ungoverned surfaces (shell hook, dashboard
Pi passthrough, `codemode` verb, `lib migrate`, `audit --calls`) were
removed outright, and a parse test pins the retired names as rejected.

### Added

- **`run <cli> --locked` — the Protected tier.** A fail-closed, no-Docker
  pre-launch gate: enforced trust, strict lock verification (including pinned
  local server executables — a one-byte edit refuses the run) and policy
  admission under the machine ceiling. What passes is frozen into a sealed
  run grant the launch-scoped bridge serves verbatim — no mid-run
  re-derivation, mutating control-plane tools refused. `--plan` prints every
  gate decision and the grant digest without launching. Asserted end-to-end
  example: `examples/projects/locked-run/`.
- **`[extensions.*]` capability kind.** Native harness add-ons (pi
  TypeScript extensions, OpenCode JS plugins) as managed, content-pinned
  capabilities: strict integrity-root digests in the lock, zero bytes
  rendered for untrusted or drifted projects, copy-based delivery with an
  ownership ledger, re-verification under `run --locked`, and library/git
  sources. Honestly labelled provenance-only at runtime.
- **History-backed `restore`.** Every manifest-driven write is recorded
  first; `agentstack restore` lists history and reverts any entry — the same
  undo the dashboard button drives.
- **Implicit default profile.** A manifest with no `[profiles.*]` activates
  its inline servers and skills as the default set; profiles stay opt-in
  selectivity.
- Bare `agentstack` reads the project's actual state and prints the one next
  step; `doctor` covers hooks and discloses progressively.
- `[guard.project_roots]` — machine-owned, workspace-scoped extra write
  roots for the host guard ("sessions under `~/x` may also write `~/y`"),
  grantable only from the machine manifest so a project can never widen its
  own scope.
- `agentstack add server --target <cli>` scopes a newly added server to named
  CLIs (repeatable; unknown adapter ids are an error).
- Adoption-ladder documentation: README and the getting-started page now
  teach one six-step path (unify → verify → guard → trust → scale →
  confine), and the shipped `using-agentstack` skill detects a project's
  current step.

### Fixed

- Interactive init no longer aborts on an unreachable keychain — it stores
  what it can and reports failed refs by name.
- D3 executable pins now derive against the project root in the preferred
  `.agentstack/` layout (previously they could silently pin nothing).
- Copilot CLI 1.0.x conformance: `mcp list` moved behind `-i`; auth gate at
  exit 0.
- `apply --write` with blocked writes now exits nonzero (matching
  `use --write`) and its summary counts each target once: "Wrote N of C
  target(s); M blocked", with a note when a blocked target was partially
  written (e.g. instructions landed, server config refused). Previously a
  target written in one section and blocked in another counted in both
  columns ("2 of 2 written — 2 blocked") and the process exited 0.

### Security

- Locked-run keystone hardening from adversarial review: the grant bridge
  re-checks the *current* machine ceiling on consumption (a post-freeze
  machine tightening now refuses), the run-grant artifact is sealed under a
  machine-local HMAC, and the ambient-scope audit matches the project root.
  Honest limits are documented in `docs/ENFORCEMENT.md`.

## v0.10.3 — 2026-07-16

Burns v0.10.2, whose tag was pushed on a broken sandbox build. Identical
content on a green build.

## v0.10.2 — 2026-07-16

Fix: host-guard `[policy.filesystem]` deny globs now match across
equivalent path spellings, so a differently written path can no longer
slip past a deny.

## v0.10.1 — 2026-07-13

Security (F7): the `tools_execute` relay binds the narrowest
Docker-reachable interface instead of a broad wildcard.

## v0.10.0 — 2026-07-13

Experimental governed `tools_execute` (bounded TypeScript over the gateway,
Docker-only, machine-opt-in) and the cooperative host guard
(`agentstack guard`) wiring pre-tool-use hooks into 9 CLIs.

## v0.9.0 — 2026-07-11

Flight-recorder fill-out, security-review finding closures (SNI-match,
anti-SSRF IP classing, host normalization, length-framed symlink-safe
digests, atomic recorder append), and IO performance fixes.

## Earlier

Versions v0.2.0 through v0.8.1 predate this changelog; see the
[GitHub Releases page](https://github.com/Tarekkharsa/agentstack/releases)
and git history.
