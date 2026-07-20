# `add skill <source>` — speaking the ecosystem's grammar (priority 2)

Status: **implemented 2026-07-20** (session 1: `22a8161` — grammar +
discovery modules; session 2: `d7c3a4b` — staging, promotion, the verb,
witnesses, reference docs). Implements priority 2 of
[`skills-sh-learnings.md`](skills-sh-learnings.md) §10 — the acquisition
layer: source grammar, conventional discovery, `--list`, fail-loud
duplicates — landing on the priority-1 hardening
([`hardening-remote-ingestion.md`](hardening-remote-ingestion.md),
implemented). Grounded in a three-seam survey of the shipped plumbing and
hardened by a three-lens adversarial review (both 2026-07-20) whose
findings are folded in — most consequentially: staging moved off `/tmp`
onto the store's own filesystem so promotion is rename-only (the reviewed
copy fallback would have stripped `.git` and dereferenced hostile
symlinks), URL userinfo is rejected at parse, and the subpath/discovery
interaction is pinned down. All file:line references verified.

Priority 3 (the full one-write transaction on `use`'s primitives —
activation included) is explicitly **not** this design; §6 draws the line.

## What this ships

```
agentstack add skill anthropics/skills                  # discover, pick, preview
agentstack add skill anthropics/skills --skill pdf      # explicit, script-safe
agentstack add skill anthropics/skills --list           # inspect only
agentstack add skill https://github.com/o/r/tree/main/skills/pdf
agentstack add skill git@github.com:o/r.git --rev v1.2 --skill pdf
agentstack add skill ./my-skill
agentstack add skill ./my-skill --name code-review      # basename invalid → choose
```

One dry-run invocation shows everything (source, resolved commit,
discovered skills, scan findings, manifest diff); `--write` commits the
manifest entry, promotes the staged clone into the content store, and
records the lock pin — activation stays `use <p> --write`, exactly as the
verb's output says.

Blast radius is favorable (verified): **no integration test and no
`docs/reference.md` section covers `add skill` today** — the current
`NAME --path` shape can be replaced outright, per the no-users rule. The
MCP `agentstack_add_skill` JSON tool keeps its explicit
name+git/path fields unchanged.

## 1. Source grammar

New module `crates/cli/src/provider/source.rs`:

```rust
pub enum SkillSource {
    Local { path: PathBuf },
    Git { url: String, ref_: Option<String>, subpath: Option<String> },
}
pub struct ParsedSource {
    pub source: SkillSource,
    /// From an `owner/repo@skill` alias — merged with --skill (conflict = error).
    pub skill_alias: Option<String>,
}
pub fn parse_source(input: &str) -> Result<ParsedSource>
```

Accepted forms, in resolution order (each row from the learnings-§1 table,
minus the deliberate exclusions):

| Input | Result |
|---|---|
| `./dir`, `../dir`, absolute path | `Local` — **spelling is mandatory**; a bare `owner/repo` is never probed against the filesystem (learnings §10: same input, same meaning, on every machine) |
| `owner/repo` | `Git { url: "https://github.com/owner/repo" }` — regex `^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$`, no further slashes |
| `owner/repo@skill` | same + `skill_alias` |
| `https://github.com/o/r[.git]` | `Git` |
| `https://github.com/o/r/tree/<ref>[/<subpath>]` | `Git` with `ref_` + `subpath` |
| `https://gitlab.com/...` incl. subgroups and `/-/tree/<ref>[/<subpath>]` | `Git` |
| `git@host:o/r.git`, `ssh://…`, `file://…`, any `*.git` URL | `Git` (generic remote) |
| `<git-form>#ref` | fragment sets `ref_` — only parsed on git-shaped inputs, never on paths |

Rules carried over from the reviewed learnings decisions:

- **Canonical flags beat aliases.** `--skill`, `--rev`, `--subpath`,
  `--name` are the scriptable spellings; `@skill` and `#ref` are
  human conveniences. An alias *and* its flag disagreeing is an error, not
  a precedence puzzle — and the same rule covers a tree-URL subpath vs
  `--subpath`: both present and disagreeing is an error.
- **No credentials in URLs.** A URL whose userinfo carries a password
  (`https://user:token@host/…`) is **rejected at parse** with
  `credentials in git URLs are not accepted — use a git credential
  helper`; fail-never-normalize means we refuse rather than silently
  strip a secret that would otherwise print in the preview and persist
  into a committable manifest. The conventional bare-user SSH form
  (`git@host:…`, `ssh://git@host/…`) stays valid — user without password
  is transport addressing, not a credential.
