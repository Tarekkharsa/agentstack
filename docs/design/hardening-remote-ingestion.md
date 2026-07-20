# Hardening remote ingestion — implementation design (priority 1)

Status: **proposed, awaiting approval**. Implements priority 1 of
[`skills-sh-learnings.md`](skills-sh-learnings.md) §10: the three hardening
prerequisites that must land before `agentstack add skill <any-repo>` invites
arbitrary ecosystem content in. Grounded in a full survey of every affected
seam, then hardened by a three-lens adversarial review whose findings are
folded in (both 2026-07-20) — notably the Ingest/Sync git-profile split, the
MCP-mode error sink, the pack-name gate, and the reclassification of the
gateway truncate as a live remotely-triggerable panic. All file:line
references verified against the working tree.

**Security-sensitive** — this design touches hostile-input handling on paths
that feed terminals, MCP clients, and filesystem writes, and it fixes one
live path-traversal defect (§C.1). Review line by line.

Three independent workstreams, each one session:

- **A. Display/context text hygiene** — a shared sanitizer applied where
  remote text enters anything a human or agent reads.
- **B. Git invocation hardening** — one hardened runner: protocol allowlist,
  LFS suppression, prompt suppression, timeout.
- **C. The name contract** — one grammar for skill names, enforced at every
  ingestion boundary and dangerous join, fixing a live traversal on the way.

Non-goals (deliberately out of scope): the `add skill <source>` verb itself
(priority 2), the auth-retry ladder and shallow clones (UX, not hardening),
harmonizing the *server* name contract (follows later on the same module),
and any change to scan-gate semantics.

---

## Threat model recap

Once `add skill owner/repo` exists, these inputs are attacker-controlled:
SKILL.md frontmatter and bodies, `pack.toml` member names/paths, git URLs
and the transports they select, registry/search API responses, and upstream
MCP servers' self-reported tool metadata. Today's gaps, verified:

1. Remote text reaches terminals and MCP clients with at most char-count
   truncation. `serde_json` escaping is wire-level only: a decoded ``
   is a live ESC byte again, and raw C1 controls (0x80–0x9F) and bidi
   overrides pass through JSON entirely untouched (verified empirically).
2. Every git spawn inherits the full parent environment: no
   `GIT_TERMINAL_PROMPT`, no protocol restriction, no LFS suppression, and
   **no timeout anywhere** — a hung transport or `/dev/tty` SSH prompt
   blocks the whole process indefinitely.
3. Skill names from remote sources flow into manifest keys, lock entries,
   and **filesystem joins** with almost no validation — including one join
   that is traversable today (§C.1).

---

## A. Display/context text hygiene

### A.1 The module

New module `crates/cli/src/text.rs` (no new dependencies — `regex` is
already a `cli` dep, and most of this is char-class logic that doesn't need
regex at all):

```rust
/// One-line metadata for display or agent context: strips terminal escape
/// sequences, raw C0/C1 controls, and invisible/bidi/tag characters;
/// collapses newlines to spaces; trims. Tabs survive.
pub fn sanitize_line(s: &str) -> String

/// Multi-line variant for error text: same stripping, but `\n` survives.
pub fn sanitize_block(s: &str) -> String

/// Char-boundary-safe truncation with ellipsis (replaces the duplicate
/// helpers in search.rs and lib.rs, and the gateway's byte-level cap —
/// which today PANICS on a mid-char boundary, see §A.2 #4). scan.rs keeps
/// its private copy: it runs on already-escaped ASCII findings and that
/// security-reviewed file stays untouched by this design.
pub fn truncate_chars(s: &str, max: usize) -> String

/// `truncate_chars` after first-line extraction (replaces mcp_server's
/// `one_line`).
pub fn one_line(s: &str, max: usize) -> String
```

What `sanitize_line`/`sanitize_block` remove, as a state machine over
chars (their `sanitize.ts` regex stack, ported and extended):

