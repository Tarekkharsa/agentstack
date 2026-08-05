//! The versioned envelope every UI-facing JSON read carries (UI control-plane
//! §"Versioned contracts"). External panels (t3code) decode `schema_version`
//! first: an unknown major means "disable and show the upgrade path", never
//! "guess". `features` names usable end-to-end contracts — a feature appears
//! only when its full read/action loop works in this binary, so a UI can gate
//! each affordance on the named contract instead of sniffing individual
//! fields.
//!
//! This is presentation-layer negotiation only. No enforcement decision may
//! read these fields: the CLI re-validates every precondition on every call
//! whether or not the caller negotiated.

/// Bumped only when an existing field changes meaning or shape. Adding fields
/// or features is backward-compatible and does NOT bump this.
pub const SCHEMA_VERSION: u64 = 1;

/// End-to-end contracts this binary serves. Names are stable identifiers for
/// external UIs; remove one only with a schema-version bump.
///
/// - `init-plan`: `init --plan` emits the detection plan with `plan_digest`.
/// - `apply-setup`: `init --yes --consented-plan <digest>` applies a reviewed
///   plan and refuses when the detected inputs drifted since the plan.
/// - `init-tool-managed-v1`: the plan carries `tool_managed[]` — servers whose
///   executable lives inside another application's bundle, which `init` leaves
///   out of the import by default. Each entry names the server, the owning
///   `application` as detected, the `path` that evidences it, the `reason`,
///   and whether it was `imported` (true only under
///   `--include-tool-managed`). Without this a panel sees such a server as
///   simply absent, which is the wrong claim: "left alone" and "not found"
///   differ, and the entry still exists in every CLI's own config.
///
///   Two properties a panel may rely on. The list is DEDUPLICATED by name:
///   the desktop applications register one server into every tool config on
///   the machine, so six sightings are one row. And it does NOT join
///   `plan_digest` — like `unsupported`, it is informational; an excluded
///   server is absent from `servers`, which the digest already binds, so
///   including it would add nothing and would re-gate a reviewed plan when a
///   vendor moved a path that changes nothing this import writes.
///
///   The classification is a heuristic over path text, not a claim of
///   provenance — nothing is executed, resolved or signature-checked. A panel
///   must present it as "looks owned by X" with the `path` beside it, never as
///   an established fact, and must keep the override reachable.
/// - `trust-preview`: `trust --preview` emits the reviewed surface with
///   `surface_digest`. This said "the FULL reviewed surface", which was not
///   true — see `trust-review-card-v1` for what it covers and what it still
///   deliberately does not.
/// - `trust-consent`: `trust --yes --consented-digest <digest>` grants bound
///   to the previewed bytes and refuses stale or missing digests.
/// - `trust-server-blockers-v1`: `trust --preview` carries server-resolution
///   and local-executable blockers with the safe next step, so an external UI
///   can disable a grant already known to fail.
/// - `trust-review-card-v1`: `trust --preview` additionally carries `hooks`,
///   `settings`, `policy_requested`, and `machine_policy_ceiling`, plus `hooks`
///   and `settings` counts — the kinds the terminal review card discloses that
///   this JSON previously omitted. Hooks are the reason this exists: they are
///   an executable kind, and a panel built on the old payload showed a
///   project's executable surface as smaller than it is.
///
///   **What it still does not carry, deliberately.** Two divergences from the
///   terminal card are load-bearing rather than gaps, and are documented here
///   so nobody "fixes" them by accident:
///
///   1. **A drifted library server stays redacted.** The preview replaces a
///      library server whose live definition does not match its lock pin with
///      an `unverified` marker instead of its command line, so an external UI
///      cannot bind consent to bytes the digest does not cover. The terminal
///      review prints the live command line with a `DRIFTED` annotation
///      instead, because the authoritative card may never disclose *less*.
///      These are opposite answers to the same question, both intentional.
///   2. **Blockers cover servers and local executables only.** The card's
///      full per-kind blocker set requires resolving skills, workflows, and
///      extensions, which reaches git worktree materialization — writes and
///      subprocesses that must not happen on a read-only, panel-facing
///      command. `state` and `surface_digest` remain the honest signal that a
///      grant may still refuse.
///
///   Consequently a UI must not present this payload as "everything the
///   reviewer will see". It is the machine-readable surface; `agentstack
///   trust` remains authoritative.
/// - `trust-card-diff-v1`: `trust --preview` additionally carries `review` —
///   the consent card itself, structured. Until now the card was print-only
///   inside the grant path, so its per-item facts reached the terminal and
///   nothing else. `review.items[]` gives each reviewed item its `kind`,
///   `name`, what it `runs`, what it `contacts`, what it `may_read`, its `pin`
///   and the `prior_pin` the last consent recorded, how many other projects on
///   this machine already approved that content, and a `change` marker
///   (`added` / `changed` / `unchanged`) computed from the SAME identity
///   strings the grant walk persists — plus `review.removed`, the items the
///   last consented surface carried and this one does not. On a re-review an
///   item whose bytes moved carries a capped changed-lines `diff`.
///
///   Four divergences, each deliberate:
///
///   1. **A drifted library server stays redacted**, inherited from
///      `trust-review-card-v1`: its item names the same "does not match the
///      lockfile pin" text instead of the live command line, and contributes
///      nothing to `runs` / `contacts`. It still reads `changed`, because
///      saying that something moved discloses nothing while emitting the bytes
///      the digest does not cover would.
///   2. **The diff is prior-consented-pin → currently-locked-pin, not
///      pin-to-live.** The consent digest covers the LOCK bytes, so that delta
///      is the one this payload can bind to; resolving live bytes would reach
///      git worktree materialization, which a read-only command must not do.
///      The terminal review stays authoritative over live bytes and diffs
///      pin-to-live there. A consequence worth stating: `change` keys on
///      identity only, so a skill whose BYTES moved still reads `unchanged`
///      while its `diff.status` reads `changed` — the pin is deliberately not
///      part of the diff key (see `SurfaceItem::pin`), and the diff object is
///      where the byte story lives.
///   3. **`recognized_other_projects` is machine-local display information**,
///      never an input to any decision. `null` means this machine has no
///      readable recognition index — which is not the same as zero.
///   4. **No per-item accept / keep-pinned / block affordance.** Those three
///      answers exist only in the interactive terminal review, where the
///      single closing yes commits them. A panel may render what changed; it
///      may not collect the answer.
///
///   A separate name rather than a revision of `trust-review-card-v1` because
///   the field sets are not versions of one another: that one carries the
///   KINDS the preview omitted, this one carries the CARD. Recorded as the
///   naming correction in `docs/design/consent-card.md` §Panel.
/// - `trust-card-groups-v1`: `trust --preview` additionally carries
///   `review.groups` — the card's detail body grouped per capability — and
///   `review.question`, the one closing question.
///
///   A group is `{kind, label, change, counts{added,changed,unchanged,removed,
///   total}, items[], removed[]}`. `items` and `removed` hold **indices into
///   `review.items` / `review.removed`**, not copies. That is the point: a
///   group has nowhere to put a fact of its own, so grouping is presentation
///   and can never become a second description of the same review. `change` is
///   the group's marker in the same three words the items use — `added` when
///   the whole group is new, `changed` when anything under it moved or was
///   dropped, `unchanged` otherwise.
///
///   Three properties a consumer may rely on:
///
///   1. **Additive.** `review.items` and `review.removed` keep their
///      `trust-card-diff-v1` shape AND order byte for byte. A panel that
///      predates this feature reads exactly what it read before, and a panel
///      that uses it renders the same items in a different arrangement.
///   2. **Exactly one question, and no answers.** `review.question` is the
///      single closing yes for the whole project. There is no per-group and no
///      per-item question, accept, keep-pinned, or block field, and there never
///      will be — grouping the body must not multiply the moments a human
///      commits to something. The answer is `trust --yes --consented-digest
///      <surface_digest>`, bound to the project's bytes, never to a group.
///   3. **Complete.** Every kind appearing in `items` or `removed` appears in
///      exactly one group; a kind this binary does not have a label for is
///      grouped under its own name rather than dropped.
///
///   Delivery routing is NOT a group and is not answerable: which lane a
///   capability reaches a harness through is informational
///   (`delivery-routing-v1`), decided by the planner, and never something the
///   consent moment asks about.
/// - `trust-content-drift-v1`: the machine surfaces stop reporting a healthy
///   project over content that has drifted from its pin, and carry the command
///   that fixes it.
///
///   The consent digest covers the manifest, the local overlay, and the
///   lockfile — not the BODIES those bytes pin. Editing an approved skill in
///   place therefore left `trust --preview` reporting `state: "trusted"` with
///   an empty blocker list and the edited item marked `change: "unchanged"`
///   (its identity — where the body comes from — genuinely had not moved),
///   and `status --json` reporting `trust: "trusted"`, while `doctor` errored
///   and `agentstack trust` refused. Two machine surfaces disagreed with the
///   gate, and a driver polling them reported the project healthy.
///
///   The drift reading itself is NOT new: it is the shared
///   `resolve::*_lock_status` seam the grant walk and `doctor` already read,
///   called once per surface. Nothing about how trust is granted, how digests
///   are computed, or what the gate decides changes — only what the reporting
///   projection says about a state the gate already refuses.
///
///   `trust --preview` gains `content_drift[]` (`{kind, name, reason, fix}`
///   per drifted item), `blockers[]` (the server blockers, kind `server`, plus
///   the content drift, each with its `fix`), `grantable`, `fix`, and
///   `next_step` `{command, why}`; each `review.items[]` entry gains `drifted`
///   and `fix`, and each `review.groups[].counts` gains `drifted` (a subset of
///   `changed`). `status --json` gains `project.content_drift[]`.
///
///   Three existing VALUES change, deliberately, because they were lying:
///   `trust --preview`'s `state` and `status --json`'s `project.trust` read
///   `drifted` over drifted content (`untrusted` still wins — never reviewed
///   is the stronger statement), and a drifted item's `change` reads
///   `drifted` where it would have read `unchanged`. `change` stays
///   identity-keyed otherwise: `added` and `changed` keep their word, because
///   they already say the item is not clean and they carry the identity answer
///   a source flip depends on. A consumer that switches on the three old words
///   must add a fourth arm; `review.groups[].counts.drifted` is tallied off the
///   per-item boolean, so an item that both flipped source and drifted is
///   counted there while still reading `changed`.
///
///   The same flag also covers NEVER-PINNED items, which are a separate claim
///   from drift — nothing about them was ever approved, so calling them drift
///   would misreport what the user is being asked about. `trust --preview`
///   gains `surface_unpinned[]` (`{kind, name, reason, fix}`, same shape as
///   `content_drift[]`) and `status --json` gains
///   `project.surface_unpinned[]`; `blockers[]` is the ONE list to read and
///   concatenates server blockers, then `content_drift[]`, then
///   `surface_unpinned[]`; and `grantable`, `fix` and `next_step` account for
///   never-pinned items exactly as they account for drift.
///
///   **`fix` is nullable** — on every `blockers[]` / `content_drift[]` /
///   `surface_unpinned[]` entry and on the top-level `fix`. `null` means no
///   single command repairs that condition, and today it has one cause: a
///   declared body that is not present on disk. `agentstack lock --write`
///   resolves before it pins, so it exits non-zero and changes nothing there,
///   and no other verb re-creates the body either. Naming a command anyway is
///   how a poll-and-run driver spins forever, so the field says nothing and
///   `reason` carries the condition. A decoder must treat `fix` as
///   `string | null`.
///
///   `surface_unpinned[]` mirrors the GRANT WALK'S BLOCKER SET, kind by kind
///   — not "everything unpinned". A LIBRARY-origin skill with no pin is a
///   yellow advisory at the gate and is therefore absent here; an INLINE one
///   blocks and is present. It is also NARROWER than the gate: the gate also
///   refuses over library servers, unpinned repo-relative local executables,
///   and unpinned or drifted blueprints, which this projection does not read.
///   So an empty `surface_unpinned[]` does not mean the grant will succeed.
///
///   `next_action` / `next_step` name `agentstack lock --write`, not
///   `agentstack trust .`: the grant REFUSES over drift and over the
///   never-pinned items reported here, so re-pinning is the step that makes
///   progress, and re-pinning flips the lockfile bytes and hence the trust
///   digest, which is what puts the review next. A driver can read one field,
///   run it verbatim, and converge. When no reported blocker carries a fix,
///   `fix` and `next_step` are `null` rather than a command that cannot work.
///
///   `grantable: false` means THIS PROJECTION can already see a reason the
///   grant refuses, and it is checked against the gate's own blocker
///   construction kind by kind. It is not an independent verdict: `agentstack
///   trust` stays authoritative in both directions, and `grantable: true` is
///   not a promise the grant succeeds. A surface that gates a human Approve
///   control on this field should treat it as advice, never as the refusal —
///   an earlier build reported `false` over a library-origin unpinned skill
///   the gate accepts, and blocked the one answer only a human may give.
/// - `activity-skill-load-v1`: a successful on-demand skill load over MCP
///   (`agentstack_load`) is recorded as first-class activity. Each one appends
///   to the machine-global `~/.agentstack/audit/loads.jsonl`
///   (`{ts, name, reason, project?, run?}`) and, when the load happens inside a
///   run, mirrors a `{"event":"skill_load", …}` line into that run's
///   `events.jsonl`. `report calls --json --include-loads` interleaves the two
///   streams into one activity feed ordered by timestamp, with a `kind`
///   discriminant (`"call"` / `"skill_load"`) on every row.
///
///   The flag is the contract. WITHOUT it the feed is byte-identical to
///   before, loads on disk or not, so an older consumer's strict decoder never
///   meets a row shape it predates; asking for the flag is how a caller says
///   it understands the new shape. A load is NOT a call and never enters the
///   ok/error/denied tallies — its own stream, its own event variant, its own
///   counts.
///
///   Recording is evidence, not enforcement: a refused load never reaches the
///   recorder at all (the MCP call itself fails first), so absence from this
///   stream means "did not happen", not "was allowed". Nothing reads it to
///   make a decision.
/// - `status-v1`: `doctor --json` carries `state` + `next_action`.
///
///   `next_action` is NULLABLE, and a consumer must handle null. It is the
///   MACHINE field: either a command that can be executed verbatim and will
///   make progress, or `null` when there is nothing to run — a healthy setup,
///   or a state whose only honest answer is a shape (`toolset create <name>
///   …`) or a prose remedy. The human sentence the terminal prints always
///   lives beside it in `next_step`, which is text for a UI to render and
///   never something to exec. `status --json`'s `next_action` object splits
///   the same pair: `command` (runnable or null) and `sentence` (prose).
/// - `status-honesty-v1`: `doctor --json` carries `readiness`, and
///   `snapshot --json` carries a singular `nextAction`. Both are ADDITIVE:
///   `state` and `nextActions` keep their `status-v1` meanings byte for byte,
///   because a panel already rendering "Ready" from `state` must not have that
///   word change meaning under its users.
///
///   `readiness` is the field to render instead. `state` answers only "did any
///   check find something to repair?", so it says `ready` over a project that
///   is untrusted and has never been activated — zero findings is true, and
///   "ready" is not, since nothing the project declares is live. `readiness`
///   answers "is this project actually live?" over the same report and takes
///   one of: `needs_attention` (findings to repair), `untrusted` /
///   `drifted` (the consent gate is what stands between here and live),
///   `never_activated` (consented or not, no lockfile — nothing was ever
///   rendered), `ready` (findings-free, trusted, activated), or `unknown`
///   (doctor ran with no project, so there is no project readiness to claim).
///   `needs_setup` appears in the pre-manifest payload, matching `state`.
///
///   A consumer migrating off `state`: render `readiness`, and treat every
///   value except `ready` as "not live", with `next_action` as the step. The
///   t3code fork's "Ready" chip is the known caller — it reads `state` today
///   and is exactly the mislabel this exists to fix.
/// - `profiles-v1`: `use --list --json` lists profiles with readiness.
/// - `diff-v1`: `diff --json` reports drift per target.
/// - `restore-last`: `restore --json` lists undoable writes; `restore --last
///   [--write]` previews/undoes the newest.
/// - `sessions-v1`: `use --list --json` carries per-profile `active` and the
///   top-level `session` object; `session start <profile>` activates
///   fail-closed (refuses untrusted or unpinned surfaces) and `session end`
///   reverts — including a session an interrupted UI left behind.
/// - `profiles-edit-v1`: `library-index` emits the central-library catalog
///   (skills + servers) for the browser; `add-skill-to-profile`,
///   `add-server-to-profile`, `create-profile`, and `use-profile` mutate the
///   toolset then re-lock + re-render, each bound to a `consent_digest` a prior
///   `--preview` returned (apply refuses on drift) and failing closed on an
///   unresolved `${REF}`. NOTE: `create-profile` no longer re-renders — see
///   `toolset-create-v2`, which is the name to gate on for that. This paragraph
///   still describes what a binary advertising ONLY `profiles-edit-v1` does,
///   which is what the name has to keep meaning.
/// - `diff-ownership-v1`: each `diff --json` target carries `managed` (the
///   names this render owns), `hand_edited` (the file no longer matches what we
///   last wrote), and `foreign_untracked` — foreign entries nobody ever declared
///   to agentstack, which `apply` preserves exactly like `kept` but which
///   `adopt` and `apply --prune-foreign` cannot act on. `kept` keeps its
///   `diff-v1` meaning (another MANIFEST's entries, adopt-eligible) precisely so
///   a panel on the older name never offers Adopt for something the CLI cannot
///   honor — the same reasoning as `library-remove-v1`. A UI that does not know
///   this name simply shows one fewer category, which is safe.
/// - `toolset-create-v2`: `create-profile` writes the manifest entry and
///   re-locks, and renders NOTHING — naming a toolset is no longer switching to
///   it (review finding H3). Activation is a separate verb (`use-profile`, or
///   `session start` for a reversible one). A separate name rather than a
///   revision of `profiles-edit-v1`, because a binary advertising that older
///   name legitimately re-renders on create: a panel that showed "created and
///   active" on the strength of `profiles-edit-v1` would be right about the old
///   binary and wrong about this one. `--allow-unresolved` is inert for this
///   verb now (nothing renders, so no `${REF}` resolves), exactly as it already
///   is for `remove-from-library`. The other three `profiles-edit-v1` verbs
///   (`add-skill-to-profile`, `add-server-to-profile`, `use-profile`) still
///   re-lock AND re-render, and are unchanged.
/// - `profiles-edit-batch-v1`: `edit-profile --profile <p>` takes repeatable
///   `--add-skill` / `--add-server` / `--remove-skill` / `--remove-server` and
///   applies them as ONE manifest write under ONE `consent_digest`, followed by
///   a single re-lock and re-render. Two things it gives a panel that no other
///   verb does: a way to take a capability OUT of a toolset (the `add-*` verbs
///   have no inverse — `remove-from-library` is machine-wide and deletes the
///   capability itself, which is a different act), and a membership edit whose
///   cost does not scale with the number of things changed. The preview carries
///   the resulting `skills`/`servers` as well as the deltas, so a UI showing the
///   end state consents to the same picture it drew, plus `empties_toolset` for
///   the case where the batch would leave nothing behind. A separate name from
///   `profiles-edit-v1` because a binary advertising that one legitimately has
///   no removal path at all.
/// - `library-remove-v1`: `remove-from-library --kind skill|server --name <n>`
///   drops a capability from the MACHINE-WIDE central library, bound to a
///   `consent_digest` a prior `--preview` returned — here over the library index
///   bytes, since no manifest is involved. It is the only panel mutation that
///   edits machine state instead of the project (nothing re-locks, nothing
///   re-renders) and it is recoverable: the body and index row move to
///   `lib/.trash`, restorable with `lib trash --restore <id> --write`. A
///   separate name from `profiles-edit-v1` because a binary advertising that
///   contract legitimately predates removal — a panel offering a Remove button
///   on the older name would offer a button the CLI cannot honor.
/// - `manifest-remove-v1`: `remove-capability --kind skill|server --name <n>`
///   removes one project-owned definition and every toolset membership that
///   names it under a consent digest, then re-locks and re-renders. It never
///   touches the machine-wide library and refuses when multiple toolsets make
///   the render selection ambiguous.
/// - `workflow-observe-v1`: `workflow list --json` surfaces every declared
///   `[workflows.*]` entry with its per-entry trust + lock state (project-scoped
///   reads), and `workflow runs --json` lists recorded run history. Unlike the
///   other reads, `runs` reads the machine-global runs directory
///   (`agentstack_home()/runs`), not the project — run evidence is not
///   project-scoped. Both are read-only observation; running/resuming re-gates
///   independently.
/// - `workflow-serial-roles-v1`: each `workflow list --json` row carries
///   `serial_roles` — the subset of that workflow's roles whose harness takes
///   no per-child MCP config, so its children launch ONE AT A TIME whatever
///   the concurrency cap says. A separate name rather than a wider reading of
///   `workflow-observe-v1`, because a binary predating this field legitimately
///   advertises that contract without it: folding the field into the older
///   name would make it over-promise, and a UI reading `serial_roles` on the
///   strength of `workflow-observe-v1` would be sniffing a field — the exact
///   thing these names exist to replace.
/// - `doctor-advisories-v1`: `doctor --json` carries a top-level `advisories`
///   count, and section lines can carry `level: "advisory"` — findings that are
///   true and worth stating but are NOT something this project must repair, so
///   they are excluded from `warnings`, from `state`, and from `next_action`.
///   A UI that does not know the name renders advisories as `ok` (its level
///   match falls through), which is safe but silent: the CLI says "1 note" and
///   the panel says nothing. Gating on this name is what lets a panel show the
///   count instead of dropping it (review finding N4).
/// - `doctor-probe-v1`: `doctor --probe` actually STARTS each stdio server,
///   speaks the MCP `initialize` handshake, and stops it again; `doctor --json`
///   then carries a top-level `probe` object — `ran`, `skipped_reason`, and one
///   `servers` entry per stdio server with `status` (`ok` / `failed` /
///   `not_probeable`). A separate name rather than a wider reading of
///   `status-v1`, for two reasons. The first is the usual one: a binary
///   predating this emits `probe: null` and a UI reading the field would be
///   sniffing it. The second matters more — this is the ONLY doctor contract
///   with side effects, so a panel must not offer a "check my servers actually
///   run" button unless the CLI behind it really can spawn, bound, and reap
///   those processes. `ran: false` with a `skipped_reason` is a first-class
///   answer, not an error: the CLI refuses to spawn anything for a project that
///   is not trusted at its current bytes, and a UI that treats that as a
///   failure would push the user to retry instead of to the trust review.
/// - `doctor-mode-v1`: `doctor --json` carries `mode` (`static` /
///   `clean-at-rest` / `zero-files`) and `activation` (`locked` /
///   `never_activated`) — the same derived readings `agentstack status`
///   prints, as typed fields. Before this a panel needing the delivery mode
///   had to substring-match section prose ("Mode zero-files", "not locked
///   (never activated)"), which rewording silently breaks. Both are `null`
///   when doctor ran with no project (`needs_setup`). A separate name rather
///   than a wider reading of `status-v1`, because a binary predating the
///   fields legitimately advertises that contract without them.
/// - `diff-existence-v1`: each `diff --json` target carries `existed_before` —
///   whether the config file was on disk when the diff was computed. With
///   `changed` it splits the two stories a pending render tells: absent file
///   ("never rendered here") vs a rendered file the manifest moved ahead of.
///   The UI's stopgap was parsing the unified-diff hunk header (`@@ -0,0`),
///   which an empty-but-present file already misclassifies. Its own name for
///   the usual reason: reading the field on an older binary would be sniffing.
/// - `doctor-cli-coverage-v1`: `doctor --json` carries `clis` — `detected`
///   (CLIs installed on this machine), `bridge_capable` (of those, how many can
///   host the stdio bridge, i.e. be served live), and `bridge_incapable` (the
///   display names of the rest). One definition shared with `gateway connect`'s
///   own eligibility, so the count a UI shows and the set the command reaches
///   cannot disagree. This is the honest denominator for any "served live"
///   claim: a zero-files footer must say "11 of 13 CLIs", and NAME the two,
///   because a coverage number that shrinks silently is worse than no number.
///   `null` when doctor ran with no project. Its own name for the usual
///   reason: a binary predating the field legitimately advertises
///   `doctor-mode-v1` without it.
/// - `set-mode-v1` — **SUPERSEDED, see [`SUPERSEDED`]. Not served.** The Mode
///   axis retired with STRATEGY.md v3; the record of what it did is kept
///   because a panel reading an older binary still meets it. It was:
///   `set-mode <static|clean-at-rest|zero-files>` switches the
///   project's delivery mode with the full house consent pipeline —
///   `--preview` returns the REAL transition plan (every file the un-render
///   removes, whether the bridge is registered and its machine-wide scope,
///   bridge coverage, the toolset a render would activate, the undo command,
///   and every blocker) plus a `consent_digest` binding the direction
///   (current→target) and the manifest bytes; apply requires `--yes
///   --consented <digest>`. Fail-closed edges are first-class: an active
///   session refuses every direction, zero-files refuses an untrusted project
///   (trust is never granted here — the review is), and clean-at-rest refuses
///   while the machine-wide bridge serves this trusted project (those facts
///   derive zero-files whatever is removed; the exit is `gateway disconnect`,
///   a machine-scope decision this project-scope verb must not make). The
///   un-render leg is shared with `uninstall`, records ONE history entry, and
///   clears the state ledger's managed sets so the derived mode stops reading
///   "static" over files that no longer exist. A UI must not offer a mode
///   picker on a binary without this name — before it, `mode_switch_plan` had
///   no un-render leg, so a rendered project that "switched" kept deriving
///   static.
/// - `json-reads-v1`: the four orientation reads that had no machine form now
///   accept `--json` and emit an enveloped body — `status`, `search`,
///   `adapters list`, and `session list`. Each is the SAME reading the human
///   screen renders, in named fields instead of scraped text: nothing writes,
///   renders, spawns, or is computed differently because the flag is present.
///   A separate name rather than a wider reading of `status-v1` (which names
///   `doctor --json`) or `sessions-v1` (which names `use --list --json` and the
///   start/end pair), because those contracts are about different payloads —
///   folding these in would make an older binary advertising them a liar about
///   four commands it cannot serve. And the failure mode here is harsher than a
///   missing field: a binary predating this does not emit `null`, it REFUSES
///   the call with a clap usage error, so a caller that guessed wrong gets a
///   broken integration rather than a degraded one. Gating on this name is what
///   lets an integrator choose the JSON path up front instead of discovering
///   the refusal at runtime — and lets it fall back to `--help`-driven
///   screen-scraping deliberately, on a binary that genuinely predates the
///   contract.
/// - `needs-your-yes-v1`: two additions, one fact. `status --json` gains an
///   optional `project.needs_your_yes` object — `refused`, `last_refused_ts`,
///   `fix` — present ONLY when the project is untrusted or drifted AND calls
///   were actually refused here since its last yes; and the two refusals that
///   happen before any dispatch (a lease the gateway will not open, a skill it
///   will not load) now leave the same evidence a refused dispatch does: one
///   `calls.jsonl` row tagged `trust` and one run-scoped `trust_refused`
///   event. Together they are what lets a panel show a *pending consent* — an
///   untrusted project something is actually waiting on — instead of a static
///   "untrusted" label indistinguishable from a project nobody has touched.
///
///   What it deliberately does NOT cover: **no card payload travels with it**.
///   The object carries a count, a timestamp, and the command — never the
///   reviewable surface. The card has exactly one walk
///   (`crates/cli/src/commands/trust.rs`, shipped as `trust-review-card-v1` /
///   `trust-card-diff-v1`), reachable through `trust --preview`, and a second
///   construction of it on a status read is precisely the disclosure drift the
///   single-walk rule exists to prevent. Nor does it cover any way to ANSWER
///   the consent: there is no MCP-invocable consent path, the refusal the agent
///   relays names a command a human runs, and gating on this name must never be
///   read as an affordance to grant trust from a UI without one.
///   Its own name for the usual reason: a binary predating it emits no such
///   key, and a UI reading the field on the strength of `status-v1` or
///   `json-reads-v1` would be sniffing.
/// - `update-offer-v1`: `status --json`'s `project` gains an optional
///   `updates` object — `packs[]` of `{name, current, available}` plus the one
///   `fix` command that takes them. `fix` uses the SHIPPED spelling,
///   `lock --upgrade <pack>` (or `--all` for several); a friendlier `upgrade`
///   verb is a working name only, and copy naming a verb the binary lacks is
///   worse than no copy. The key is INSERTED, never emitted as `null` or `[]`,
///   so presence alone answers "is there an offer".
///
///   What this name explicitly does **not** promise: **absence is not
///   currency.** The check behind it is offline by construction — `status`
///   must not hang or fail on a network call — so it reads only the tag pinned
///   in the `[packs.*]` ledger and the tags git has already fetched into this
///   machine's store clone. A pack never cloned here, a clone that predates
///   the newer tag, a machine without git, or a `catalog:` source (one version
///   per id, so there is no version axis) all contribute nothing, and are
///   indistinguishable from a pack that is genuinely current. Only
///   `agentstack lock --upgrade <pack>` asks the remote. A UI must render this
///   as an offer and must never derive an "up to date" badge from a missing
///   `updates` key.
/// - `package-members-v1`: `status --json`'s `project` gains an optional
///   `packages[]` — one row per package this project **pinned**, each carrying
///   `name`, `version`, `source`, `rev`, the `toolsets` that selected it, the
///   `removed` member names, an `overrides` count, and `members[]` with
///   `name` / `kind` / `lane` / `origin` / `checksum` / `provenance` per member.
///   The key is INSERTED, never emitted as `[]`, so a project that selects no
///   package reads exactly as it did before.
///
///   What the name promises: this is the **effective** member set — what this
///   project actually took, after its `[package_overrides.*]` were applied —
///   read from the LOCK. `origin` says `package` or `project-override` per
///   member and `removed` names what was dropped, so a panel can render "took
///   the package, replaced one member" without holding the package itself to
///   diff against. `lane` is derived from the member's kind (`rendered` for an
///   instruction, `dynamic` for a skill or server) precisely so no UI can
///   describe an instruction member as served through the gateway.
///
///   What it explicitly does **not** promise. **It is not a view of the
///   library.** These rows describe pinned bytes, so a package whose library
///   copy has moved ahead reports the version and digests this project is
///   pinned to — which is the reproducibility rule working, not staleness. A
///   UI must not derive "you are on the latest" from it; `update-offer-v1` is
///   the offer surface, with its own honest limits. **It is also not an
///   activation reading:** a pinned package is not a running one, and nothing
///   here says whether a lease is open or a server started.
/// - `lease-status-v1`: `lease status --json` emits the machine-level runtime
///   lease registry — one row per record with `instance`, `project`,
///   `toolset`, `pid`, `started_unix`, a derived `liveness`, and the `why`
///   sentence behind it. This is the authoritative read: a lease used to exist
///   only in the MCP subprocess's memory, so no other surface could see that
///   one was open.
///
///   **What `liveness` promises.** It is DERIVED at read time, never stored.
///   `live` means the recorded PID exists *and* that process's start time still
///   matches the one recorded when the lease opened. `stale` means the process
///   is gone, or its PID now belongs to a different process — a crashed MCP
///   process leaves its record behind, and PID reuse is exactly why the start
///   time is part of the comparison rather than the PID alone. A panel may
///   therefore poll this without caching: the file is a record, not a truth.
///
///   **What `unknown` means.** Some platform must supply the process start
///   time — Linux reads `/proc/<pid>/stat`, macOS asks `ps`. Where neither is
///   available the row reads `unknown`: the PID exists, but reuse cannot be
///   ruled out, so nothing is claimed. A UI must render `unknown` as "not
///   established" and must never fold it into `live`; that is the fail-closed
///   direction, and it is the only honest one.
///
///   **What it does not promise.** A lease is **process-scoped** — it
///   disappears with the process that owns it, and there is no way to keep one
///   alive or to re-attach to it. Nothing here is an authority: no enforcement
///   decision reads this registry, and there is no action on this contract at
///   all — leases are opened and closed by the MCP connection that owns them,
///   never from a panel. Its own name for the usual reason: a binary predating
///   it has no such command and refuses the call outright.
/// - `delivery-routing-v1`: `delivery --json` emits the delivery planner's
///   answer — `default` (always `"automatic"`), and one `harnesses` row per
///   targeted CLI with `id`, `display`, `mcp_capable`, `render_locally`,
///   `override` (`none` / `project` / `harness`), a plain-language `summary`,
///   and a `routes` array giving each capability `kind` its `lane`
///   (`dynamic` / `rendered`), the `why`, and `full_ceremony`.
///
///   **What it promises.** This is the routing, not a mode. There is exactly
///   one user setting behind it — **Render locally** — and it can only move a
///   capability towards files; nothing makes an instruction, a hook, or a
///   file-only CLI's capability go live, because no channel would carry it. A
///   panel may therefore render `lane` as a fact about where the bytes go, and
///   `override` as the scope a person actually set.
///
///   **What it does not promise.** It is **not an activation reading**: a
///   `dynamic` lane says where a capability is routed, not that a lease is
///   open, that the bridge is registered, or that the project is trusted —
///   `lease-status-v1`, `doctor-cli-coverage-v1` and the trust surfaces answer
///   those, each with its own limits. And `full_ceremony` is a statement about
///   hooks and extensions being executable kinds, never a claim that a
///   ceremony has happened. Its own name for the usual reason: a binary
///   predating it has no such command.
/// - `library-sources-v1`: `status --json`'s `project` gains a
///   `shadowed_names` array (`project.shadowed_names`, beside the other
///   per-project readings, so one project card is read from one place) —
///   one plain sentence per capability name that more than one **linked
///   library source** holds, naming the source that wins, the count, and the
///   `<source>:<name>` reference that pins the other copy
///   (`docs/design/linked-library-sources.md`).
///
///   **What it promises.** The array is always present and is `[]` when no
///   name is shadowed, so a panel can distinguish "checked, nothing shadowed"
///   from an older binary that has no such key at all. The sentences are the
///   SAME text `agentstack doctor` and `agentstack lib sources` print, so no
///   UI has to compose its own account of a collision, and the winner named
///   here is the one a bare reference actually resolves to.
///
///   **What it does not promise.** It is **not a serving reading.** Precedence
///   decides *selection*; a locked project serves the bytes its lock pins,
///   read from the content store, so a name shown as shadowed here may be
///   irrelevant to everything this project currently serves. It says nothing
///   about which sources are linked, in what order, or where they live — that
///   is `agentstack lib sources`, deliberately not a panel surface, because
///   the link list is personal-layer machine state and never project state.
/// - `instruction-channels-v1`: `status --json`'s `project` gains an
///   `instruction_channels` array (`project.instruction_channels`, the same
///   per-project object `shadowed_names` and `packages` live on)
///   — one row per targeted CLI with `id`, `display`, the `file` that
///   actually carries house rules there (`null` when the CLI has none), the
///   `live_channel` its adapter descriptor declares (`id`, `display`,
///   `confirmation` = `confirmed` / `unconfirmed`, and `used`), the `selection`
///   (`fragment`, `variant`, `model`, `model_source`), and the one `sentence`
///   the terminal prints (`docs/design/instruction-variants.md`).
///
///   **What it promises.** The array names EVERY targeted CLI, including the
///   ones with no instruction channel at all — a `file` of `null` is the
///   honest "house rules do not reach this tool", and an adapter that silently
///   disappeared from a coverage list would read as covered. `confirmation`
///   distinguishes "observed consuming this channel" from "documented or
///   protocol-level and never verified here", and the two are never collapsed.
///   `model_source` is always one of `toolset:<name>`, `settings`, or
///   `unknown`, so a panel can show *why* a variant was chosen rather than only
///   which.
///
///   **What it does not promise.** `used` is `false` on every live channel and
///   is a field rather than an omission for exactly that reason: no live
///   channel carries house rules today, confirmed or not, because none of them
///   varies by model or sits behind a lease. A `confirmed` channel is therefore
///   **not** an activation reading and never means instructions are being
///   served live — nothing is, and no surface may say otherwise. `selection`
///   reports the FIRST fragment that compiles for that CLI, not every one;
///   `agentstack instructions` is the per-fragment surface. And a `file` path
///   is a destination, not proof anything has been written to it — `rendered`
///   and `agentstack diff` answer that.
/// - `image-plan-v1`: `image --json` emits the packaging plan
///   (`docs/design/packaging.md`) under `image` — `toolset`, `harness`, `tag`,
///   `base`, a `posture` object, a `members` array giving every pinned member
///   its `kind`, `name`, `digest`, `provenance`, `dest` and `compiled` flag,
///   `required_secrets`, `blockers`, `buildable`, the `context` directory a
///   `--write` would stage, and the `cmd` the image's entrypoint execs.
///
///   **What it promises.** `members` is COMPLETE for the named toolset: every
///   member that would enter the image is listed with the digest its bytes are
///   read by, so a panel can show the composition without unpacking anything.
///   `required_secrets` is a list of `${REF}` NAMES and can never hold a
///   value — the whole build path constructs no secret resolver
///   (`CLAUDE.md` invariant 5). `buildable` is `blockers.is_empty()`, and a
///   plan with blockers exits non-zero while still emitting this payload, so a
///   UI can render *why* rather than only *that* a build refused.
///
///   **What it does not promise.** `posture.slug` / `posture.label` are the
///   shipped `Posture::Sandbox` values and describe what the artifact is
///   *prepared for*, never what any run enforces — `posture.established_by` is
///   the constant `"run"` and `posture.caveat` carries the sentence in full. It
///   is **not a build receipt**: the plan says what a `--write` would do, not
///   that an image exists, that Docker is present, or that anything was built;
///   nothing here is a reading of the local daemon. And it is not a
///   reproducibility claim beyond the AgentStack layer — the base image may be
///   a floating tag and a Docker build is not bit-reproducible, both stated in
///   the design doc rather than implied away here.
/// - `workflow-role-selection-v1`: the per-role model/effort facts become
///   panel-readable. Two payloads, one contract. Each `workflow list --json`
///   row gains `role_details[]` — `{role, harness, model, effort, serial,
///   undeliverable[]}` — and `workflow explain --json` now carries this
///   envelope, which it did not: it emitted a bare body, so a panel could read
///   the richest workflow payload the CLI has and could never *negotiate* it.
///
///   **What it promises.** The facts come from the SAME `role_selection` walk
///   `explain` renders, asked of the SAME authority the launch path asks — the
///   bound adapter's descriptor — so the tree a panel draws and the argv a run
///   builds cannot tell different stories. `undeliverable[]` carries one entry
///   per declared value that would not reach the child, each with its
///   `dimension`, the `harness`, and the plain sentence saying why; the two
///   distinct cases item 6 shipped stay distinct (the adapter has no notion of
///   that dimension at all vs. it has the setting but no confirmed way to
///   select it for a single headless launch), and a value the adapter's own
///   catalog REJECTS says so with the warning that a real run refuses that
///   child before launch. `role_details` rides on `list`, which is the
///   refusal-free surface, so an untrusted or drifted project still renders
///   its tree.
///
///   **What it does not promise.** `model` and `effort` are what the role's
///   toolset DECLARES, never what a run used — `null` means the toolset
///   declares none and the harness's own default applies, which AgentStack
///   does not read. `role_details` is NOT index-aligned with `roles`: a role
///   with no declared toolset, or one binding an unknown harness, contributes
///   no entry at all (refusal-free, exactly as `serial_roles` already is), so
///   a missing entry is "not established" and never "no model" — `run` and
///   `doctor` are where that entry's bigger problem gets reported. And this is
///   observation only: it is not an activation reading, and there is **no
///   action on this contract** — running, resuming, or authoring a workflow
///   from a panel stays deferred (UI control-plane §Deferred), because each
///   would need an authority path the read surface does not have.
/// - `abandoned-render-v1`: a native server config that is ON DISK while this
///   project routes that harness's MCP servers through the live lane is
///   reported, by every surface that reports anything about delivery, with a
///   remedy conjugated by AUTHORSHIP.
///
///   `agentstack why <name> --json` gains `abandoned[]` — the display names of
///   the harnesses whose config for that capability is a leftover render —
///   beside the `written[]` it already carried; the matching `written[]` row
///   for such a harness carries the sentence and its `↳ <remedy>` slot.
///   `doctor --json` reports each one as a `warn` section line whose `↳` slot
///   holds the COMMAND and nothing else, and `status` prints the same reading
///   and can promote it to `next_action`. `apply` names it at the moment of
///   writing. All four read ONE walk
///   (`commands::apply::abandoned_live_renders`) and one sentence
///   (`AbandonedRender::sentence`), so no surface can hold a second opinion.
///
///   **Disk is the trigger, the ledger only conjugates the remedy.** The
///   detector reads the file and lists the servers it actually declares. What
///   the state ledger decides is which of two commands is offered:
///   `agentstack x unrender --write` for a file AgentStack recorded writing,
///   and `agentstack adopt` for one it did not (a clone, a git checkout, a hand
///   edit). That split is not cosmetic — `x unrender` removes only
///   ledger-recorded entries, so offering it for a foreign file would name a
///   command that answers "nothing is ours to remove" and makes no progress.
///   A panel must render the remedy the payload gives it and must never
///   synthesise one from the presence of the finding.
///
///   `agentstack x unrender` exists as a command on this binary: it takes an
///   abandoned server config back off disk, previewing by default and writing
///   under `--write`, and it is reachable in one hop under `agentstack x`. It
///   is NOT a panel action — it is absent from [`PANEL_ACTIONS`], so a panel
///   may show the command as text for a human to run and may not invoke it.
///
///   **What it does not promise.** It is not an activation reading: a file on
///   disk says the harness may still be reading it, never that any server is
///   running. And `status --json` carries no `abandoned` key of its own — the
///   finding reaches a machine consumer there only through `next_action`, and
///   through `doctor --json`'s section lines. A panel wanting the per-harness
///   list asks `why <name> --json`.
///
/// # Breaking changes a panel must absorb
///
/// **Panel action names are unchanged.** Every entry in [`PANEL_ACTIONS`]
/// keeps its name, its verb and its binding, so no argv a panel builds breaks.
/// What changed is the SHAPE of four read payloads. Three fail loudly; one
/// fails silently, and that one is the dangerous one.
///
/// 1. `doctor --json`'s `next_action` is **nullable**. It is the machine
///    field, so it holds only commands that can run verbatim and make
///    progress, and `null` is the true answer over a healthy project. A
///    consumer typed to expect a string breaks on every healthy project —
///    loudly, and immediately. The human sentence is `next_step`, always
///    present, never something to exec. (`status-v1` states this; it is
///    repeated here because it is a change a live panel meets.)
/// 2. `status --json`'s `next_action` is now an **object**, not a string:
///    `{command, sentence, why}`, where `command` is runnable-or-null and
///    `sentence` is display prose. A consumer treating it as a string breaks
///    loudly. `command` is filtered by the same `machine_command` rule
///    `doctor` applies, so the two surfaces cannot hand a program different
///    commands for one state.
/// 3. **DANGEROUS — fails silently.** `drifted` is now a FOURTH value a
///    consumer meets (`trust-content-drift-v1`), and nothing throws when it
///    does. Each `review.items[].change` may now read `drifted` where the
///    field only ever held `added` / `changed` / `unchanged`; a decoder
///    matching those three words with a `default:` arm quietly files a drifted
///    item under whatever that arm says. `trust --preview`'s `state` keeps its
///    word set (`trusted` / `drifted` / `untrusted`) but now reaches `drifted`
///    in a NEW case — a project whose digest is intact while a pinned BODY was
///    edited in place — so a panel that only ever saw `drifted` after a
///    manifest edit now sees it over a manifest that never moved. Both are
///    silent failures: the payload parses, the types hold, and the panel
///    reports a project the gate refuses as healthy. `status --json`'s
///    `project.trust` takes the same reading, and
///    `review.groups[].counts.drifted` is the matching tally. Every consumer of
///    those fields must be audited by hand; a type error will not find this
///    one.
/// 4. `diff --json` **omits live-routed targets from `targets[]`** and names
///    them in `warnings` instead. Where the planner routes a harness's MCP
///    servers live, `apply` writes no server config, so there is nothing
///    rendered to compare — and `changed: false` is the wire form of "in
///    sync", which would be a claim about a comparison that never happened
///    (invariant 8). A panel that counts targets sees fewer than it expects
///    and must not read the absence as "clean"; the free-text `warnings` entry
///    names each withheld harness and whether it is served live or planned
///    live and not connected. This fails loudly only if the panel asserts a
///    target per configured CLI; otherwise it under-reports, so treat the
///    `warnings` line as the authority on what was not compared.
pub const FEATURES: &[&str] = &[
    "init-plan",
    "apply-setup",
    "init-tool-managed-v1",
    "trust-preview",
    "trust-consent",
    "status-v1",
    "profiles-v1",
    "diff-v1",
    "restore-last",
    "sessions-v1",
    "profiles-edit-v1",
    "diff-ownership-v1",
    "toolset-create-v2",
    "profiles-edit-batch-v1",
    "toolset-rename-v1",
    "toolset-delete-v1",
    "library-remove-v1",
    "manifest-remove-v1",
    "trust-server-blockers-v1",
    "trust-review-card-v1",
    "trust-card-diff-v1",
    "trust-card-groups-v1",
    "activity-skill-load-v1",
    "workflow-observe-v1",
    "workflow-serial-roles-v1",
    "doctor-advisories-v1",
    "doctor-mode-v1",
    "doctor-probe-v1",
    "diff-existence-v1",
    "json-reads-v1",
    "gitignore-opt-out-v1",
    "doctor-cli-coverage-v1",
    "status-honesty-v1",
    "needs-your-yes-v1",
    "update-offer-v1",
    "package-members-v1",
    "lease-status-v1",
    "delivery-routing-v1",
    "library-sources-v1",
    "instruction-channels-v1",
    "image-plan-v1",
    "workflow-role-selection-v1",
    "trust-content-drift-v1",
    "abandoned-render-v1",
];

