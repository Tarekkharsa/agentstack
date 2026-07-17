# Native extensions as a governed capability kind

> **Status:** draft for maintainer review (E0)<br/>
> **Date:** 2026-07-16<br/>
> **Origin:** pi's extension system (<https://pi.dev/docs/latest/extensions>)
> is per-CLI only; the maintainer wants the same capability managed through
> agentstack across harnesses.<br/>
> **Queue position:** [`TODO.md` extensions lane](../../TODO.md#native-extensions-capability-lane-added-2026-07-16-post-cut)

## 0. Motivation

Several harnesses accept native executable add-ons that hook their lifecycle:
pi loads TypeScript modules from `~/.pi/agent/extensions/` and
`.pi/extensions/`, OpenCode loads JS plugins from
`~/.config/opencode/plugins/`, Claude Code and Codex have plugin packages,
Gemini CLI has extensions. These are the most powerful — and most dangerous —
capability surface a harness exposes: the code runs **inside the harness
process with full user permissions**, can intercept every tool call and
provider request, and today is installed by copying files by hand from
unreviewed sources.

AgentStack already governs skills (inert text), MCP servers (subprocesses the
gateway can fence), instructions, settings, and declarative hooks. Extensions
are the last unmanaged capability surface. For pi specifically the gap is
acute: pi has no MCP support by design, so extensions are its only custom-tool
mechanism — agentstack currently delivers pi only skills, `AGENTS.md`, and the
host guard.

## 1. What already exists (build on it, don't duplicate it)

- **pi is an adapter** (`crates/adapters/descriptors/pi.yaml`) and its
  descriptor already declares both extension directories via `ExtensionsSpec`
  (`crates/adapters/src/descriptor.rs:316`) — currently **discovery-only**
  (`discover_extensions`, `crates/adapters/src/lib.rs:178`), surfaced
  read-only in the dashboard.
- **The write path is proven.** The host guard already authors full native
  modules into these exact surfaces: `~/.pi/agent/extensions/agentstack-guard.ts`
  and OpenCode's `agentstack-guard.js` (`crates/cli/src/commands/guard.rs:511-571`).
  Those are fixed agentstack-authored templates; this design generalizes the
  delivery, not the templating.
- **The digest machinery for executable content exists.** D3's
  `integrity_root_digest` (`crates/core/src/digest.rs:188`) pins a directory
  of interpreted code strictly — symlinks are a hard error, `.git` is
  included — unlike the lenient skill `dir_digest`. Extensions reuse it
  unchanged.
- **Trust re-gating is automatic.** `digest_for` (`crates/trust/src/lib.rs:121`)
  hashes manifest + local manifest + lock; once extension checksums live in
  the lock, any byte change re-gates review with no new trust code.
- **Two adjacent mechanisms stay distinct:** the declarative `[hooks.*]`
  table (portable, compiled per-CLI via `crates/cli/src/render/hooks.rs`) and
  `[plugins.*]` recipes (Claude Code/Codex packaging,
  `crates/cli/src/plugin_recipes.rs`). Extensions do not replace either;
  unification is deferred (§9, E4).

## 2. Non-goals

- **No cross-CLI translation.** A pi extension is TypeScript against pi's
  `ExtensionAPI`; an OpenCode plugin is a different API. One extension entry
  targets exactly one CLI. The portable cross-CLI layer remains the
  declarative `[hooks.*]` table.
- **No agentstack-side execution.** AgentStack pins and delivers extension
  bytes; only the harness executes them. This keeps the strategy rule
  "arbitrary workflow code never executes on the host (via agentstack)"
  intact — the trust relationship is identical to stdio MCP servers.
- **No marketplace.** The starter catalog and the personal central library
  are the only sources, same as skills and servers.

## 3. Manifest shape

```toml
[extensions.checkpoint]
description = "Git checkpoint on every agent turn"
path = "./extensions/checkpoint"     # or: git = "...", rev = "...", subpath = "..."
target = "pi"                        # exactly one adapter id — code is CLI-specific
```

- `target` is **singular** (unlike skills' `targets` list) because extension
  code is written against one CLI's API. A future CLI pair sharing a format
  would revisit this; none exists today.
- Source forms mirror `Skill` (`crates/core/src/manifest/model.rs:697`):
  `path` or `git`+`rev`(+`subpath`). Same resolution and caching machinery.
- Render scope (user-global dir vs project dir) follows the active agentstack
  scope, the same way skills choose between `~/.claude/skills` and
  `.claude/skills` today.
- No `[extensions.*]` entry may name the guard's reserved artifact names
  (`agentstack-guard.*`) — validation error.

## 4. Lock pinning (security-sensitive)

Each extension gets a lock entry pinned with the **strict** root digest:

```toml
[[extension]]
name = "checkpoint"
target = "pi"
checksum = "sha256:…"   # integrity_root_digest over the source tree
```

- Reuses `integrity_root_digest` exactly (symlink anywhere = hard error,
  `.git` included). The lenient skill `dir_digest` is **not** acceptable for
  executable content.
- The pin records `target` alongside the checksum (E1 addition): the review
  bound this code to one harness, so retargeting an extension without
  re-locking is drift — verification blocks it even when the bytes are
  unchanged.
- `Lock::retain_*` pruning mirrors the executable-pin rules that landed with
  D3 (`retain_executables` semantics): pruning only from a complete manifest
  view, never from a profile-scoped subset.
- Trust consequence is automatic: lock bytes change → `TrustState::Changed` →
  re-review. The trust preview must label extension entries distinctly (§7).

## 5. Rendering

Extend `ExtensionsSpec` from discovery-only to a render target, per adapter:

- **Copy, not symlink.** Skills default to symlinks; extensions render as
  copies so the bytes the harness loads are the bytes that were pinned at
  render time. A post-render source edit changes nothing on the harness
  surface until a re-render, which requires passing trust + lock verification
  again. (A symlink would let live edits reach the harness between agentstack
  operations.)
- **Ownership ledger.** Rendered artifacts are recorded (marker file or
  ledger, matching the plugin-recipe `MARKER` pattern,
  `crates/cli/src/plugin_recipes.rs:20`) so `apply` can prune a removed
  extension's artifacts and must never touch: hand-installed extensions
  (surface via existing discovery + `adopt`, don't delete) and the guard's
  `agentstack-guard.*` files (explicit deny-list in the prune path).
- **Untrusted means inert (rule 3).** An untrusted bundle renders zero
  extension bytes into any harness directory — same gate as skill symlinks
  and server spawn, checked before any filesystem write.
- **First targets: pi and OpenCode.** Both surfaces already receive
  agentstack-authored modules from the guard. Gemini extensions and
  Claude Code/Codex plugin unification are deferred (E4).

## 6. Interaction with `run --locked`

Extension checksums are part of the lock, so `ensure_locked_inputs` covers the
*source* automatically. Additionally, before launch, the locked flow verifies
each **rendered copy** still matches its pinned digest (re-render or refuse on
mismatch) — otherwise a tampered rendered copy would load while the source
still verifies. Rendered extensions appear in `--plan`, the trust preview, and
the run report as declared executable surface, alongside D3 pins.

## 7. Honest posture (labels, not promises)

Extension code executes **in-process with the harness at full user
permission**. The policy ceiling, the gateway, and the egress fence cannot
observe or constrain it. What agentstack provides is provenance and content
binding: which bytes, from where, reviewed by whom, re-gated on any change.

Required wording surfaces:

- Trust review: extensions listed under a distinct "executable, in-process,
  ungoverned at runtime" heading — the strongest warning of any kind.
- `report` / posture output: rendered extensions listed with the same honesty
  as D3's "intentionally unpinned" labels.
- Docs: the enforcement matrix gains an extensions row whose runtime cells
  are honest "not enforced — provenance only".
- The text-oriented injection scan (`crates/cli/src/scan.rs`) is not
  meaningful for code; do not imply it was applied. No static analysis is
  claimed in v1.

## 8. Library and catalog

- Central library gains `kind: extension`; bodies under
  `~/.agentstack/lib/extensions/<name>/`, resolver and `lib` CLI verbs mirror
  skills. `agentstack search` covers them.
- Extensions are **not** loadable via the MCP zero-files mode
  (`agentstack_load`) — they are rendered artifacts for a harness, not
  context content. `agentstack_list_loadable` excludes them.
- Doctor: lock drift (existing), rendered-copy drift vs pin, broken source,
  unmanaged files in extension dirs surfaced read-only (existing discovery).

## 9. Staged implementation

Each stage is one supervised increment with its witness; E1 and E2 touch
trust/digest semantics and get line-by-line review.

- **E0 — approve this design.** Settle: `target` singular, copy-render,
  strict digest, guard-name reservation, pi+OpenCode first.
- **E1 — core + trust (supervised).** `[extensions.*]` manifest kind,
  `[[extension]]` lock pinning via `integrity_root_digest`, retain/prune
  rules, trust-preview labelling.
  *Witness:* a one-byte edit to any extension source file fails locked
  verification and re-gates trust review.
- **E2 — adapters + render (supervised).** `ExtensionsSpec` render for pi and
  OpenCode, ownership ledger, prune path, rendered-copy verification in the
  locked flow, `--plan`/report/posture surfaces.
  *Witnesses:* an untrusted bundle renders no extension bytes; removing an
  extension prunes exactly its artifacts and never touches unmanaged files or
  `agentstack-guard.*`.
- **E3 — library.** `kind: extension` entries, resolver, doctor coverage,
  docs + enforcement-matrix row. (Alongside, close the separate long-standing
  gap: library `hooks` support, noted as future work in
  `crates/cli/src/library.rs:10`.)
- **E4 — deferred until evidence.** Unify Claude Code/Codex plugin recipes
  and the guard payloads under the same render engine; Gemini extensions;
  any static-analysis or capability-declaration scheme for extension code.

## 11. Delivery notes (E2 + E3, landed 2026-07-16)

Decisions made during implementation, beyond the sections above:

- **Copy delivers the digest's exact file set.** `integrity_root_digest` was
  split to expose `integrity_root_files`; the copy walks that same strict,
  symlink-rejecting list, so a link appearing after the digest check can
  never smuggle bytes into a rendered artifact.
- **Render anchors at the digest's own root.** `ResolvedExtension` carries
  the `(anchor, declared)` pair the pin walked (manifest dir, git checkout
  root, or library body dir), and the renderer copies from it — all three
  source kinds deliver, not just inline paths.
- **Prune-when-untrusted is intentional.** Removing agentstack's own
  ledger-owned artifacts is the inert direction and proceeds for an
  untrusted project; only RENDERING (adding executable bytes) is gated.
- **The ledger is hostile input.** It lives inside the (possibly
  repo-controlled) extension directory, outside the trust digest. Keys must
  be plain basenames — checked at the single load choke point and again at
  the materialize sink (adversarial review found and closed two
  path-traversal routes: forged ledger keys and extension names containing
  separators; the latter is also a validation error,
  `InvalidExtensionName`).
- **Git extension bodies require a `subpath`** so the checkout's `.git`
  never enters a reproducible pin; digests anchor at the checkout root.
- **`rendered-verify`** in `run --locked` compares the delivered copy to the
  authoritative lock pin (not the ledger's record); a vanished-but-ledgered
  artifact is tampering, honest absence is not an error.

## 10. Open questions for E0

1. Copy-render invalidates pi's live-reload development loop for extension
   authors. Acceptable (author in the library, re-render to test), or does
   dev mode need an explicit, labelled symlink escape hatch? (Recommend: no
   escape hatch in v1; honesty beats convenience.)
2. Should project-scope rendering into `.pi/extensions/` be supported at all,
   given pi itself trust-gates that directory — or is user-global scope
   enough for v1? (Recommend: follow the active scope, both supported, same
   as skills.)
3. Does the enforcement matrix need a new posture slug for "provenance-only
   executable capability", or does the D3 "declared executable surface"
   language stretch to cover it?
