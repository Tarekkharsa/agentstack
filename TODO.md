# AgentStack work queue

> **Purpose:** the only ordered product-wide work queue.
>
> **Status:** rebuilt 2026-08-12 against agentstack 0.18.1. Closed work and
> the reasoning behind it live in git history and in `CHANGELOG.md`; this file
> holds only what is open. The queue is the maintainer's to reorder; deviations
> edit this file, never [`STRATEGY.md`](STRATEGY.md).

## The activation study

1. **[ ] Clear §0 of the study kit.** `docs/design/activation-study.md` still
   opens with **DO NOT RUN THIS KIT UNTIL §0 IS CLEARED**. It pins the study to
   `v0.18.0-rc.2`; update §3's install line and the version floors in §3 and
   Appendix B3 to the release the study will actually run against, and settle
   §0.1's B-B note (the JSON surfaces still carry the bare semver).
2. **[ ] Run the study**, then fix the three blockers it names. The blockers are
   its *output*, not a backlog — §7 holds three empty slots filled in after all
   five sessions — so nothing here can start before the study runs.

## Enforcement gaps, still open

Each reverses or narrows an adopted design, so none is a cleanup; each is a
consent decision that stays the maintainer's.

3. **[ ] G4 — `require_trust` reaches only `Gateway::from_frozen`.** The host and
   lease constructors still pass `require_trust: false`
   (`crates/cli/src/gateway.rs:449,1933`). Extending it revokes "naming the
   manifest dir is the consent."
4. **[ ] G5 — no headless path for unsigned content.** `receive --yes` refuses
   it. A `--consented-digest` acceptance bound to previewed bytes would open
   one, if that path is wanted at all.
5. **[ ] G8 — `--allow-unresolved` writes a literal `${NAME}` into live config.**
   Omit the key instead, or remove the flag.
6. **[ ] G15 — `max_wall_seconds` is inert.** It parses into the workflow model
   (`crates/core/src/manifest/model.rs:332`) and nothing enforces it.
7. **[~] G3 — an env-value-only owned-server refresh still auto-repins trust.**
   The command-line half shipped: a refresh that moves what a server *executes*
   withholds the grant. Env is executable-equivalent for an interpreter-launched
   server (`NODE_OPTIONS`, `LD_PRELOAD`, `PATH`), so the residual is real and is
   disclosed in `docs/ENFORCEMENT.md` rather than hidden. Closing it makes every
   env rotation a review.
8. **[~] G6 — Cursor file writes never reach the guard.** The payload-shaped
   classifier shipped; Cursor exposes no pre-write hook, so only
   `beforeShellExecution` and `beforeReadFile` are wired. Waits on a surface
   someone else has to ship.
9. **[~] G19 — Kiro gets no host guard.** Its descriptor records MCP config
    only, so there is no hook path to install into. Marked `NOT_WIRED` (a fact
    about agentstack) rather than implying none could exist. Same shape as G6.

## Honest-surface gaps from the P8 evidence pass

Each is a surface that promises what a later command refuses, or a contract the
docs describe and the code does not emit.

10. **[x] P8-G2 — `use`'s dry run promises what the write refuses.** Closed by
    #50: on an untrusted project the preview now closes "1 target would be
    BLOCKED, so --write would refuse and write nothing" and never names
    `--write`; pinned by
    `crates/cli/tests/a_dry_run_predicts_its_write.rs::use_dry_run_does_not_promise_a_write_its_own_gate_would_refuse`.
11. **[ ] P8-G3 — `apply`'s dry run has the same shape.** "0 targets would
    change. Re-run with --write to write." above an `apply --write` that exits 1
    with "nothing was delivered".
12. **[x] P8-G4 — `use --write` reports activation for a run that failed.**
    "activated 'backend' on 4 targets (wrote 0)" prints above `error: 3 targets
    blocked` and exit 1. **Closed in `64109b5`, verified 2026-08-13:** a fully
    blocked write now prints "'backend' NOT activated — nothing was written"
    (the same `total_failure` reading that already skips the lockfile pin), the
    per-target refusals still print, and
    `crates/cli/tests/a_dry_run_predicts_its_write.rs` holds the witness and its
    control.
13. **[ ] P8-G6 — `workflow explain --json` omits the documented
    `role_details[]`.** `list` has it; `explain` emits a top-level `roles[]`.
    Deliberately unfixed: choosing between documenting two key paths and
    changing `explain` is a contract decision.
14. **[ ] P8-G7 — a manifest schema error names neither the file nor the fix.**
    A missing top-level `version` fails with the serde message and exit 1, with
    no path and no valid-header example.
15. **[ ] P8-G8 — confirm an observed asymmetry.** On an untrusted project
    `x workflow explain` refuses while `x workflow list` prints names, roles and
    ceilings marked `TRUSTED false`. It looks deliberate; someone should say so
    on the record.

## Documentation

16. **[ ] Reference prose for `workflow`, `image`, and `shim`.**
    `docs/reference.md` lists them in the command inventory but gives them no
    prose section of their own.
17. **[ ] Decide the `docs/archive/` citations.** Eight archived files stay
    tracked only because current pages cite them for material those pages do not
    restate — a threat model's residual risks, an accepted ADR's rationale,
    operational field notes. Either fold what is load-bearing into the operative
    docs or accept the citations permanently. `docs/archive/README.md` records
    the rule in the meantime.
18. **[ ] Re-run what P8 left unverified.** Each is believable, not verified:
    the `--unprotected` HOST/ADVISORY banner (never captured — both
    non-interactive doors refuse it), real-model workflow semantics and the
    performance bookends, macOS kernel containment (Docker there is a Linux VM),
    and `agentstack more image --write` beyond its plan screen.