| Class | Detail | Action |
|---|---|---|
| OSC / DCS / PM / APC | `ESC ]`, `ESC P`, `ESC ^`, `ESC _` … until BEL or ST (`ESC \`), unterminated → strip to end | drop whole sequence |
| CSI | `ESC [` params(0x30–3F) intermediates(0x20–2F) final(0x40–7E) | drop whole sequence |
| Bare ESC + one char | covers `ESC 7`, `ESC c`, malformed lookalikes | drop both |
| Raw C1 | U+0080–U+009F (8-bit CSI/OSC/DCS/ST forms — untouched by JSON escaping) | drop |
| Remaining C0 + DEL | except `\t` always; `\r`/`\n` per the newline rule below | drop |

Newline rule, stated once: `sanitize_line` converts each `\r`/`\n` to a
space, then collapses whitespace runs to one space and trims (`"foo\nbar"`
→ `"foo bar"`, never `"foobar"`); `sanitize_block` preserves `\n` and
drops `\r`.
| Invisible/bidi/tag | the exact set `scan.rs::invisible_label` already names: ZWSP/ZWNJ/ZWJ/word-joiner/BOM/soft-hyphen/U+180E, bidi embed U+202A–202E, bidi isolate U+2066–2069, tag chars U+E0000–E007F | drop |

Two deliberate divergences from prior art, with rationale:

- **We drop, `scan.rs` escapes visibly.** Different jobs: `scan.rs`
  *reports* hostile content, so `escape_invisible` renders it as `\u{XXXX}`
  for the human reviewing a finding — that stays exactly as is. Display
  hygiene *neutralizes* content in passing surfaces (a search row, a tool
  description); rendering every stripped byte would make lists unreadable.
  The scan gate remains the place where the user is *told* about hostile
  bytes; `text.rs` just refuses to be their delivery vehicle.
- **We strip invisible/bidi Unicode too; vercel's sanitizer doesn't.**
  Their stack stops at escape sequences; a RLO override in a description
  still reorders their list output. Ours reuses scan.rs's character
  knowledge so display surfaces can't be visually spoofed either.

Rust note (TS mental model): the API takes `&str` and returns `String` — a
borrowed view in, an owned buffer out, like `(s: string) => string` but the
compiler enforces the caller keeps the original alive for the call. The
sequence-walker is a plain `match` on a small enum state — a discriminated
union with exhaustive switch, no regex needed for the ESC state machine.

### A.2 Where it applies — chokepoint inventory

Sanitize **at ingestion** where a single parse point exists, **at the sink**
where text arrives pre-assembled. Every row verified in the survey:

| # | Site | Change |
|---|---|---|
| 1 | `parse_frontmatter_description` (`library.rs:36`) | `sanitize_line` on the returned description — covers `lib list`, `LibraryProvider` search, MCP `list_loadable` descs, and `initialize.instructions` in one move |
| 2 | `explain.rs:822` duplicate frontmatter parser | **delete it**; call the shared `library.rs` parser (also fixes its missing block-scalar handling). Any fix applied only to #1 misses this path today |
| 3 | Registry ingestion `registry.rs:63-67` (`to_candidate`) | `sanitize_line` on `description`/`title` at parse — the one true remote-HTTP origin; downstream `search.rs`, `mcp_server::search_text`, dashboard all inherit the clean value |
| 4 | Gateway `namespace_tool` (`gateway.rs:1331-1348`) | Three parts. (a) `sanitize_line` + `truncate_chars(600)` on the upstream description — the current byte-level `String::truncate(600)` **panics** when byte 600 falls mid-UTF-8-char, i.e. a hostile upstream MCP server can crash the gateway's tool listing today; this is a remotely-triggerable DoS fix, not hygiene, and gets a multibyte-at-cap regression test. (b) an upstream tool whose *name* contains hostile bytes is **dropped from the listing entirely** (with a stderr note) rather than sanitized-and-remapped: dispatch forwards the bare name verbatim to the upstream, so a changed name would be uncallable, and a name-level injection is grounds to hide the tool — the same "invisible, not just refusable" shape as the policy firewall. (Implementation refinement over the earlier sanitize-and-remap wording.) (c) Walk the upstream `inputSchema` and `sanitize_line` every `description`/`title` **string value** recursively — schema text is agent-context the model reads to decide calls. Property *keys* and structural values stay verbatim (explicit exemption: renaming keys breaks the call contract with the upstream server, same rationale as the skill-body exemption). Same treatment for `tools_search` summaries/cards (`gateway.rs:1144-1170`, `mcp_server.rs:1529`) |
| 5 | Trust review listing (`trust.rs:230-450`) | `sanitize_line` on every interpolated manifest-declared string: server `command`/`args`/`url`, extension `name`/`target`, skill names — this is the consent screen; it must be unspoofable |
| 6 | Top-level error sink (`main.rs:34`) | `eprintln!("error: {}", text::sanitize_block(&format!("{err:#}")))` — catches every `.with_context()` chain on the **CLI stderr path**, incl. `store.rs:284`. Not the only error sink: the MCP-mode error surface is row #9 |
| 7 | `mcp_server::one_line` (`:1872`), `search.rs::truncate` (`:154`), `lib.rs::truncate` (`:2191`) | replace with `text.rs` equivalents (dedup; sanitization already done upstream by #1/#3) |
| 8 | `add from` stdout (`add.rs:58-64` found-lines, `:96-104` pack plan) | `sanitize_line` on `candidate.name`/`candidate.id` and the pack's self-reported name/URL at print. These are remote (`PackToml.name` via `gitpack.rs:258`; registry `id` is the **raw un-normalized** `s.name` — only `name` passes `clean_name`) and they hit the terminal on a *dry run*, before any `--write` — the first human-read surface of `add from git:…` must be unspoofable |
| 9 | MCP tool-error sink (`mcp_server.rs:945/:957/:963`) | `sanitize_block` on the `format!("Error: {e}")` text. Row #6's `main.rs` chokepoint only covers the CLI stderr path — under `agentstack mcp`, handler errors (which embed registry HTTP text and git stderr via `provider::resolve`) are formatted into JSON-RPC results and never reach `main.rs` |

**Explicit exemption — skill bodies.** `agentstack_load`'s `instructions`
field (`mcp_server.rs:2297-2323`) returns the full SKILL.md body verbatim,
and stays that way. The body is the product being delivered; it is
integrity-checked against the lock, and hostile bytes in it are the **scan
gate's** jurisdiction (hidden Unicode already blocks at install). Silently
rewriting delivered content would break the "bytes match the lock" story.
The design makes this exemption explicit so nobody "fixes" it later.

Doctor JSON / dashboard output inherit sanitized values via #1/#3; no
separate work.

### A.3 Test

One table-driven unit test in `text.rs`: OSC title-spoof, CSI
clear-screen, raw C1 CSI (0x9B), RLO bidi, zero-width joiner, unterminated
OSC, plain UTF-8 text with tabs, a multi-line input through `sanitize_line`
(pins the newline rule: `"foo\nbar"` → `"foo bar"`), and a multibyte char
straddling the `truncate_chars` cap (the gateway-panic regression) —
asserting exact outputs. Plus one
proptest invariant (security claim → witness): for arbitrary input, the
output of `sanitize_line` contains no char in any banned class and no ESC
byte. `proptest` is dev-only and already blessed; if pulling it into `cli`
dev-deps is unwanted, the invariant becomes an exhaustive-class unit test
instead — flagged as the one dependency question in this design.

## B. Git invocation hardening

### B.1 One runner

New module `crates/cli/src/gitx.rs`; `store.rs::run_git` and
`lib.rs::{git_out, git_ok}` plus the four direct `Command::new("git")` sites
in `lib.rs` (`:1731`, `:1793`, `:1875`, `:1891`) all route through it.
Extract-don't-rewrite: the function bodies move, the call shape
(`args`, optional cwd via `-C`, captured output, stderr-in-error) is kept.
Like `sys.rs` concentrates the workspace's unsafe surface, `gitx.rs`
concentrates its git-spawn policy — greppable in one file.

One runner, **two profiles** — because not all git targets are hostile:

- **`Ingest`** — fetching *content we're about to trust-gate* (store
  resolve/clone/fetch, `ls_remote_tags`, gitpack, `lib add --git`). Full
  hardening, including prompt suppression: ingestion must never wedge on
  interactive auth.
- **`Sync`** — the central library's *first-party* remote
  (`lib.rs` sync: clone/fetch/pull/push at `:1731/:1793/:1875/:1891/:1976`).
  This is the maintainer's own repo and legitimately needs credentials — a
  passphrase-protected key or an HTTPS credential prompt must keep working
  (the pull path even classifies auth-failure stderr today,
  `lib.rs:1906-1914`). `Sync` gets the protocol allowlist, LFS suppression,
  and the timeout, but **not** `GIT_TERMINAL_PROMPT=0` and **no**
  `GIT_SSH_COMMAND` override. This is an explicit, documented behavior
  split — without it, hardening would silently break `lib push`/`pull` for
  anyone whose key isn't in an agent.

Environment and flags per profile:

| Setting | Value | Why |
|---|---|---|
| `GIT_TERMINAL_PROMPT` | `0` (**Ingest only**) | fail fast instead of prompting; `.output()` nulls stdin but git prompts via `/dev/tty` directly. `Sync` leaves it unset so first-party auth prompts still work |
| `GIT_ALLOW_PROTOCOL` | `https:ssh:file` | **narrower than vercel's** `https:http:ssh:git:file` — we drop plaintext `http:` and `git:` deliberately; a security tool doesn't fetch skills over cleartext. `file:` stays for tests and local flows, `ssh:` for private repos. Env-overridable escape hatch: respect a pre-set `GIT_ALLOW_PROTOCOL` from the caller's environment (explicit user choice wins) |
| `GIT_LFS_SKIP_SMUDGE` | `1` | LFS content never downloads during checkout (skills are text) |
| `-c filter.lfs.smudge=` `-c filter.lfs.process=` `-c filter.lfs.required=false` | flags | the *other* failure mode: `git-lfs` **not installed** → an LFS-attributed repo aborts checkout with "command not found"; these make the filter inert (vercel ships both layers; both are needed) |
| `GIT_SSH_COMMAND` | `ssh -oBatchMode=yes` (**Ingest only**, and only if unset) | SSH prompts via `/dev/tty`, bypassing stdin nulling; BatchMode fails fast. A user-set `GIT_SSH_COMMAND` is never overridden; `Sync` never touches it |

Transport pre-check at the URL entry points (`Store::resolve`,
`ls_remote_tags`, `lib add --git`, `sync_init`): reject a URL matching
`^(ext|fd)::` case-insensitively with
`unsupported git transport '<prefix>' — https, ssh, or file only`.
Belt-and-suspenders: `GIT_ALLOW_PROTOCOL` gates `ext::` on modern git, but
vercel's comment notes older gits don't reliably route it through the
allowlist, and the check costs one `starts_with`.

### B.2 Timeout — std-only, no new dependency

Constraint (verified): no timeout crate is reachable from `cli` production
code (`wait-timeout` exists only as a transitive dev-dep via proptest), and
`tokio` is confined to `egress` by architecture rule. The codebase already
contains the pattern to copy: `gateway.rs::StdioChild::wait_for_exit`
(poll `try_wait` against an `Instant` deadline) and the process-group
primitives in `sys.rs` (the workspace's single `#[allow(unsafe_code)]`
file — **no new unsafe is added by this design**; `gitx` calls the existing
wrappers).

Algorithm, spelled out because the pipe-deadlock detail is load-bearing:

1. Spawn via `sys::spawn_in_new_process_group` (unix) with
   `Stdio::piped()` for stdout/stderr, stdin null.
2. `take()` both pipes and hand each to a reader thread that drains into a
   `Vec<u8>` — draining must be concurrent with waiting, otherwise a child
   that fills the OS pipe buffer (a chatty clone) blocks forever and the
   poll loop misreads it as a hang.
3. Poll `try_wait()` every 50ms against `Instant::now() + timeout`.
4. On deadline: `sys::signal_group(SIGTERM)` → 300ms grace →
   `sys::signal_group(SIGKILL)`, then join the reader threads (they get EOF
   once the child dies) and return
   `git <args> timed out after <n>s — set AGENTSTACK_GIT_TIMEOUT_MS to raise
   the limit, or clone manually and use a local path`.
5. On normal exit: join readers, preserve today's error shape
   (`git {:?} failed: {stderr}`).

Timeout default **300s**, override `AGENTSTACK_GIT_TIMEOUT_MS` — the same
const-plus-env shape as the existing `AGENTSTACK_STDIO_START_MS`. One knob
for all git ops (network and local): a local `rev-parse` will never hit it,
and two tiers isn't worth the second knob. Non-unix fallback: plain
`child.kill()` without the group signal (best-effort; the tool's supported
platforms are macOS/Linux today).

Rust note: moving each `ChildStdout` into its thread is an ownership
transfer — after `thread::spawn(move || …)` the parent *cannot* touch the
pipe again, which is exactly the guarantee that makes the concurrent drain
race-free. The TS analogue is transferring a stream to a worker, except
here it's compile-time enforced.

### B.3 What we deliberately don't do

- No auth-retry ladder (`gh` fallback, SSH retry) — UX, later, and its
  quota/`gh` shell-outs deserve their own review.
- No shallow clones — the store needs tags and pinned revs; revisit with
  priority-2 fetch volume, not here.
- No change to cache naming: `store.rs::sanitize` (every non-alnum → `-`)
  already flattens URLs into single path components; traversal-safe as is.
  Known cosmetic collision potential is accepted and documented in code.

### B.4 Test

Two focused tests: (1) env assembly extracted as a pure
`fn hardened_env(profile, overrides…) -> Vec<(String, String)>` asserted
directly for **both profiles** — `Ingest` sets prompt suppression, `Sync`
does not, both set the allowlist and LFS flags, and a caller's
`GIT_ALLOW_PROTOCOL`/`GIT_SSH_COMMAND` win; (2) the
timeout path, using a stub `git` script (sleeps 60s) prepended to `PATH` in
a tempdir, asserting the runner returns the timeout error within the test's
own small limit and the child is gone. The transport pre-check gets one
assertion alongside (1). Existing `store.rs` tests keep passing unchanged —
they use `file://` URLs, which stay allowed.

## C. The skill-name contract

### C.1 The live defect this fixes — flag for line-by-line review

`crates/cli/src/commands/add.rs:492` builds
`format!("instructions/{}.md", instr.name)` and later joins it into
`ctx.dir.join(dest)` (`:561`). `instr.name` is a `PackMemberToml.name`
parsed verbatim from a **remote** `pack.toml` (`gitpack.rs:112-116`) — the
`contained()` check guards the member's `path` field, never its `name`.
A pack declaring `name = "../../../<anything>"` writes outside the project
directory on `add from git:… --write`. Pack *skill* extraction is safe by
construction (keys off the containment-checked `path`); instruction
extraction keys off `name` and is not. The name contract closes this; the
fix ships in the same change with its own regression test.

### C.2 The grammar

```
^[a-z0-9][a-z0-9._-]{0,63}$        (1–64 chars, must start alphanumeric)
```

- **Lowercase only.** Not style: the data model is case-sensitive
  (`IndexMap` keys, `Vec` linear `==` finds) but macOS's default filesystem
  is not — the survey confirmed `lib add PDF` then `lib add pdf` produces
  two index entries whose bodies silently share one directory, last writer
  owning the bytes. Lowercase-only makes the collision unrepresentable
  going forward.
- Starts alphanumeric → no dotfiles, no `-`-prefixed names that parse as
  flags in later shell/CLI contexts, and `.`/`..` are unrepresentable.
- No separators of any kind → always exactly one `Normal` path component.
- 64-char cap: manifest keys, lock entries, and list output all stay
  readable; nothing in the ecosystem sample comes close to it.
- **Fail, never normalize.** vercel's `sanitizeName` lowercases, collapses,
  strips, and falls back to `'unnamed-skill'` — three different normalizers
  for three contexts, and a hostile name silently becomes a different valid
  one. We reject with the reason and the remedy:
  `invalid skill name '<name>' — lowercase letters, digits, '.', '_', '-';
  must start with a letter or digit; max 64 chars. Pass --name to choose a
  valid name.` The `--name` override is the escape hatch when a *source's*
  name is invalid (matters for priority 2, where frontmatter names arrive;
  today names are always user-supplied).

### C.3 Enforcement matrix

One function, `text::validate_name(&str) -> anyhow::Result<()>` (same new
module — it's the same "hostile strings" domain). It is **strictly
narrower** than `valid_lib_name` — it accepts a subset of the old language
(no uppercase, no non-ASCII, ≤64 chars, must start alphanumeric) — and it
applies to **skill names and pack names only** this round.
`valid_lib_name` is **retained, not deleted**: it has six call sites, and
four of them guard non-skill paths — library server/hook/extension adds
(`lib.rs:401/:778/:1005`) and two traversal-guard joins
(`lib.rs:669` servers, `:923` hooks) — which keep it until the server-name
harmonization follow-up. Removal paths (`lib remove`, `remove`, doctor's
rename remedy) **never** validate the name they're removing, so a
pre-contract entry (`PDF`, a 65-char name) remains removable/renamable —
otherwise doctor's remedy would be unreachable for exactly the entries it
flags. Enforcement boundaries from the survey:

| Boundary | Site | Today | Becomes |
|---|---|---|---|
| `add skill NAME` (CLI) | `add.rs:805` | duplicate-check only | validate first |
| `agentstack_add_skill` (MCP) | `add.rs:1136` | `!is_empty()` | validate |
| `lib add` — **skills only** | `lib.rs:113` | `valid_lib_name` | `validate_name` |
| Pack's own name (`PackToml.name`) | `gitpack.rs:190/:258` parse | **none** — remote string becomes the `[packs.<name>]` ledger key and the `[servers.<name>]` key (`add.rs:359/:447/:513`) | `validate_name` at the parse gate, same all-or-nothing rejection as members |
| Pack members (skills + instructions) | `gitpack.rs:190-207` parse | path checked, **name unchecked** | validate every `PackMemberToml.name`; invalid member → whole pack rejected (all-or-nothing, matching the existing atomic-add semantics) |
| Pack instruction extraction | `add.rs:492` | **none — traversable** | validated upstream by the parse gate; the format site additionally asserts single-component as defense in depth |
| Materialization join | `render/skills.rs` `plan()`/`materialize()` | none | validate each active name before `skills_dir.join(name)`; a bad name in a hand-edited manifest fails the plan with the standard message, never reaches the join |
| Doctor | new check | — | flag any manifest/library/lock name violating the contract, and any case-folding collision pair among *existing* entries, with the rename remedy |

Deliberately **not** enforced in `core`'s manifest parser: `core` stays a
format layer (parse defensively on size, which it does — 8 MiB cap), and
the contract lives in one `cli` module the way spawn policy lives in
`gitx` and unsafe lives in `sys`. Rejecting at load would also make every
command fail on a legacy manifest instead of `doctor` explaining it —
fail-closed *at the dangerous operations* (write, join, extract), diagnose
everywhere else. No users → no migration shim; the maintainer's own
library is kebab-case and passes.

Server/extension/hook names keep `valid_lib_name` (their current, weaker
check) this round; harmonizing them onto `validate_name` is a listed
follow-up, not silent scope creep — the registry's `clean_name` normalizer
(`provider/mod.rs:577`) produces names the skill grammar would reject, and
that reconciliation deserves its own decision. The one exception pulled
forward is the pack name (matrix row above), because it writes manifest
keys from a remote string *today*.

Frontmatter `name:` is **still never read** in this design (verified: no
code reads it today). The contract exists now so priority 2's
frontmatter-driven discovery lands on a defined grammar instead of
inventing one mid-implementation.

### C.4 Tests

One table test: hostile names (`../x`, `.hidden`, `-flag`, `PDF`, 65 chars,
`a b`, `日本語`, empty, `a/b`, plus valid `sql-review`, `v1.2_beta`).
One proptest invariant (path-safety is a security claim): any accepted name
yields exactly one `Component::Normal` when parsed as a `Path`, and
`dir.join(name)` starts with `dir`. One regression test for C.1: a pack
with a traversing instruction name is rejected at parse, and nothing is
written outside the project dir; the same test covers a pack whose own
`name` violates the contract.

---

## Sequencing and touched files

Any order works; suggested: **C first** (fixes the live traversal), then A,
then B. Each session ends green on `cargo fmt --check`, `cargo clippy
--workspace --all-targets -- -D warnings`, and `cargo nextest run -p
agentstack-cli <touched filters>`.

| Area | New | Modified |
|---|---|---|
| A | `cli/src/text.rs` | `library.rs`, `explain.rs`, `provider/registry.rs`, `gateway.rs`, `mcp_server.rs`, `commands/{search,lib,trust}.rs`, `main.rs` |
| B | `cli/src/gitx.rs` | `store.rs`, `commands/lib.rs` |
| C | (in `text.rs`) | `commands/{add,lib,doctor}.rs`, `provider/gitpack.rs`, `render/skills.rs` |

No new dependencies (`proptest` as a `cli` dev-dep for the two invariants
was approved by the maintainer 2026-07-20 — already blessed for
`trust`/`policy`). No new `unsafe`.
No changes to `trust`, `policy`, or `core` beyond none-at-all — the entire
design lives in the `cli` crate.
