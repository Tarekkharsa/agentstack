# AgentStack work queue

> **Purpose:** the only ordered product-wide work queue.
>
> **Status:** rebuilt 2026-08-12 against agentstack 0.19.0. Closed work and
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

Each reverses or narrows an adopted design. None is a cleanup; each needs a
maintainer's decision.

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
   `beforeShellExecution` and `beforeReadFile` are wired. Waits on a hook
   someone else has to ship.
9. **[~] G19 — Kiro gets no host guard.** Its descriptor records MCP config
    only, so there is no hook path to install into. Marked `NOT_WIRED` (a fact
    about agentstack) rather than implying none could exist. Same shape as G6.

## Honest-surface gaps from the P8 evidence pass

Each is command output that promises what a later command refuses, or a
contract the docs describe and the code does not emit.

10. **[x] P8-G2 — `use`'s dry run promises what the write refuses.** Closed by
    #50: on an untrusted project the preview now closes "1 target would be
    BLOCKED, so --write would refuse and write nothing" and never names
    `--write`; pinned by
    `crates/cli/tests/a_dry_run_predicts_its_write.rs::use_dry_run_does_not_promise_a_write_its_own_gate_would_refuse`.
11. **[x] P8-G3 — `apply`'s dry run has the same shape.** Closed: already fixed
    by #50 (`delivers_nothing` arm in `crates/cli/src/commands/apply.rs`,
    regression test `apply_dry_run_does_not_promise_a_write_that_would_deliver_nothing`);
    re-verified 2026-08-13. The preview now names the refusal and both
    recovery commands instead of `--write`.
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
    the `--unprotected` HOST/ADVISORY banner (never captured, because both
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
    for `fsx::copy_dir_all`. It runs a metadata-only eligibility scan, then one
    `clonefile(2)` into a temporary sibling, then an atomic rename onto the
    destination; anything it cannot clone falls back to the core loop, so the
    fast path can only be skipped, never be the reason a copy fails. The scan
    exists because `clonefile` reproduces the source exactly while the loop does
    not (it drops `.git` and reads through symlinks), so a tree holding either
    is declined whole rather than cloned and repaired. The unsafe is confined to
    `cli::sys::clone_tree` under the dated `STRATEGY.md` exception (approved
    2026-08-13) and `core` keeps `forbid(unsafe_code)`.

    Converted callers: the store's `snapshot_content`, skill materialization
    under the copy strategy (`render/skills.rs`), the skill and asset copies in
    `commands/add.rs` and `commands/try_skill.rs`, and the two upgrade backup
    sites.

    Left on the old loop: `lib add`'s `copy_extension_source` and the trash-move
    fallback, because they call `copy_dir_all_following_symlinks`. Image staging
    has no `copy_dir_all` call to convert.

    Measured about 20x on a 2000-file tree: about 500 ms down to about 25 ms.
20. **[ ] Merge the redundant tree walks.** One resolve can traverse the same
    tree four or five times: `reject_symlinks` (`crates/cli/src/scan.rs:212`),
    `dir_digest` (`crates/core/src/digest.rs:113`), the copy walk, a re-digest,
    and `dir_size` (`crates/cli/src/store.rs:1277`). Fold reject-symlinks,
    digest and size into one traversal. Constraint: the merged walk still hashes
    the current bytes on every call. `docs/ARCHITECTURE.md` forbids a
    stat-fingerprint digest cache on any verification path. The old one was
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
that the next `lock --write` would void. Both are workarounds. The real
problem is one the product could remove instead: the ceremony needs two
commands in a fixed order, so users must learn it and every screen must teach
it.

23. **[ ] Fold the pin step into trust — one verb, one yes.** `trust` re-pins
    before it reviews, so the lock and the grant are one act and the order can
    no longer be gotten wrong. The constraints are what make it a decision and
    not a refactor:
    - **Re-pin at the currently-resolved revision only.** `trust` never fetches
      a newer upstream. A git skill stays at the rev already resolved, and
      moving to a later one stays `lock --update`. A review that silently moved
      the pin forward would grant consent to bytes nobody saw.
    - **The pin diff goes on the review card**, covering every kind the lock
      carries: skills, servers, instructions *and their per-(CLI, model)
      variants*, settings keys, executables, extensions, workflows and their
      blueprints, package member sets. A kind missing from the card is a change
      the user consented to without seeing.
    - **A fetch or digest failure refuses the grant.** No partial pin, no "yes"
      recorded over content that could not be read.
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

## After the study

Everything here is gated on the v0.19.0 study concluding. The freeze is not a
pause on thinking — these are the things the walk and the dogfood turned up
while the build was closed — but nothing here touches `main` until the five
sessions are done and their three blockers are known.

29. **[ ] Slash visibility for live skills.** The live lane serves skills to the
    MODEL — gateway index plus on-demand load — and writes no native file, so
    the CLI's slash menu never lists them and the human loses the `/name` entry
    a native skill gets. Design candidate: a thin native stub per live skill
    whose only job is slash-menu presence and triggering the gateway load —
    human discoverability with zero-drift delivery. Found by the maintainer in
    his own dogfood project.

30. **[ ] The guard reads quoted prose as an invocation.** `check_bash` judges
    tokens, so a shell line that merely CONTAINS `agentstack trust` is refused
    whether it is a command or a quotation: writing the documentation for the
    consent refusal tripped the refusal, twice, in this repository. Same family
    as the deny-glob false positive on a commit message, and the same shape of
    fix — a narrower reading that can tell an argument from a program. A study
    participant documenting their own setup can hit this.

31. **[ ] Port the two tutorial artifacts into the docs; the layers table
    lives on the second one.** Two artifact pages are approved as accepted
    drafts of future docs pages (2026-08-13). They are drafts of *pages*, not
    of prose to paste: the port re-authors them as Markdown in the docs
    pipeline. Sources: `/tmp/agentstack-pages/{seven-steps,dormant-layers}.html`.

    - **"AgentStack in seven steps"** — a continuous five-minute beginner
      journey: install → `init --connect` → trust → restart and `status` → add
      → second machine → undo. Unslop voice, every command sandbox-verified
      against v0.19.0. It becomes the docs tutorial page and **supersedes the
      current step-map**, which points into `start.md` rather than teaching.

    - **"The dormant layers"** — the advanced tour, organised by what is
      off until asked: schema autocomplete (with a live taplo transcript
      catching a missing `type` through the `#:schema` line), policy grammar
      via `examples/policies/developer.toml`, the three run tiers, the lockdown
      proxy, workflows' child-run gating, images, leases. This is the layers
      page, and **the doctor-line and layers-table requirements of the former
      item 31 fold into it**: a capability that is declared, pinned and trusted
      but not selected by the active toolset is invisible in the surfaces that
      matter, and a reader cannot tell "not installed" from "installed and not
      selected". **Deliberately NOT a trust-off flag** — considered and
      rejected: make the state legible, never make the gate optional.

    **Non-negotiable on the way in:** their sample commands join
    `docs_commands` and the site checks, so CI executes them forever. An
    artifact snapshot can rot in silence; a docs page in this repository
    cannot, and that difference is the whole reason to port rather than link.
    First-person test claims convert to the docs' dated verified-output
    convention — "I ran this and saw X" becomes output captured and dated, or
    it does not ship.

32. **[ ] The manifest should teach, not scaffold.** A generated manifest
    carries the keys the import produced and nothing about the ones a reader
    would want next. Before documenting any of it, verify the claim it rests
    on: that an ABSENT `[targets]` really does mean every detected CLI, in the
    code rather than in the docs.

33. **[ ] The external-adapter PR milestone.** The adapter descriptors are the
    most contribution-shaped surface in the product, and no outside PR has ever
    landed one. First external adapter descriptor merged is the milestone worth
    naming, because it is the first evidence the seam works for someone who
    did not build it.

34. **[ ] "Fail visible" needs a noun.** The property — a refusal that is
    printed, exit-coded and recorded rather than swallowed — is the one this
    product is built on and the one it has no word for. Naming it is a
    documentation act with teeth: an unnamed property cannot be tested for
    consistently, and every surface currently invents its own phrasing.

35. **[ ] Say what would falsify the strategy.** `STRATEGY.md` names revisit
    triggers, which is close, but a trigger is an event and not a prediction.
    A strategy that cannot be wrong cannot be checked, and the study is the
    first chance to write down what a bad result would look like BEFORE the
    result arrives.

36. **[ ] A typo'd root key is silently ignored — by the parser AND by the
    schema.** Reproduced 2026-08-13 against the released binary: a manifest
    whose only defect is `default_tolset = "dev"` (for `default_toolset`)
    passes taplo validation, parses, and `status` runs clean — reporting
    `Toolset dev — default`, which is the value it picked up from
    `[toolsets.dev]`, not from the misspelled key. The key is dropped without a
    word. The root `Manifest` and `toolsets` are serde-permissive while the
    eleven nested tables are strict, which is almost certainly deliberate:
    forward compatibility, so an older binary can read a manifest written by a
    newer one.

    So the fix is probably **not** `deny_unknown_fields` at the root — that
    would trade a silent typo for a hard refusal of every future key. The
    candidate is a `doctor` / `status` note instead: *unknown key
    'default_tolset' — did you mean 'default_toolset'?*, an edit-distance
    suggestion against the known key set, which costs nothing when the key is
    genuinely from a newer version and catches the typo when it is not.

    Also worth considering: `additionalProperties: false` at the SCHEMA root
    only, so an editor underlines the typo while the binary stays tolerant.
    **That divergence is the actual design question, not a detail** — a schema
    stricter than the parser means the editor and the binary disagree about
    what a valid manifest is, and this project's own rule is that claims match
    enforcement. Decide whether "the editor warns, the binary accepts" is an
    honest split or a second truth; the answer decides the shape of the fix.

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