/// How one panel action's apply is bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Consent {
    /// The apply requires `--yes` AND the named flag carrying the digest the
    /// matching `--preview` returned. The CLI recomputes the digest and refuses
    /// any mismatch before writing a byte, so a reviewed preview can never be
    /// replayed against state that moved underneath it.
    Digest(&'static str),
    /// No digest, because the verb introduces no content for a human to
    /// review: it re-renders, adopts, reverts, or undoes material that was
    /// already consented (or that the CLI's own gates re-check on the call).
    /// These are still fixed verbs with fixed argv — never a command string —
    /// and every precondition behind them is re-validated here.
    Preconditions,
}

/// One entry in the panel's closed action surface.
pub struct PanelAction {
    /// The panel-side action name (t3code's `AgentstackActionKind`).
    pub name: &'static str,
    /// The clap subcommand path its fixed argv starts with.
    pub verb: &'static [&'static str],
    /// How the apply is bound.
    pub consent: Consent,
}

/// **The closed set.** Every state-changing thing a panel can do, declared in
/// the CLI rather than only in the panel's own TypeScript — so "the panel is
/// never a second authority" is a property this repository can enumerate and
/// witness (`crates/cli/tests/panel_surfaces.rs`) instead of a promise made
/// somewhere else. There is no generic `run_command` action, and adding an
/// entry here is a deliberate edit that fails the witness until the verb really
/// carries the binding it claims.
///
/// Reads are deliberately NOT listed. A read writes nothing, so it needs no
/// closed set: [`FEATURES`] is how a panel learns which read contracts this
/// binary serves. This table is about the surface that can change state.
///
/// Two things it is not:
///
/// 1. **Not an authorization.** Nothing consults this table at runtime — no
///    enforcement decision may read it, exactly as none may read [`FEATURES`].
///    A panel that names an action here still meets every gate the terminal
///    meets: trust state, strict lock verification, machine policy, and the
///    digest itself.
/// 2. **Not a per-item consent path.** The review card has exactly one closing
///    question and one answer — `trust-grant`, over the whole project's bytes.
///    No entry here accepts, keeps-pinned, or blocks a single reviewed item,
///    and none ever will (`docs/archive/design/consent-card.md` §Panel).
pub const PANEL_ACTIONS: &[PanelAction] = &[
    // Setup and consent: the two digests that bind a reviewed preview.
    PanelAction {
        name: "setup-apply",
        verb: &["init"],
        consent: Consent::Digest("consented-plan"),
    },
    PanelAction {
        name: "trust-grant",
        verb: &["trust"],
        consent: Consent::Digest("consented-digest"),
    },
    // Revoking withdraws a yes rather than giving one: there is no reviewed
    // preview to bind to, and failing to revoke is the unsafe direction.
    PanelAction {
        name: "trust-revoke",
        verb: &["trust"],
        consent: Consent::Preconditions,
    },
    PanelAction {
        name: "apply-project",
        verb: &["apply"],
        consent: Consent::Preconditions,
    },
    PanelAction {
        name: "apply-global",
        verb: &["apply"],
        consent: Consent::Preconditions,
    },
    PanelAction {
        name: "adopt-project",
        verb: &["adopt"],
        consent: Consent::Preconditions,
    },
    PanelAction {
        name: "adopt-global",
        verb: &["adopt"],
        consent: Consent::Preconditions,
    },
    PanelAction {
        name: "guard-install",
        verb: &["guard", "install"],
        consent: Consent::Preconditions,
    },
    // Bound by the id the `restore --json` inventory returned, never `--last`:
    // the undo ledger is machine-global and a project panel must undo its own
    // newest entry.
    PanelAction {
        name: "restore-write",
        verb: &["restore"],
        consent: Consent::Preconditions,
    },
    PanelAction {
        name: "session-start",
        verb: &["session", "start"],
        consent: Consent::Preconditions,
    },
    PanelAction {
        name: "session-end",
        verb: &["session", "end"],
        consent: Consent::Preconditions,
    },
    // The panel-edit verbs: manifest or library mutations, each previewed and
    // each bound to the digest its own preview returned.
    PanelAction {
        name: "add-skill-to-profile",
        verb: &["add-skill-to-profile"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "add-server-to-profile",
        verb: &["add-server-to-profile"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "create-profile",
        verb: &["create-profile"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "edit-profile",
        verb: &["edit-profile"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "rename-profile",
        verb: &["rename-profile"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "delete-profile",
        verb: &["delete-profile"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "use-profile",
        verb: &["use-profile"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "remove-from-library",
        verb: &["remove-from-library"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "remove-capability",
        verb: &["remove-capability"],
        consent: Consent::Digest("consented"),
    },
    PanelAction {
        name: "set-gitignore",
        verb: &["set-gitignore"],
        consent: Consent::Digest("consented"),
    },
];

/// Contract names this binary KNOWS and deliberately no longer serves.
///
/// A panel gates an affordance on `features.includes(name)`. That answers one
/// question — can I call it? — and conflates two very different noes: a binary
/// too old to have the contract, and a binary that retired it. The first will
/// gain the affordance on upgrade; the second never will. A UI that cannot
/// tell them apart shows an "update AgentStack" prompt that no update fixes.
///
/// So a retired name moves from [`FEATURES`] to here and stays here. The
/// envelope carries both lists, and a name may never appear in both.
///
/// - `set-mode-v1`: the Mode axis retired (STRATEGY.md v3, TODO.md item 9).
///   Mode asked the user to choose between static, clean-at-rest and
///   zero-files. v3 deleted that choice: the delivery planner routes each
///   capability by kind and harness, and `status` reports what it decided.
///   A mode picker is therefore a control over something the user no longer
///   decides, so the command refuses rather than switching. The un-render leg
///   it shared with `uninstall` is unchanged and still reachable there.
pub const SUPERSEDED: &[&str] = &["set-mode-v1"];

/// Wrap a response body in the envelope. The two envelope keys are injected
/// into the body object so existing consumers keep their field paths; a
/// non-object body would be a programming error and panics in debug builds.
pub fn envelope(body: serde_json::Value) -> serde_json::Value {
    let mut map = match body {
        serde_json::Value::Object(map) => map,
        other => {
            debug_assert!(false, "envelope() needs a JSON object, got {other}");
            let mut map = serde_json::Map::new();
            map.insert("body".into(), other);
            map
        }
    };
    map.insert("schema_version".into(), SCHEMA_VERSION.into());
    map.insert(
        "features".into(),
        serde_json::Value::Array(FEATURES.iter().map(|f| (*f).into()).collect()),
    );
    map.insert(
        "superseded".into(),
        serde_json::Value::Array(SUPERSEDED.iter().map(|f| (*f).into()).collect()),
    );
    serde_json::Value::Object(map)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_injects_version_and_features_without_touching_body() {
        let out = envelope(serde_json::json!({"a": 1}));
        assert_eq!(out["schema_version"], SCHEMA_VERSION);
        assert_eq!(out["a"], 1);
        let features: Vec<&str> = out["features"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(features, FEATURES);
        // Every name ever shipped must still be SERVED. That — not position —
        // is the property external UIs depend on: they gate an affordance with
        // `features.includes("<name>")`, so dropping or renaming one silently
        // disables a working button, while inserting a new name anywhere is
        // harmless. An earlier version of this test pinned the order instead
        // and failed the moment a contract was added mid-list, which tested
        // the test rather than the contract. Removing a name is a
        // schema-version bump, so it must break here first.
        for shipped in [
            "init-plan",
            "apply-setup",
            "trust-preview",
            "trust-consent",
            "status-v1",
            "profiles-v1",
            "diff-v1",
            "restore-last",
            "sessions-v1",
            "profiles-edit-v1",
            "workflow-observe-v1",
            "workflow-serial-roles-v1",
            "doctor-advisories-v1",
            "doctor-mode-v1",
            "diff-existence-v1",
            "trust-review-card-v1",
            "trust-card-diff-v1",
            "activity-skill-load-v1",
            "needs-your-yes-v1",
            "update-offer-v1",
            "package-members-v1",
            "lease-status-v1",
            "library-sources-v1",
            "instruction-channels-v1",
            "image-plan-v1",
            "workflow-role-selection-v1",
        ] {
            assert!(
                features.contains(&shipped),
                "FEATURES dropped '{shipped}' — a UI gating on that name loses a working \
                 affordance, so removing one needs a schema-version bump"
            );
        }
        // No duplicates: a repeated name means two contracts think they own it.
        let mut sorted = features.clone();
        sorted.sort_unstable();
        let before = sorted.len();
        sorted.dedup();
        assert_eq!(before, sorted.len(), "FEATURES contains a duplicate name");
    }

    /// The abandoned-render reading is advertised, and it is a READ contract
    /// only: `agentstack x unrender` removes a file, so it may never become a
    /// panel action without its own deliberate entry in [`PANEL_ACTIONS`].
    #[test]
    fn abandoned_render_is_advertised_and_is_not_a_panel_action() {
        assert!(
            FEATURES.contains(&"abandoned-render-v1"),
            "a panel cannot gate the abandoned-render reading it cannot negotiate"
        );
        assert!(
            !SUPERSEDED.contains(&"abandoned-render-v1"),
            "a name may never be both served and retired"
        );
        for action in PANEL_ACTIONS {
            assert_ne!(
                action.verb,
                &["x", "unrender"],
                "`x unrender` deletes a config file; a panel names it as text for \
                 a human to run, and does not invoke it"
            );
            assert_ne!(action.name, "unrender");
        }
    }

    /// A retired name is advertised as retired, and never as both. The two
    /// lists together are what lets a panel say "this will never come back"
    /// instead of "try updating".
    #[test]
    fn superseded_is_disjoint_from_features_and_advertised() {
        for name in SUPERSEDED {
            assert!(
                !FEATURES.contains(name),
                "'{name}' is in FEATURES and SUPERSEDED — a UI cannot tell whether it is served"
            );
        }
        let out = envelope(serde_json::json!({}));
        let advertised: Vec<&str> = out["superseded"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(advertised, SUPERSEDED);
        assert!(
            advertised.contains(&"set-mode-v1"),
            "the Mode axis retired in v3; a panel still offering a mode picker must be able to see that"
        );
    }

    /// Every served contract is written down, and none is written down as
    /// absent. `docs/integrations.md` is where an integrator learns which
    /// affordances this binary can gate on, so the page drifts in two
    /// directions and both mislead: a name added to [`FEATURES`] with no row
    /// there is a contract nobody can find, and a row still marked
    /// "next release" over a name this build serves tells an integrator not to
    /// build something that already works. Both had happened — fourteen names
    /// were undocumented and six rows outlived the release they described.
    ///
    /// The page is READ at test time rather than embedded with `include_str!`,
    /// because embedding would bake the bytes into the binary and make the
    /// check a property of whoever last rebuilt; reading means editing the
    /// page is what re-runs it. `CARGO_MANIFEST_DIR` is `crates/cli`, so the
    /// repository root is two levels up — the same anchor
    /// `tests/docs_commands.rs` uses.
    #[test]
    fn every_served_contract_is_documented() {
        // The marker the page uses for a contract this build does NOT serve.
        // Nothing may carry it today; it stays named here so re-introducing it
        // over a served name fails in this repository rather than in an
        // integrator's version negotiation.
        const NOT_SERVED: &str = "no — next release";

        let page =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/integrations.md");
        let text = std::fs::read_to_string(&page)
            .unwrap_or_else(|e| panic!("cannot read {}: {e}", page.display()));

        for name in FEATURES {
            // Match the backticked spelling, so `status-v1` is not satisfied by
            // a line that only mentions `lease-status-v1`.
            let quoted = format!("`{name}`");
            let mentions: Vec<&str> = text.lines().filter(|l| l.contains(&quoted)).collect();
            assert!(
                !mentions.is_empty(),
                "'{name}' is served but docs/integrations.md never names it — an \
                 integrator cannot gate on a contract that is not written down"
            );
            for line in mentions {
                assert!(
                    !line.contains(NOT_SERVED),
                    "docs/integrations.md marks '{name}' as \"{NOT_SERVED}\" while this \
                     build serves it: {}",
                    line.trim()
                );
            }
        }
    }
}