- **Shorthand components must contain an alphanumeric** and are never
  `.`/`..` — defense in depth behind the local-path check that already
  runs first, so `../x` can never reach the shorthand branch even if the
  ordering ever changes.
- **Subpath segments are validated at parse** (`..` rejected) — mirroring
  `sanitizeSubpath` and our own `git_content_dir` guard, before any fetch.
- **`gitx::deny_weird_transport` runs at parse output** for every git form
  (`ext::`/`fd::` can also arrive via the generic-remote fallback).
- **Deliberately excluded** (learnings §1/§9): arbitrary "well-known"
  HTTPS documents, the `github:`/`gitlab:` prefix spellings (the URL forms
  cover disambiguation), and any filesystem-probing of shorthand.

No collision with `add from` (verified): `add from` recognizes only
`git:`-prefixed pack refs and exact provider ids — a bare `owner/repo`
matches nothing there today (`provider/mod.rs:558`, catalog/library names
can't contain `/`, registry ids are reverse-DNS). The grammar lives only
in `add skill`; `add from` is untouched.

## 2. Discovery — the conventional locations

New module `crates/cli/src/provider/discover.rs`:

```rust
pub struct DiscoveredSkill {
    pub name: String,          // dir basename — identity is external, never frontmatter
    pub rel_path: String,      // location inside the repo (the future `subpath`)
    pub description: Option<String>, // parse_frontmatter_description (already sanitized)
}
pub fn discover_skills(root: &Path) -> Result<Vec<DiscoveredSkill>>
```

Locations and depth discipline copied from the ecosystem convention
(learnings §2, ported with attribution in the code comment):

1. Root itself: `SKILL.md` at `root` → the repo *is* one skill; done.
2. Priority containers, one level deep each: `skills/`,
   `skills/.curated`, `skills/.experimental`, `skills/.system`, and the 24
   agent-convention dirs (`.claude/skills`, `.agents/skills`,
   `.codex/skills`, …). One extra level only for catalog layouts
   (`skills/<category>/<skill>/SKILL.md`). Never descend past a found
   `SKILL.md`.
3. **Recursive fallback (depth ≤ 5, pruning `node_modules .git dist build
   __pycache__`) only when the priority list finds nothing — and it is
   announced**: every fallback hit prints where it was found, and
   fallback-discovered skills are never auto-selected, even when there is
   exactly one; the user picks explicitly (learnings §2: nothing enters
   the pipeline from a location the user never saw named).

Policies that replace theirs (all previously decided):

- **Duplicate basenames across locations → hard error naming both paths.**
  Never first-wins.
- **`metadata.internal` is not read.** We parse frontmatter for
  `description:` only (verified: nothing in the codebase reads a
  frontmatter `name:`, and this design keeps it that way — skill identity
  stays the manifest key, chosen at add time).
- Missing/empty `description:` is a warning on the picker line (matching
  `lib add`), not a silent skip: a dir with `SKILL.md` is a skill.
- Every candidate's `name` (dir basename) is checked against
  `text::validate_name` at discovery; an invalid basename is listed as
  unselectable-without-`--name` rather than hidden. Selecting it without
  `--name` errors with the contract message; `--name` applies only when
  exactly one skill is selected.
- **Every displayed string from the repo is sanitized**: dir basenames
  (valid ones are print-safe by grammar, but *invalid* ones can carry
  ESC/bidi bytes and are exactly the hostile case) and anything handed to
  the picker as an item label go through `text::sanitize_line`, per the
  priority-1 rule that remote text never reaches a terminal raw.
- **Explicit subpath scopes discovery.** When `--subpath` or a tree-URL
  subpath is given, the discovery root is `clone_root.join(subpath)` — a
  `SKILL.md` there is the root-skill case, and the manifest `subpath` is
  the user's subpath joined with any discovered relative path beneath it.
  An explicit subpath never triggers whole-repo discovery; the user
  already navigated.

## 3. Staging — preview without persistent mutation

The priority-1 preview semantics (learnings §4, hardening design §A/§C
philosophy): a preview may fetch into **transient staging** but never
mutates manifest, lock, library, persistent store, or rendered targets.

Verified building blocks make this cheap:

- `Store::with_root(<any path>)` is already fully root-relative
  (`store.rs:37`; the store's own tests stage into temp dirs), and
  `store::checkout(&store, url, rev) -> (clone_root, head)` is the public
  non-`Skill` seam (`store.rs:250`) that runs on `gitx::Profile::Ingest`
  hardening automatically (via `store::run_git`).
- **Staging root (revised by review):**
  `paths::agentstack_home().join("stage").join(<runs::gen_id()>)` — on the
  store's own filesystem *by construction*, named with the same 32-byte
  random id the sandbox tempdirs use (`sandbox.rs:312/:784`), created
  fresh (an existing path with that id is an error, never reused —
  crash-leftover reuse would skip re-fetch and re-scan), permission-
  restricted 0700 immediately after creation (the `restrict` call the
  sandbox pattern pairs with it, `sandbox.rs:786`), and held by an RAII
  guard whose `Drop` does best-effort `remove_dir_all`. The original
  `/tmp`+pid sketch was rejected in review on three counts: predictable
  path in a world-writable directory, pid-reuse content reuse, and —
  decisive — a different filesystem than the store, which forces a copy
  fallback during promotion (see below). No `tempfile` dependency (it is
  dev-only, and stays that way).
- The stage path is never recorded anywhere persistent, so
  `lib.rs::is_temp_path`'s dangling-provenance concern doesn't arise.
  `doctor` gains a one-line sweep flagging stale `stage/` leftovers from
  crashed runs, with the `rm` remedy.

Every invocation (dry-run, `--list`, `--write`) stages fresh:
parse → `deny_weird_transport` → checkout into the staging store →
discovery → per-skill scan. Local-path sources skip staging (nothing
fetched; scanned in place).

**Promotion on `--write` — rename-only, never a filesystem copy.** Within
a single `--write` run, the bytes that were scanned must be the bytes
that land (no fetch-again TOCTOU window against a force-pushable remote).
New helper in `store.rs`:

```rust
/// Adopt a staged clone into this store's slot for `url` — only if the
/// slot is empty. fs::rename ONLY: staging lives on the store's
/// filesystem by construction, so rename succeeds; there is no copy
/// fallback. Returns the adopted clone root.
pub fn adopt_clone(&self, url: &str, staged_root: &Path) -> Result<PathBuf>
```

**A copy fallback is explicitly forbidden**, not just omitted — review
showed both shipped copy helpers are disqualifying: `fsx::copy_dir_all`
deliberately skips `.git` (`fsx.rs:60`), which would leave a wedged
non-git store slot that `ensure_git` never re-clones
(`fresh = !dest.exists()`, `store.rs:279`), and its file-copy path
dereferences symlinks (`fsx.rs:78`), which would vendor a hostile repo's
`link -> /etc/passwd` target bytes into the trusted store *and* produce a
lock checksum (`dir_digest` skips symlinks) that no fresh-cloning machine
can ever reproduce — a filesystem-layout-dependent false drift block.
`fs::rename` preserves `.git` and symlinks verbatim; if rename fails
anyway (slot created concurrently, exotic mount), the staged copy is
discarded and the flow falls back to a **pinned re-resolve** —
`Store::resolve` with `pinned_rev = <staged HEAD commit>` against the
real store — which is deterministic by commit, re-clones properly via
git, and re-scans before the lock write. Fetch-again is acceptable there
because the commit pin closes the TOCTOU; a byte-copy that silently
rewrites content is not.

If the real store *already* has a clone for the URL, staging is still
used for the preview (a dry run must never run `git checkout` inside the
shared real-store clone — that mutates its working tree, the exact
rev-sharing hazard the survey flagged); on `--write` the staged copy is
discarded and the existing clone is pinned-re-resolved to the staged
HEAD. Across separate invocations (dry-run today, `--write` tomorrow)
content can legitimately change; the `--write` run re-stages and
re-scans, and the lock pins what it actually saw.

Scan gate: the `lib.rs::scan_gate` shape (`lib.rs:2122` — scan a dir,
High blocks unless `--allow-flagged`, everything returned as warnings) is
extracted to a shared `pub(crate)` home so `add skill`, `lib add`, and
`install` stop growing parallel copies. `add skill` grows
`--allow-flagged` for parity.

## 4. The verb — selection, preview, one write

### CLI surface (replaces `AddSkillArgs` wholesale)

```rust
pub struct AddSkillArgs {
    /// owner/repo, git URL (incl. /tree/... paths), or ./local-dir.
    pub source: String,
    #[arg(long)] pub skill: Vec<String>,     // select by name; repeatable
    #[arg(long)] pub list: bool,             // inspect only, adds nothing
    #[arg(long)] pub rev: Option<String>,    // branch/tag/commit recorded in the manifest;
                                             // the exact commit is pinned in the lock
    #[arg(long)] pub subpath: Option<String>,
    #[arg(long)] pub name: Option<String>,   // manifest key override (single selection only)
    #[arg(long)] pub profile: Option<String>,
    #[arg(long)] pub allow_flagged: bool,
    #[arg(long)] pub write: bool,
}
```

### Selection

- Exactly one discovered skill (from a priority location) → auto-selected.
- Several → interactive **multi-select** in a TTY (`dialoguer::MultiSelect`
  with the `ColorfulTheme`, gated on the existing
  `core::util::confirm::is_interactive()`, with the numbered-stdin
  fallback shape `init.rs::read_numbered_secret_choice` established) —
  the crate's first multi-select, flagged as such. **All entries start
  unchecked** (opt-in, consistent with fallback hits never being
  auto-selected); confirming with zero selections aborts with
  `nothing selected — nothing to add`, never a silent no-op write.
  Non-TTY: `--skill` is required; the error lists every discovered name.
- `--skill` names are matched exactly (case-sensitive — the contract is
  lowercase anyway); an unmatched name errors listing what was found.
- Duplicate-basename error fires before any selection UI.

### Preview (one screen, dry-run and `--write` alike)

House style throughout (`→` cyan pending, `✓` green, `✗` red, `⚠` yellow,
`·` dimmed — the exact marker vocabulary `use`/`install` already speak):

```
→ anthropics/skills (git) — https://github.com/anthropics/skills at 8fa21c0d3b2e
  skills discovered: 12 (2 selected)
  ✓ pdf         skills/pdf         scan: clean        "Fill, split, and merge PDF files"
  ✓ docx        skills/docx        scan: 1 warning    "Create Word documents"
  · helper      found via recursive fallback at tools/helper — select explicitly to include
→ add 'pdf', 'docx' in ./.agentstack/agentstack.toml
→ add to profile 'backend' (the manifest's only profile)
  <manifest diff>
· after --write: activate with `agentstack use backend --write`

Dry run. Re-run with --write to update the manifest.
```

(The fallback-provenance `·` line and the profile-membership `→` line are
required output, not decoration — they render whenever their §2/§4 rules
fire.)

The header line follows `add_git_pack`'s existing sanitized
found-line (`add.rs:96`, already routed through `text::sanitize_line`).
Descriptions come pre-sanitized from `parse_frontmatter_description`.
Scan findings print per-skill with the standard `✗`/`⚠` markers; any High
finding without `--allow-flagged` fails the whole command **before** the
manifest diff renders — nothing is offered that can't be written.

### What `--write` does (and doesn't)

In order, all-or-nothing:

1. Duplicate manifest names hard-block first (before selection UI, before
   promotion), with the existing remedy message.
2. Everything above succeeded (fetch, discovery, validation, scan).
3. Promote the staged clone (`adopt_clone`, rename-only; pinned
   re-resolve fallback, §3).
4. Manifest: one `[skills.<name>]` entry per selected skill —
   `build_manifest_with` inserts exactly one entry per call (verified:
   no multi-entry loop exists today), so the verb calls it once per
   skill, threading the manifest text through; profile enrollment rides
   each call and is idempotent (`add_to_profile` dedups by exact name).
   Fields: `git = <canonical, credential-free url>`, `rev = <user's
   --rev/#ref if given, else omitted>`, `subpath = <user subpath +
   discovered rel_path>` (omitted for a root-skill repo). Local sources
   write `path = …` exactly as today.
   **Rev semantics, stated plainly:** the manifest `rev` records intent
   (branch/tag/commit); the **lock commit is authoritative** — verified
   `store.rs:71`, the lock's pinned rev wins over the manifest rev on
   every resolve — so a manifest `rev = "main"` is inert until
   `lock --update` relocks, at which point it re-tracks the branch tip.
5. Lock: upsert a `LockedSkill` per skill via the existing
   `install::locked_entry` shape — `rev` = the staged HEAD commit,
   `checksum` = `dir_digest` of the resolved subpath dir. After one
   `--write`, `doctor` is green (`present · SKILL.md ok`) and `install`
   is a no-op for these entries.

**Transaction guarantee (honest scope).** The manifest is written
**atomically first** (temp + rename) and is the source of truth; the lock
is derived from it. Two files can't be renamed as one, so the guarantee is
not two-phase commit — it is: if the lock write fails, the manifest still
stands and the error tells the user to run `agentstack lock`, which
reconciles it. The manifest never lands ahead with no path back to a
matching lock. Materialization (priority 3) runs after and is additive; it
reports per-target `✓`/`⚠`/`✗` outcomes and the command exits non-zero
naming any failed target — the manifest and lock stand, and
`use --write` completes the materialization. There is no cross-target
rollback (symlinks/copies are additive, not transactional); the
"report-and-leave-diagnosable" path is the guarantee, and `doctor` names
any half-materialized target.

**Immutable content (the shared-clone hazard, closed).** The store's
per-URL clone has a single mutable working tree, so checking out a second
revision of the same repo would change the bytes an already-materialized
symlink points at — across invocations, silently, while the first skill's
lock still pins the first commit. The write therefore **snapshots the
resolved, symlink-free content into a content-addressed immutable dir**
(`store/content/<digest>`) and materializes symlinks against *that*: a
different commit is a different digest is a different dir, so a later add
can never clobber an earlier skill's bytes. (Symlinks are rejected at
every ingestion gate first, so the snapshot copy is faithful.)

### Profile membership (replaces `add`'s current do-nothing)

Decided in learnings §10, now concrete:

- Zero profiles declared → no membership edit (the implicit default
  already activates every inline skill — verified
  `crates/cli/tests/default_profile.rs:38`,
  `profile_less_manifest_activates_and_locks_the_full_inline_set`).
- Exactly one profile → membership added automatically (via the existing
  idempotent `add_to_profile`), and the preview says so.
- Several profiles → `--profile` required; in a TTY, a `dialoguer::Select`
  asks (same pattern as `choose_delivery_mode`); non-TTY errors naming
  the profiles. **`--profile` naming a profile that doesn't exist is now
  an error with a did-you-mean** — the current silent-create behavior
  (`add_to_profile` creates any name it's given, verified `add.rs:1059`)
  is a typo trap; creation stays `agentstack_create_profile`/manifest
  editing. `add server`/`add from` keep their current behavior this
  round; harmonizing them is a listed follow-up.

## 5. Tests

Per house rules — one focused test per behavior, witnesses for the
security claims:

- `source.rs`: table test over every grammar row + the rejects (bare
  name, `owner/repo/extra/deep`, alias-vs-flag and subpath-vs-tree-URL
  conflicts, `..` subpath, `ext::` URL, and **`https://user:pass@host`
  credential rejection** — while `git@host:…` stays accepted).
- `discover.rs`: one fixture tree exercising priority order, the
  catalog-layout extra level, never-descend-past-SKILL.md, the announced
  fallback, and the duplicate-basename hard error.
- **Preview-mutates-nothing witness** (integration, the §3 security
  claim): dry-run `add skill file:///<fixture-repo>` → assert the real
  store root, manifest, and lock are byte-identical afterward, and the
  staging dir is gone.
- **Write-is-complete witness** (integration): `--write` against a local
  `file://` fixture repo with two skills → manifest entries with
  `git`+`subpath`, lock entries with commit+checksum, and the promoted
  store clone is a **functional git checkout** (`git rev-parse HEAD`
  succeeds in it — the regression the rejected copy fallback would have
  caused); then `use --write` in the same test materializes without
  `install`. A second case exercises the rename-failure path (pre-create
  the store slot) and asserts the pinned re-resolve lands the same
  commit.
- Scan-gate regression: a fixture skill with hidden Unicode blocks the
  whole command pre-diff; `--allow-flagged` admits it with warnings.

## 6. Scope line, follow-ups, and touched files

Priority 3 takes over from here: folding activation into the same
confirmed write (mode-aware: static → materialize; clean-at-rest /
zero-files → manifest+lock only) on `use`'s extracted primitives. This
design deliberately keeps `add skill --write` manifest+store+lock so P3
changes *one* seam, not two.

Named follow-ups (not silent scope creep): profile-selection rules and
the no-silent-create rule for `add server`/`add from`; scan-gate helper
adoption by `install.rs`; GitLab self-hosted URL forms beyond
gitlab.com; `lib add <source>` gaining the same grammar
(`lib add owner/repo --skill pdf`, learnings §10).

| Area | New | Modified |
|---|---|---|
| Grammar | `provider/source.rs` | — |
| Discovery | `provider/discover.rs` | — |
| Staging/promotion | — | `store.rs` (`adopt_clone`), staging guard in the verb |
| Verb | — | `cli.rs` (`AddSkillArgs`), `commands/add.rs` (`add_skill` rewrite), shared scan-gate extraction from `commands/lib.rs` |
| Docs | `docs/reference.md` gains the `add skill` section it never had | — |

No new dependencies (`dialoguer` is already shipped; `MultiSelect` is in
its default surface — verify the feature set at implementation, and if it
isn't, the numbered-stdin fallback becomes the only picker rather than
adding a feature flag without approval). No new unsafe. Two sessions:
(1) `source.rs` + `discover.rs` with their tests; (2) staging, promotion,
the verb rewrite, integration witnesses, and the reference-doc section.
