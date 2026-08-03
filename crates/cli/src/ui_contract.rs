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
/// - `set-mode-v1`: `set-mode <static|clean-at-rest|zero-files>` switches the
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
/// - `library-sources-v1`: `status --json` gains a `shadowed_names` array —
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
/// - `instruction-channels-v1`: `status --json` gains an `instruction_channels`
///   array — one row per targeted CLI with `id`, `display`, the `file` that
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
pub const FEATURES: &[&str] = &[
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
    "set-mode-v1",
    "status-honesty-v1",
    "needs-your-yes-v1",
    "update-offer-v1",
    "package-members-v1",
    "lease-status-v1",
    "delivery-routing-v1",
    "library-sources-v1",
    "instruction-channels-v1",
];

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
}