## Disk cost on many small files

macOS/APFS charges per file, and every path below walks or copies a tree.
None of these changes what is enforced, and the constraint in 20 is that none
of them may.

19. **[x] Clone trees instead of copying them file by file.** Landed, but
    uncommitted: the change sits in the working tree beside this note. The fast
    path is `crates/cli/src/fsclone.rs`, a wrapper whose `copy_dir_all` drops in
    for `fsx::copy_dir_all` — a metadata-only eligibility scan, then one
    `clonefile(2)` into a temporary sibling, then an atomic rename onto the
    destination; anything it cannot clone falls back to the core loop, so the
    fast path can only be skipped, never be the reason a copy fails. The scan
    exists because `clonefile` reproduces the source exactly while the loop does
    not (it drops `.git` and reads through symlinks), so a tree holding either
    is declined whole rather than cloned and repaired. Converted callers: the
    store's `snapshot_content`, skill materialization under the copy strategy
    (`render/skills.rs`), the skill and asset copies in `commands/add.rs` and
    `commands/try_skill.rs`, and the two upgrade backup sites. `lib add`'s
    `copy_extension_source` and the trash-move fallback keep the loop because
    they call `copy_dir_all_following_symlinks`, and image staging has no
    `copy_dir_all` call to convert. The unsafe is
    confined to `cli::sys::clone_tree` under the dated `STRATEGY.md` exception
    (approved 2026-08-13) and `core` keeps `forbid(unsafe_code)`. Measured about
    20x on a 2000-file tree: about 500 ms down to about 25 ms.
20. **[ ] Merge the redundant tree walks.** One resolve can traverse the same
    tree four or five times: `reject_symlinks` (`crates/cli/src/scan.rs:212`),
    `dir_digest` (`crates/core/src/digest.rs:113`), the copy walk, a re-digest,
    and `dir_size` (`crates/cli/src/store.rs:1277`). Fold reject-symlinks,
    digest and size into one traversal. Constraint: the merged walk still hashes
    the current bytes on every call. `docs/ARCHITECTURE.md` forbids a
    stat-fingerprint digest cache on any verification path — the old one was
    removed deliberately, because a same-stat content change could serve a stale
    digest and become a trust bypass. Do not reintroduce one.
21. **[ ] A disk-I/O benchmark, so 19 and 20 are measurable.** The only benches
    (`crates/workflow/benches`) time no I/O, so a store snapshot or a tree copy
    has no baseline and a regression is invisible. Add one repeatable timed run
    over either. Criterion would be a new dependency and needs maintainer
    approval; a plain timed harness may be enough. Shape to borrow:
    `github.com/NullVoxPopuli/disk-perf-git-and-pnpm`, which times `git clean`
    and `pnpm install` to expose the same APFS many-small-file cost.
22. **[ ] A `doctor` check for Spotlight indexing.** Indexing measurably slows
    many-small-file I/O on macOS, and `~/.agentstack` (store, runs) is indexed
    by default. Report whether it is — `mdutil`, or a `.metadata_never_index`
    marker — and name the exclusion. Report only: excluding a directory stays
    the user's to do.

## The ceremony's second verb

The lock now records the manifest it was pinned from, so `status` and `doctor`
stop calling a stale pin healthy and `trust` says "lock first" before a grant
that the next `lock --write` would void. Both are signposts around a hazard the
product could remove instead: the ceremony has two verbs and the order matters,
which is a rule the user has to learn and a rule every surface has to teach.

23. **[ ] Fold the pin step into trust — one verb, one yes.** `trust` re-pins
    before it reviews, so the lock and the grant are one act and the order can
    no longer be gotten wrong. The constraints are what make it a decision and
    not a refactor:
    - **Re-pin at the currently-resolved revision only.** `trust` never fetches
      a newer upstream — a git skill stays at the rev already resolved, and
      moving to a later one stays `lock --update`. A review that silently
      advanced the world would be the opposite of consent.
    - **The pin diff goes on the review card**, covering every kind the lock
      carries: skills, servers, instructions *and their per-(CLI, model)
      variants*, settings keys, executables, extensions, workflows and their
      blueprints, package member sets. A kind missing from the card is a change
      the user consented to without seeing.
    - **A fetch or digest failure refuses the grant.** No partial pin, no "yes"
      recorded over a surface that could not be read.
    - **The ~40 lock-remediation strings collapse to "review and re-trust".**
      They already run through two shared formatters — `crates/cli/src/verify.rs`
      (`bail_blocked`/`bail_locked`, which end every refusal with "then run
      `agentstack lock --write`") and
      `crates/cli/src/commands/trust.rs` (`UNPINNED_FIX`/`DRIFT_FIX`) — so the
      collapse is one edit per formatter plus the rungs that read them
      (`overview::correct_trust_rung` and the stale-pin rung both retire).
    - **Signed-lockfile flows sign AFTER the relock**, or the signature covers
      bytes the grant replaced.
    - **The docs drop the verb from the beginner path** — `docs/start.md`,
      `add-a-server`, `add-a-skill`, `concepts`, `ci` — keeping `lock --write`
      documented where it is still its own act (`--update`, `--upgrade`, CI
      verification).

## Not scheduled, deliberately

24. R2 — the positioning flip. `STRATEGY.md` reopens only at its named
    tripwires.
25. R1 — the private APM disclosure. **Maintainer-only: embargoed to a third
    party's private channel, so no agent drafts, sends, or contacts anyone about
    it.**
26. The retired Mode axis still has leftovers in `doctor --json`, `overview.rs`
    and `ui_contract.rs`.
27. `docs/design/automatic-delivery.md` — the automatic-delivery design lane.
28. `x diff --profile`.
