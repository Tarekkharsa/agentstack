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

10. **[ ] P8-G2 — `use`'s dry run promises what the write refuses.** On an
    untrusted project it exits 0 and closes "Re-run with --write to apply.";
    `use --write` then exits 1.
11. **[ ] P8-G3 — `apply`'s dry run has the same shape.** "0 targets would
    change. Re-run with --write to write." above an `apply --write` that exits 1
    with "nothing was delivered".
12. **[ ] P8-G4 — `use --write` reports activation for a run that failed.**
    "activated 'backend' on 4 targets (wrote 0)" prints above `error: 3 targets
    blocked` and exit 1.
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
    and `agentstack x image --write` beyond its plan screen.

## Disk cost on many small files

macOS/APFS charges per file, and every path below walks or copies a tree.
None of these changes what is enforced, and the constraint in 20 is that none
of them may.

19. **[ ] Clone trees instead of copying them file by file.**
    `crates/core/src/util/fsx.rs` copies one `fs::copy` per entry and has no
    `clonefile(2)`/reflink path; on APFS that costs 5-10x a clone. Add the fast
    path and keep the current loop as the fallback. Callers that pay today: the
    store's `snapshot_content`, `lib add`'s `copy_extension_source`, upgrade
    backups, the trash-move fallback, and image staging. `clonefile` requires
    the destination not to exist and clones a symlink as a symlink, so it fits
    `copy_dir_all` only; `copy_dir_all_following_symlinks` keeps the loop. A
    prototype is in progress.
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

## Not scheduled, deliberately

23. R2 — the positioning flip. `STRATEGY.md` reopens only at its named
    tripwires.
24. R1 — the private APM disclosure. **Maintainer-only: embargoed to a third
    party's private channel, so no agent drafts, sends, or contacts anyone about
    it.**
25. The retired Mode axis still has leftovers in `doctor --json`, `overview.rs`
    and `ui_contract.rs`.
26. `docs/design/automatic-delivery.md` — the automatic-delivery design lane.
27. `x diff --profile`.
