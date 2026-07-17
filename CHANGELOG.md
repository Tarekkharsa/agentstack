# Changelog

User-facing changes per release. The [GitHub Releases
page](https://github.com/Tarekkharsa/agentstack/releases) carries the built
binaries, checksums, and provenance attestations for each entry.

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
