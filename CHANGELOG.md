# Changelog

User-facing changes per release. The [GitHub Releases
page](https://github.com/Tarekkharsa/agentstack/releases) carries the built
binaries, checksums, and provenance attestations for each entry.

## v0.18.0-rc.2 — 2026-08-01

**The study byte-set.** rc.1's five-lane review produced a blocker queue
(`FINDINGS.md`); this candidate clears it. The activation study recruits on
this version — never on rc.1. Like rc.1, this is a pre-release: `install.sh`
and `brew install` keep serving v0.17.1 until v0.18.0 is final.

- **Consent core hardened, witnessed end to end.** Every BLOCKING finding
  (F1–F11) closed with a tampering witness. Headline: the accept path now
  refuses a content swap performed in the review→commit window — witnessed by
  a real on-disk swap test driven through the production seam, verified
  independently including a guard-removal mutation check (the test fails when
  the guard is deleted). Nothing pins that was not displayed.
- **One grant path everywhere.** The refusal semantics hold across every
  surface that can reach a grant — `trust`, `yes`, and the panel action route
  through the same gate (F7, F8), and the funnel is reachable from every
  surface that detects a drop (F9).
- **Intake is one hostile-input pass.** The from-anywhere surfaces (dropped
  files, cloned content, `add from`) share one defensive parse (F1–F3,
  F13–F18).
- **Green means verified.** Status surfaces no longer report ready states they
  did not check, and every dead end names its exit (F10–F12, F19–F22).
- **Share hardening.** Signed-message fallback uses a sentinel and
  `deny_unknown_fields` on the envelope.
- **Docs match the territory.** The findings ledger, `ENFORCEMENT.md` (incl.
  the lifecycle-hooks honesty statement and cross-reference), and the command
  reference are reconciled with what ships.

**Known-open, deliberately (not regressions, labeled here so they read as
known):**

- Inbound `add` does not yet thread attribution into `Lock::upsert` —
  production `LockedSkill` sites still write `None` for origin.
- `fetch_url` uses a bare HTTP client (SSRF class; low severity — the URL
  comes from the user's own argv, not repo content). The planned fix reuses
  the egress crate's resolve-once address-class validation.

## v0.18.0-rc.1 — 2026-08-01

**A release candidate, published for the activation study.** This is not the
latest release: `install.sh` and `brew install` keep serving v0.17.1 until
v0.18.0 is final, and v0.18.0 is not final until the study passes. If you were
sent here to try something, you were given a version-pinned install command;
if you arrived by accident, you probably want v0.17.1 instead.

**What it is.** Everything since v0.17.1 is one journey, built in four passes,
and this is the first release that carries all of it:

- **Drop a file and it counts.** Put a skill or an instruction fragment in
  `.agentstack/`, and the tools notice. For content you demonstrably wrote
  yourself, `agentstack yes` takes it from a file on disk to working in every
  CLI you use, behind one review and one confirmation.
- **One yes, with everything disclosed.** The review opens with two to five
  plain lines: what will run, what it will contact, what secrets it can read,
  whether the bytes are pinned. Nothing a project declares is missing from it.
- **Change it and you see the diff.** When approved content changes, the
  re-review shows what moved and offers three answers: accept the new bytes,
  keep the ones you already approved, or block the item and carry on without
  it.
- **Refusals explain themselves and leave evidence.** Every block says what
  was stopped, why, and the one safe thing to do about it — and is recorded,
  so you can look it up afterwards instead of needing to have been watching.
- **Undo is a first-class verb.** `agentstack undo` lists recent changes
  newest-first; pick a point and go back. The undo is itself recorded, so it
  too can be undone.
- **`agentstack up`** sets up a new machine from a setup that already exists.
- **`agentstack share` / `agentstack receive`** move a setup between people.
  Sharing signs; receiving reviews.
- **`agentstack add from`** takes capabilities from outside ecosystems through
  the same staged review as everything else.

### Drop a file, then one yes

- **Undeclared content in `.agentstack/skills/` and `.agentstack/instructions/`
  is noticed.** `status`, `doctor`, `use`, `lock`, and `adopt` all see a file
  you dropped and offer to adopt it, with a preview, instead of ignoring it
  because no manifest entry names it.
- **`agentstack yes` is new.** It performs declare → lock → trust → render
  behind one review and one confirmation, through the same functions and the
  same gate the explicit four-command sequence uses. Declining leaves the
  project byte-identical: nothing recorded, nothing materialized.
- **`agentstack adopt --to-library`** adopts into the shared central library
  rather than only this project.
- **Whether content is "your own work" is decided by evidence, not assumption.**
  Inside a git work tree, tracking decides. Outside one, the clock is the last
  recorded trust grant. Content that fails this check does not get the
  compressed path — it takes the full explicit review.

Four defects found by adversarial review before this shipped, listed because
each was a real way to be misled: a `git checkout` rewriting mtimes could
promote pulled content to "your own work"; a symlinked `SKILL.md` was
followed; a dropped file sharing a name with a pinned git or library
declaration could replace it behind a preview that called it an addition; and
an adopted skill joined no toolset, so adopting alone did not make it usable.

### The yes gets a card — and stops omitting two capability kinds

The trust review now opens with two to five plain lines saying what the
project will run on your machine, what it will contact, what secrets it can
read, and whether the content is pinned to the bytes you are reading. The full
line-by-line review follows it unchanged: the card summarizes the review, it
never replaces it. Same gate, same digest, same single grant path — this is
presentation.

- **Changed content gets a diff and three answers.** When bytes you already
  approved have moved, the re-review shows the difference and offers accept,
  keep-the-approved-bytes, or block. A blocked item is excluded at delivery
  and says so in a standing status line, rather than silently reappearing.
- **The bytes you approved are kept.** Approving content deposits it in a
  content-addressed store as part of the act of pinning, so "keep what I
  approved" has something real to keep.
- **Content you have approved before is recognized.** A short line says so and
  the card's body shortens. It is machine-local, keyed by content digest, and
  changes only what the card says — never the outcome, never the gate. A
  machine-level "always allow this anywhere" is deliberately not built.

### Setup, Toolset, Status, Undo

- **`agentstack doctor` always ends with exactly one recommended command.** In
  the JSON, `next_action` is no longer nullable — `state` already answers
  whether anything is wrong, so this key is free to answer "what now?"
- **Green means verified.** A check that examined nothing now says so in words
  instead of reporting a pass. A report cannot claim a pass over an empty set.
- **Every refusal is one plain sentence.** What was stopped, why, and the safe
  next step, with a per-family clause naming what did not happen ("nothing
  ran", "nothing was sent", "nothing was read", "nothing was written"). Two
  families that recorded nothing now do: host-path egress, and secret-scope
  refusals.
- **`agentstack undo` is new** — the interactive face of `restore`. Recent
  changes newest-first, pick a point, revert to it. It adds no new destructive
  machinery, and it records the pre-undo bytes, so the one action that changes
  your files is now itself in the ledger and itself undoable.
- **First contact speaks in ideas, not mechanisms.** Default `--help` and
  `status` name Setup, Toolset, Status, and Undo; the glossary of manifests,
  locks, digests, and gateways moved to `--help --all`, which opens by
  defining itself. Nothing was renamed.
- **`agentstack explain` covers every declared kind.** It covered three of
  seven and told the other three they did not exist, while the review card
  listed all seven — so `explain` disagreed with the surface you had already
  said yes to. `status` now counts every declared kind too (it counted two of
  seven), and `[settings.*]` gained the `doctor` section it never had.

### Anywhere: a new machine, sharing, and outside ecosystems

- **`agentstack up` is new.** One command on a machine that has a checkout and
  nothing else: it finds the CLIs you have, verifies the environment against
  `agentstack.lock`, renders each CLI's config, and names what is left — which
  on a new machine is this machine's secrets. `init` is for a setup that does
  not exist yet; `up` is for one that does.
- **`agentstack share` and `agentstack receive` are new.** Sharing signs: a
  bundle is signed as part of sharing rather than through a separate step
  nobody performs. Receiving reviews: the bundle is staged inert, carded, and
  activates only if you say yes. A valid signature from a publisher you have
  chosen to recognize changes what the card says — it settles whose key it is
  — and changes nothing else. **No signature, from anybody, replaces the yes.**
  An unsigned bundle and an invalid signature are both named on the card, and
  the full review stands in both cases.
- **`agentstack add from` reads outside ecosystems.** It accepts a URL or path
  to an eve-format skill package, an MCP connection definition, or a registry
  listing, and puts every byte through the same funnel: fetched, bounded,
  quarantined, carded, and only then — on your yes — added. Credentials found
  in a fetched connection become `${REF}` placeholders on the way in and are
  never written down. A registry listing only ever lists; nothing installs
  from reading a catalog. AgentStack hosts no registry or marketplace of its
  own — this consumes ecosystems, it does not create one.
- **Licence and origin travel with imported content.** `[[skill]]` lock
  entries gained `license` and `origin`, and `LICENSE`/`NOTICE` text is carried
  with the files rather than summarized into a tag. The review card shows it
  ("Apache-2.0, from …"), and a source that declared no licence is stated as
  such. This records what a source declared and verifies none of it.
- **The content-pinning refusal now leaves evidence.** When delivered bytes do
  not match what you reviewed, the server is dropped — that was already true
  and unchanged. What is new is that it now says so in the standard one-line
  form and records the refusal, so the block that matters most is one you can
  look up afterwards.

### What this release does not do

Stated plainly, because each is a reasonable thing to assume and none of it is
true:

- **Zero-files ("dynamic") delivery is not the default,** and this release
  does not change that. It remains opt-in.
- **Hooks and extensions keep the full consent ceremony.** They are executable
  capability kinds; no compressed path covers them, and `agentstack yes` does
  not touch them.
- **Servers are not adopted from a dropped file.** They still require a
  declaration.
- **A signature is not a review, and recognition is not consent.** Verifying a
  signature proves bytes came from a key unchanged. It says nothing about
  whether the content is safe, nothing about who the key really belongs to,
  and it never shortens the gate.
- **Quarantine is not a sandbox.** Staged content is inert because nothing is
  arranged to read it, not because something confines it.
- **Attribution is recorded, not verified.** A source claiming `Apache-2.0`
  gets `Apache-2.0` written down.
- **An unresolved `${REF}` holds back that CLI's whole config,** not just the
  server that needs the secret. This is the documented fail-closed rule;
  relaxing it is separate, reviewed work.

### Fixed

- **`agentstack yes` promised an undo it had not recorded.** It printed "Undo
  any of it with `agentstack restore --last --write`" before writing and again
  on success, but recorded no history row — so on a skills-only project both
  `agentstack undo` and `restore --list` answered "nothing recorded" and the
  promised undo did not exist. The promise predated the ledger row. Accepting
  now records one revertable entry covering the declaration and the pin,
  through the same history seam `apply` and `init` use, and the message names
  only what that entry puts back: the files already delivered into each CLI
  are reconciled by `agentstack use --write`, which the message now says. A
  narrow true promise beats a wide false one. Found by replaying the journey
  against this release candidate, not by review.

- **`doctor` reported `state: "ready"` for a project nothing had activated.**
  Zero errors and zero warnings was true; "ready" was not, because an
  untrusted, never-activated project has nothing live in it. `state` keeps its
  `status-v1` meaning — a panel already rendering "Ready" from it does not
  silently change meaning under its users — and a new `readiness` field
  answers the honest question, behind a new feature name `status-honesty-v1`.
  It takes one of `ready`, `needs_attention`, `untrusted`, `drifted`,
  `never_activated`, `unknown`, or `needs_setup`. External panels should
  render `readiness` and treat everything except `ready` as not-live. The
  dashboard snapshot also gained a singular `nextAction` beside its plural
  array, matching the one-decision shape `doctor` and `status` now use.

- **`doctor` printed a green "resolved from env" for a secret no server could
  read.** If `[policy.secrets]` refuses a reference for every server that uses
  it, the reference resolves and nothing can read it. The Secrets section said
  it was fine while the Policy section raised an error about the same thing.
  It now says which, and why.

- **A refused secret was told to do work that could not help.** A policy
  refusal was given the advice for a missing value — "set it with `agentstack
  secret set`" — which does not address a rule that refuses it.

- **The machine-readable trust surface omitted hooks and settings too.**
  `agentstack trust --preview` is what an external UI reads to show you a
  project before you approve it. Like the terminal screen, it listed servers,
  skills, workflows, extensions and instructions — and said nothing about
  `[hooks.*]` or `[settings.*]`. **A panel built on that payload therefore
  showed a project's executable surface as smaller than it actually was**, and
  the contract documentation described the payload as "the full reviewed
  surface", which was not true.

  The preview now carries hooks (labelled as executable), settings, the
  requested policy, and the machine-policy ceiling, behind a new feature name
  `trust-review-card-v1`. Two omissions remain deliberate and are now written
  down rather than implied: a library server whose definition has drifted from
  its pin stays redacted in the JSON (so an external UI cannot bind consent to
  bytes the digest does not cover — the terminal review shows the live command
  line instead, because the authoritative card may never disclose less), and
  the payload's blockers cover servers and local executables only, because
  computing the rest requires resolution steps that must not run on a read-only
  command. `agentstack trust` remains authoritative.

- **The review screen did not show hooks or settings.** `[hooks.*]` and
  `[settings.*]` are declared capability kinds. Editing one changes the
  manifest bytes, so the trust digest moved and AgentStack correctly asked you
  to review the project again — but the screen it showed you listed servers,
  skills, instructions, workflows, extensions and policy, and said nothing
  about the hook or the settings block. **Grants made before this release
  therefore approved hooks and settings that the review never displayed.** A
  hook is executable: it runs a command in or around the harness at your
  permission. Both kinds are now disclosed, and a hook's wildcard target
  (`["*"]`, previously rendered as a bare `[*]`) is spelled out as "every
  hook-capable CLI" rather than left as the widest possible scope shown as the
  least alarming glyph.

  Digests are unchanged, so **nothing re-gates automatically** — already-trusted
  projects stay trusted and you will not be re-prompted. If you want to see
  what you previously approved, run `agentstack trust` in the project and read
  the card; `agentstack trust --revoke` withdraws consent if it is not what you
  expected.

  A structural check (`tools/check-structure.py`, the `:review` requirement)
  now fails the build if any manifest kind lacks a disclosure site on the card,
  so this class of omission cannot recur silently.

- **Undo is named before the write, not after it.** `adopt` never named its
  undo at all — neither before nor after writing. `apply`, `use --write` and
  `session start` named it only in the success summary, once the files had
  already moved. All four now say the way back before the first byte changes.

## v0.17.1 — 2026-07-31

**The pilot's blocker, fixed before the study runs.** The §1.6 activation
study rehearsal found a shape that failed silently: a project whose whole
agent setup lives in project-scope config files (`.mcp.json`,
`.codex/config.toml`) and nothing in the user's home. Every surface except
`adopt` was asking a machine-scope question — "is this CLI installed here?" —
and answering a project-scope one with it. So `status` said "none detected on
this machine" over four servers in the working directory, `init` wrote an
empty manifest, and `doctor` then reported zero problems with it. The
capability was never missing; only the discovery was. Participants install
the latest release, so the study now tests the fixed journey.

### Fixed

- **Project-scope detection.** `status`, `init`, `doctor`, and target fan-out
  now ask whether a CLI is configured *in this directory*, not just on this
  machine. A repo carrying only `.mcp.json` or `.codex/config.toml` is no
  longer reported as "none detected on this machine".
- **`init` imports project-scope configs** instead of writing an empty starter
  manifest over them. If a config is present that cannot be imported, `init`
  names the file and points at `agentstack adopt` rather than staying silent.
- **The false ready.** `doctor` gains an `Unmanaged setup` check: servers
  configured here that the manifest does not declare are a warning naming the
  file, the servers, and `agentstack adopt`. A clean `doctor` now means ready.
  The section is hidden when nothing is uncovered.
- **One truthful CLI count.** The orientation screen no longer prints
  "none detected" above "13 detected CLI(s)" — detected-here and the adapter
  catalog are stated as the two different facts they are.
- **Next-step chain.** After a completed setup, `status` advances to
  `toolset create` instead of re-offering `doctor`; a trust-stale project's
  Next line names `agentstack trust .` directly instead of routing through
  `doctor` first.
- `init`'s post-import note no longer tells a project-scope import to run
  `apply --scope global --write` — those files are the ones `apply` manages.
- The interactive tutorial's install transcript and the cookbook's CI recipe
  now pin the current release; both had stayed one release behind.

### Added

- **The panel preview is a public docs page** (`/panel/`, in the site nav
  beside the tutorial): the interactive walkthrough of the graphical
  companion — every question a user arrives with, answered by the panel and
  the CLI side by side, including the trust gate, drift, and denial states.
  The page says plainly that the panel isn't publicly downloadable yet and
  that every action shown maps to a CLI command that ships today. It passes
  the same release-grade site checks as every other page (H1/skip-link
  keyboard path, reduced motion honored, WCAG AA contrast, focusable
  terminal regions) and the toolset-vocabulary gate.

## v0.17.0 — 2026-07-29

**The release the activation study installs.** Before putting the launch
journey in front of five strangers, the maintainer ran it cold — sandboxed
HOME, release binary, two CLIs — and fixed everything that run surfaced:
doctor no longer contradicts a fresh toolset switch, `toolset create backend`
works the way every first attempt typed it, and the errors and success lines
along the way stopped speaking in internals. The recovery pair (`restore`,
`adopt`) is in the default `--help`, toolsets can be renamed and deleted, and
the release workflow itself — whose first v0.16.0 run quietly lost its draft —
now verifies what it built. Adding those two toolset verbs turned up a fence
that failed open: a toolset disappearing out from under a live session widened
what that agent could reach instead of narrowing it, which is fixed here. The §1.6 study kit ships in
`docs/design/activation-study.md`; the study is what stands between this
release and a validated launch.

### Security

- **A toolset that disappears out from under a live session now fences to
  nothing, not to everything.** The gateway resolves a session's fence by name
  on every call. When that name stopped resolving — the toolset renamed,
  deleted, or hand-edited away mid-session — the fence resolved to "no profile",
  and "no profile" does not mean "no servers": it means every server in the
  manifest plus every profile's. So the agent's reachable surface silently
  *widened* at exactly the moment the configuration was in an unexpected state,
  with nothing in the way (the host gateway is built without the trust gate).
  The lease fence already failed closed here; the ambient-session fence now
  does too (and the pinned fence, which today only the frozen-run path
  constructs, so the one user-visible change is the session case). A fence that
  names no toolset at all is unchanged — that legitimately means "nothing is
  fenced" and still serves the declared surface.

### Fixed

- **`doctor` and `diff` no longer contradict a fresh toolset switch.** Both
  now remember which toolset a `use --write` activated and compare each CLI's
  on-disk config against that selection — previously a successful switch was
  immediately reported as "changes pending ↳ apply --write", and following
  doctor's own cue silently widened the render back to the full manifest,
  undoing the switch. Their fix cues now name the command that preserves the
  selection (`agentstack use <toolset> --write`); a full `apply --write`
  still renders everything, and now says which active toolset selection it
  replaced and how to switch back. `session end` and `toolset rename` keep
  the recorded selection consistent (restored and followed, respectively).
  Found by the pre-study dry run of the §1.6 activation journey.
- The no-such-toolset error now points at `agentstack toolset list` instead
  of telling a first-time user to go read `[profiles.*]` tables in the
  manifest, and `toolset create`'s undo hint names
  `agentstack toolset delete` instead of hand-editing TOML — that command
  exists now.
- The lock summary counts only what actually pinned. The beginner journey's
  first success line printed six "+ 0 …" segments of internal category names;
  a two-server pin now reads "pinned 2 server(s) from 1 toolset(s)".

- The release workflow now builds and attests the sidecar first, creates the
  complete draft once, uploads by the server-provided URL, and verifies the
  exact GitHub release ID and seven assets. The first v0.16.0 run created an
  `untagged-*` draft, and a later release-note update silently replaced the real
  tag with another placeholder; every job stayed green while `latest` remained
  v0.15.0. A manual existing-tag input can rebuild a lost draft without moving
  or recreating the tag, and publication remains an explicit human action.
- The generated Homebrew formula now installs from Homebrew's stripped archive
  root instead of looking for a top-level directory that no longer exists at
  staging time. The official tap passes a clean install and formula test.
- Release references in the automation example, CI recipe, and interactive
  tutorial now agree on v0.16.0.

### Added

- **`edit-profile` edits a toolset's membership as one batch, with an
  inverse.** Repeatable `--add-skill/--add-server/--remove-skill/
  --remove-server` apply the whole intent as one manifest write under one
  consent digest, then re-lock and re-render once — previously composing a
  toolset cost a full pipeline per capability, and there was no way to take
  a member back out at all. Removal ends the membership only: the capability
  stays declared in the manifest and stays in the library. An empty batch,
  or a name on both sides of the same change, is refused before any write.
- A study kit for the §1.6 activation study
  (`docs/design/activation-study.md`): recruiting message, participant
  criteria, observation protocol, metrics sheet, and a results template that
  maps 1:1 onto the Stage 1 gate.
- Release-grade docs browser checks now cover every canonical sitemap page at
  phone and desktop widths: landmarks, keyboard skip navigation, theme
  switching, reduced motion, overflow/console errors, and axe WCAG A/AA scans.
- Migration recipes for Claude + Codex, Cursor + Gemini, dotfiles, teams without
  shared secrets, and complete removal.
- Code of Conduct, maintainer/succession and funding posture, and explicit
  support routes/window for the planned public launch.

### Changed

- `toolset create|rename|delete` accept the name positionally
  (`agentstack toolset create backend`), the spelling every comparable CLI
  taught first-time users to try; `--name` remains as an equivalent flag for
  scripts. Both spellings run the one authority path and produce the same
  consent digest, and the frozen `*-profile` panel argv is untouched.
- `toolset create` and `toolset rename` close by pointing at
  `agentstack use <name> --write` — the persistent switch the beginner
  journey teaches — with `session start` as the temporary alternative,
  instead of steering everyone to sessions.
- The CLI portability and recovery loop is the public launch path. t3code is
  described consistently as an optional private graphical companion, and the
  old workflow-first t3code launch plan is labelled a closed prototype record.
- `restore` and `adopt` are visible in the default `--help`. Undo is one of
  the four beginner concepts, so the way back is findable without
  `--help --all`.
- `TODO.md` follows its own rule again: every closed item is one line plus a
  date or commit ref (review finding M12), stale "uncommitted" annotations
  were corrected against git, and the F13/F14 workflow review items are
  recorded as shipped in v0.16.0.

## v0.16.0 — 2026-07-28

**The release an end-to-end review asked for.** Two independent product
reviews walked the whole journey — install, import, apply, check, switch,
undo, remove — and reported thirty-two findings, most of them coherence
rather than correctness: a tool that was well built but read as unfinished
ninety seconds after the happy path ended. This release closes every one of
them that was a defect. The plaintext `.env` is written `0600`; success
summaries count what they covered instead of what changed; `--help` fits a
screen; `create-profile` no longer activates what it names; undo entries say
which change they are; the product says **toolset** in every sentence a
person reads; `init` targets the CLIs that actually contributed config
instead of every binary on `PATH`; and `apply` reports what moved instead of
reprinting the files.

It also adds the four things a first-time user reached for and did not find:
a **troubleshooting page and an FAQ**, **shell completions**, `doctor
--probe` to prove a rendered server actually starts, and a **tiered adapter
support matrix** that says which of the thirteen adapters are exercised by
CI and which are best-effort. The CLI is now stated plainly as the primary
surface and the launch channel, the Homebrew formula is generated from the
release's own checksums rather than checked in and stale, and `agentstack
self update` gives the binary an upgrade path.

Security: `doctor --live` no longer contacts an untrusted repository's
servers, and an independent line-by-line review of the consent/grant path
and the workflow interpreter closed eleven findings. **Consent digest v3
means every trusted project reads as `Changed` once after upgrading —
re-review with `agentstack trust`.**

### Security

- **`doctor --live` no longer contacts a repository's servers before you have
  trusted it.** The flag performs live MCP handshakes against every HTTP server
  the manifest declares — resolving that server's `${REF}` headers to do it. On
  a repository you had cloned but not reviewed, that meant the repo chose the
  destination and AgentStack supplied the credentials. It now refuses on an
  untrusted or drifted project, exactly as `session start`, the gateway and
  `doctor --probe` already do. A trusted project probes as before.

An independent line-by-line review (2026-07-23) of the consent/grant path and
the workflow interpreter's ambient capabilities produced nine consent-path
findings and two interpreter findings. All are closed:

- **Interactive grants bind to the reviewed bytes.** The whole review renders
  from one immutable snapshot and the grant records that snapshot's digest —
  a manifest swapped mid-review is no longer silently blessed; the project
  reads Changed and every use site fails closed.
- **The trust preview shows only pinned library content.** A library-backed
  server whose live definition does not match the lockfile pin renders as
  unverified instead of displaying content the consent digest does not cover.
- **`apply`'s owned-manifest refresh re-pin** now digests the exact bytes it
  wrote (never a disk re-read), only ever updates an existing entry, and
  preserves the recorded review surface.
- **Consent digest v3** distinguishes absent pinned files from present empty
  ones. **Existing trust entries re-gate: each trusted project reads as
  Changed once after upgrading — re-review with `agentstack trust`.**
- **Trust-store writes are serialized across processes**, so a concurrent
  grant can no longer resurrect an entry a racing revoke removed.
- **Init plan digest v2** covers the complete import — full server objects
  (env, cwd, argv boundaries), settings values, and the secret destination —
  and a consented `init` writes exactly the detection it verified, closing a
  verify-then-redetect window. Plan digests from older builds no longer
  match; re-run `init --plan`.
- **Review output sanitizes** policy, blocker, and secret-reference text
  (terminal control sequences from hostile manifests).
- **The `isatty` consent probe's limits are documented honestly** in
  `docs/ENFORCEMENT.md`: a same-user PTY wrapper reads as interactive, which
  is exactly as strong as the same-user-writable store file — the enforced
  claim is that headless callers need `--yes --consented-digest`.
- **Workflow scripts run in UTC** (Boa's default host hook leaked the OS
  timezone through explicit-argument `Date` methods) and **`WeakRef` /
  `FinalizationRegistry` are poisoned** (GC-schedule nondeterminism that the
  resume journal could not replay).

### Added

- **A tiered adapter support matrix, published.** Thirteen adapters shipped
  under one word — "supported" — while only five of them are checked nightly
  against the real upstream CLI and the rest are pinned by a render snapshot
  that only compares our output to our own expectation. The new
  [adapter support matrix](docs/adapters.md) states per adapter which of the two
  checks it gets (tier 1 nightly-verified, tier 2 best-effort, tier 3
  community-reported and currently empty), reads each adapter's real
  capabilities out of its descriptor — servers, project scope, skills,
  instructions, hooks, settings, extensions, headless run — and says plainly
  what happens when a vendor changes its config schema: tier 1 goes red within
  a day, tier 2 you notice first. It also explains why conformance is nightly
  rather than per-pull-request, and that a descriptor dropped into
  `~/.agentstack/adapters/` is tier 3 by definition. Same claim discipline the
  enforcement matrix applies to security, applied to compatibility.
- **`--json` on the last four reads that lacked it, and one page that lists
  every one of them.** `agentstack status`, `search`, `adapters list`, and
  `session list` now speak JSON, so driving the CLI from an agent or a script
  no longer needs a screen-scraping fallback for part of the surface. Each body
  carries the same versioned envelope every other machine-readable read does
  (`schema_version` + `features`), and each is the same reading the human screen
  renders — nothing writes, renders, spawns, or resolves a secret because the
  flag is present. The new contract name is `json-reads-v1`: check for it in
  `features` rather than probing, because a binary that predates it refuses
  `--json` outright instead of degrading. `status --json` is the biggest of the
  four — detected CLIs, manifest state, toolsets, active session, lock, trust,
  delivery mode, unresolved secret *names*, and the single `next_action` — and
  it distinguishes "no manifest" from "manifest that will not load" with a
  `project: null` plus a reason, rather than making a caller guess.

  The new [Automation contract](https://tarekkharsa.github.io/agentstack/automation.html)
  page is the single place an integrator looks: every JSON-capable command, its
  contract name, its payload shape, how errors are reported (empty stdout,
  non-zero exit), and which JSON output is deliberately *not* part of the
  contract.

- **`agentstack doctor --probe` — proof your servers actually run.** Until now
  `doctor` could tell you your config parses, your secrets resolve, and your
  digests match; it could not tell you a server *starts*. It even warned that a
  bare `npx` launcher may fail to spawn under a GUI-launched harness and then
  offered no way to find out. `--probe` starts each stdio server exactly as a
  rendered config would — same args, `env`, and `cwd` — speaks the MCP
  `initialize` handshake, counts its tools, and stops it again, reporting per
  server: started (with startup time and tool count), did not start, exited
  before the handshake, hung past the deadline, or not-probeable. The HTTP
  counterpart is the existing `--live`.

  It is the one `doctor` flag with side effects, so it is bounded on every
  side: opt-in only; refused outright for a project that is not trusted at its
  current bytes, on the same rule as `session start`; ten seconds per server as
  a hard wall covering spawn, handshake, and tool count; the child killed with
  its whole process group (so a launcher's real server process goes too) and
  reaped on every exit path, including Ctrl-C; a server whose `${REF}` does not
  resolve reported as not-probeable rather than started with a half-substituted
  environment; and the child's stdout and stderr treated as hostile — bounded
  and stripped of escape sequences before anything is printed. `doctor --json`
  carries the results under a top-level `probe` object, behind the new
  `doctor-probe-v1` feature name.
- **Shell completions**: `agentstack completions bash|zsh|fish` prints a
  completion script on stdout, with installation for each shell documented in
  [the reference](https://tarekkharsa.github.io/agentstack/reference.html#shell-completions-agentstack-completions).
  It is generated by walking the CLI's own command tree, so it covers every
  command — including the ones `--help` groups away — every nested subcommand,
  and every long flag, and cannot drift from the binary that produced it.
  Values (toolset names, paths, harness ids) are deliberately left to the
  shell's own file completion.

- **`--version` says which build you have**: the sandbox backend is a
  compile-time option, so a release binary and a plain `cargo build --release`
  used to be indistinguishable while having different capabilities. Now
  `agentstack --version` prints `agentstack 0.16.0 (sandbox: yes|no)`,
  `agentstack doctor` repeats it in **Adapters & CLIs**, and `run --sandbox` on
  a build without it names the real cause — compiled without the feature —
  instead of reading like a Docker problem.

- **A troubleshooting page and an FAQ**, the two pages the docs set was
  missing. [Troubleshooting](https://tarekkharsa.github.io/agentstack/troubleshooting.html)
  is organised by what the user experiences — "my CLI doesn't see the servers",
  "a secret won't resolve", "it says my files drifted", "it refuses because the
  project isn't trusted", "I want to undo something", "a server won't start" —
  and quotes each message verbatim from the binary so the text pasted into a
  search box is the text that finds the fix, paired with the exact repair
  command `doctor` would have printed after its `↳`. The
  [FAQ](https://tarekkharsa.github.io/agentstack/faq.html) answers the fifteen
  questions a first-week user actually asks (will this overwrite my configs, is
  my API key in the manifest, do I have to commit it, can my teammate use a
  different CLI, do I need Docker, what happens if I uninstall). Both are
  reachable from the sidebar, the docs index, and the README.
- **`agentstack self update` — an upgrade path (C4b).** Until now a binary you
  installed stayed at the version you installed, and nothing ever told you a
  newer one existed. `agentstack self update` previews what the newest release
  would install and `--write` installs it. **The archive is verified against
  the `checksums.txt` published with the release before it is unpacked or moved
  into place**, then swapped in with an atomic rename; a mismatch aborts with
  both digests and leaves your existing binary byte-for-byte untouched. Stated
  honestly: archive and checksums share one TLS-authenticated origin, so that
  proves the integrity of the transfer, not the provenance of the release — the
  command names `gh attestation verify` for provenance rather than claiming it.
  The three cases it cannot fix are detected *before anything is downloaded*
  and answered with a command that works: Homebrew (`brew upgrade`), an
  unwritable directory (`sudo`), and a platform with no published asset. A
  source build is refused and pointed at `cargo build --release`.
- **`agentstack doctor` says when a newer release exists.** One line, as a
  **note** — it counts in `advisories`, never in `errors`/`warnings`, so it
  cannot move `state` off `ready` or become the "start with" action. At most
  one short bounded request per 24 hours, cached in
  `~/.agentstack/update-check.json`, silent when offline, and backing off for a
  full day after a failed check rather than re-dialling. Opt out of every
  release-channel request with `AGENTSTACK_NO_UPDATE_CHECK=1`.
- **`doctor-advisories-v1`** joins the negotiated `features` list: `doctor
  --json` carries an `advisories` count and section lines can carry
  `level: "advisory"`. A panel that does not know the name renders advisories
  as `ok` — safe, but silent — so the name is what lets it show the count
  instead of dropping it. t3code surfaces them as a muted "· 2 notes" beside
  the status chip.
- **The approved workflow blueprint is pinned with the script (F13).**
  `[workflows.<n>].blueprint` names the reviewed graph; `lock` pins it beside
  the script, so one consent covers both and editing either re-gates. The trust
  review renders the approved pattern and each node's role/model/effort right
  where execution is authorized, and **admission verifies the pin** — a graph
  swapped after consent refuses the run. Stated honestly, in the review itself:
  the two artifacts are bound, but nothing verifies the script implements the
  graph.
- **`agentstack workflow declare` — authoring as one transaction (F14).**
  Stages the script and blueprint, adds the manifest entry, validates, and
  re-locks; on any failure it rolls all of it back and names the step that
  failed. Previously six independent writes, where a failure partway left a
  half-declared workflow behind a button labelled "Approve". Previews by
  default; a successful declare is one `agentstack restore` entry. It
  deliberately does not trust or run — consent stays the human's step.
- **`agentstack uninstall` — the whole way out.** Reverts every managed region
  agentstack rendered (servers, settings, hooks, instruction blocks) in every
  CLI's own config, then removes `~/.agentstack`. Previews by default;
  `--write` acts, `--verbose` shows each diff, `--keep-home` keeps the undo
  ledger, `--scope` limits it. Removal runs through the same planners `apply`
  uses, given an empty manifest, so it takes exactly what agentstack manages
  and leaves foreign entries — and another project's global entries —
  untouched. Your `agentstack.toml` is never touched, and because each edit is
  captured first, **the uninstall is itself undoable** with
  `agentstack restore --last --write`.
- **`create-profile` is usable by hand.** At a terminal it now shows a plain
  review and asks, instead of requiring the panel's
  `--yes --consented <digest>` round trip — naming a toolset was previously
  reachable only by editing TOML. The authority contract is unchanged: no
  caller writes without presenting the reviewed digest, and `--preview` forces
  the enveloped shape at a terminal too. (See *Changed* for what a bare
  non-interactive call does now, and for create no longer rendering.)
- **Advisory findings in `doctor`.** Findings that are true but carry no action
  for this project — a server launched through bare `npx`, for instance — are
  now counted separately from warnings, reported as notes, and excluded from
  both the project's `state` and its recommended next action. A healthy setup
  reads `ready` instead of sitting permanently on `needs_attention`.
  `doctor --json` gains an `advisories` count.
- **Versioned UI contract.** Every machine-readable read for external panels
  (`init --plan`, `trust --preview`, `doctor --json`, `use --list --json`,
  `diff --json`, `restore --json`) now carries `schema_version` and a
  `features` list naming usable end-to-end contracts, so a UI negotiates
  instead of guessing and mismatched CLI/UI pairs fail closed with upgrade
  guidance.
- **Consent-bound setup.** `init --plan` emits a `plan_digest` identifying
  the exact reviewed import plan; `init --yes --consented-plan <digest>`
  refuses to write when detection no longer produces that plan — the same
  reviewed-bytes binding trust grants already enforce.
- **Complete trust preview.** `trust --preview` lists the full named surface
  (skills, workflows with roles, extensions with targets, instructions), not
  just counts, so an external consent screen shows everything the
  interactive review shows.
- **Status contract.** `doctor --json` now leads with one `state`
  (`needs_setup` / `needs_attention` / `ready`), one recommended
  `next_action`, and factual `protection` booleans; an uninitialized
  directory is a state, not an error (outside `--ci`).
- **Machine-readable undo.** `restore --json` lists the undo ledger with
  per-project attribution (`touches_project`) and id-addressed
  preview/result output, so a project-scoped Undo reverts its own newest
  write — never another project's — while `restore --last` keeps its
  machine-wide meaning.
- **Library removals are recoverable.** `lib remove*` now moves the entry to a
  machine-local `lib/.trash/` instead of deleting it. `lib trash` lists what is
  there, `lib trash --restore <id>` puts an entry back (refusing to overwrite a
  same-named capability unless you pass `--replace`), and `lib trash --empty`
  discards it for good. The t3code Remove action is bound to the same path.
- Witnesses: end-to-end preview→edit→apply race tests for both consent
  bindings, and a parity test proving the t3code panel's fixed argv and the
  direct CLI journey produce byte-identical files.
- **The landing page leads with the product.** The hero's right half was a
  decorative logo tile and the install command was two screens down; the
  recorded first-value run and the `curl` line now sit above the fold
  together. This is a tool whose value is visible — two configs converging, a
  clean `doctor`, a byte-perfect restore — and the recording that shows it was
  buried below the fold.
- **A pinned toolchain (`rust-toolchain.toml`) and an MSRV job.** `fmt` and
  `clippy` output moves between compiler releases, so a floating `stable` made
  "the tree is formatted" a claim about whichever compiler ran that week.
- **Supply-chain gates.** `deny.toml` plus a weekly `cargo deny` run
  (advisories, licences, sources, banned crates) and a CycloneDX SBOM. An
  advisory filed against a dependency that was fine when it was added is not
  something a merge gate alone can catch.
- **Bug reports can attach `doctor --json`.** It carries the detected CLIs,
  every check and verdict, drift, trust state and the build's feature set — most
  of what triage would otherwise ask for one question at a time.

### Fixed

- **The declared minimum Rust version was wrong.** `rust-version` said
  `1.80`; the workspace has not built on it for some time — `boa_engine`
  requires 1.88 and several transitive crates need an edition-2024 Cargo, so
  the resolver fails outright. It now says `1.88`, and CI's new `msrv` job
  checks the claim on every push rather than trusting it. A build claim
  nobody had verified is the same defect class the enforcement docs exist to
  prevent, pointed at the build instead of at security.
- **Two advisories cleared out of the lockfile**: `crossbeam-epoch` 0.9.18 →
  0.9.20 (RUSTSEC-2026-0204, invalid pointer dereference; reached only through
  dev-dependencies, so no shipped binary contained it) and `anyhow` 1.0.102 →
  1.0.104 (RUSTSEC-2026-0190, unsoundness in `Error::downcast_mut`).
- **Five clippy lints a warm build cache had been hiding.** Incremental clippy
  reused cached results for the CLI crate, so lints a toolchain update
  introduced never re-ran locally and would have failed CI on a cold checkout.
  Bumping every crate version forced the re-analysis that found them
  (`unnecessary_map_or` ×3, `manual_is_multiple_of`, `manual_repeat_n`). All
  are mechanical rewrites with no behavior change.

- **The library trash can no longer be tricked into serving content from
  outside the library.** F22's first fix guarded the destination — a plain
  name, a resolved-path check, a symlinked target — but not the source.
  `restore` moves the body with `rename`, which relocates a symlink *itself*
  rather than what it points at, so a crafted `.trash/<id>/body` symlink was
  renamed into the live library and the library then served whatever it pointed
  at. The expected-name allowlist did not catch it (`body` is the expected
  name) and `exists()` follows links, so the incomplete-entry check passed too.
  Every component from the trash root down to the body is now checked with
  `symlink_metadata`, which never follows — so a symlinked *entry directory*
  is refused for the same reason.
- **A failed restore rollback now says so.** Both rollback moves were
  best-effort and their errors discarded, while the message still claimed the
  entry was "left in the trash, unchanged". A rollback that does not complete
  now reports which paths still hold the bytes and states that nothing was
  deleted.
- **Restore refuses to overwrite a leftover displaced copy.** `--replace` sets
  the live entry aside in `replaced/` before moving the trashed one in. A
  `replaced/` already present is the residue of an *earlier* restore whose
  rollback did not finish — possibly the only surviving copy of the live
  entry — and it was deleted to free the name. It is now a refusal that names
  the recovery.
- **A failed `workflows/` creation no longer strands an undo entry.**
  `workflow declare` records its undo entry before writing anything, but the
  directory creation sat outside the transaction, so its failure returned early
  and left a durable no-op at the head of the ledger — `restore --last` would
  offer to undo a declaration that never happened, shadowing the user's real
  last change. It runs inside the transaction now, and takes the same
  rollback-and-discard path as every other step.
- **The project `.env` is no longer world-readable.** It holds real token
  values, but was written at the ambient umask — `0644` on a normal machine —
  so every local account could read it. It is now created `0600` before any
  bytes are written, a write tightens a file an older version left permissive,
  and pre-write backups of it are hardened the same way. `doctor` warns about
  any `.env` still readable by others and names the exact `chmod`. **If you
  used `--secrets env` on an earlier build, run `agentstack doctor` — it will
  tell you which file to fix, and the next `secret set --env-file` repairs it
  automatically.**
- **The `.gitignore` rule AgentStack writes is scoped to its own file.** It was
  a bare `.env`, which git matches at every depth — so it silently ignored the
  project's own env files too. It is now anchored (`/.agentstack/.env`, or
  `/.env` for a legacy root manifest), and still lives outside the managed
  block so a re-render cannot drop it. Existing hand-written `.env` /
  `/.env` / `**/.env` rules are still honoured, so nothing is added twice.
- **Apply and activation summaries no longer say "0 target(s)" after
  succeeding.** They counted only *changed* targets, so an idempotent re-apply
  printed `Applied to 0 target(s).` under four `✓ up to date` lines, and
  `session start` printed `activated 'x' on 0 target(s).` directly above the
  list of four files it manages. Both now report coverage first:
  `4 target(s) in sync — wrote 4.` / `4 target(s) already in sync — nothing to
  change.`
- **Undo pointers name a command that undoes.** `apply` and `use` suggested
  bare `agentstack restore`, which *lists* the ledger; they now print
  `agentstack restore --last --write`.
- **`status` stopped recommending `doctor` twice** in six lines, once as the
  next step and again as the deep check. The second pointer now appears only
  when the next step is something else.
- **`trust` describes what it actually gates.** Its summary and banner said
  "for the zero-files gateway", but `session start` and every other activation
  path refuse on an untrusted project too — so the refusal read as a bug. It is
  now "review and approve this project's declared capabilities — required
  before anything activates them".
- **Status paths no longer prompt for keychain values.** On macOS, `doctor`,
  secret provenance, MCP status, and the t3code snapshot ask the keychain only
  whether an entry *exists* rather than decrypting it. Read-only status stopped
  triggering a system authorization prompt on every check after a local
  rebuild.
- **`restore`'s listing says what each undo entry actually was.** Three
  recorded changes with the same scope and file count used to print as
  identical rows — nothing distinguished an `init` from a `session start` from
  a plain `apply`. Every entry now records the command that produced it
  (`session start 'backend'`, `apply (profile 'x')`, `workflow declare
  'name'`, …) and the listing renders it. Two `init`/setup call sites were
  also feeding the ledger raw adapter ids (`claude-code`) instead of display
  names (`Claude Code`) — the same rows now read consistently. Entries
  recorded by a build before this change render as `unlabeled change
  (recorded before undo entries named their operation)` rather than a blank
  column or a panic. `agentstack restore --list` is now an explicit alias for
  the bare listing.
- **`agentstack diff` labels foreign and hand-edited entries instead of
  showing them as unlabeled context.** A hand-added server (or one left by
  another agentstack manifest) sat in the printed diff with no indication
  AgentStack doesn't own it and won't remove it. Each target's report now
  tags `· managed: <names>` for what it renders from the manifest, `foreign
  (kept)` for entries it preserves but does not own — split into "applied by
  another manifest" (adopt/`--prune-foreign` eligible) and "not agentstack's"
  (never added by us, so no prune path exists for it) — and `hand-edited`
  when the file changed outside agentstack since the last write, with a
  one-line legend. `doctor`'s matching drift line no longer asserts a
  hand-edit as the cause (the same state can arise from a session ending onto
  a stale baseline); it now states the observed fact — "no longer matches
  what agentstack last wrote" — and offers `diff` to review and `adopt` to
  accept the on-disk version.

### Changed

- **A consented `init` records trust for the manifest it just wrote.** The trust
  gate used to first meet a user as a hard stop in their own repo, on the
  command the docs call the beginner way to switch toolsets, with no wrongdoing
  on their part. It is safe to grant here because of what `init` actually does:
  it imports only from your machine-global CLI configs (`~/.claude.json`,
  `~/.codex/config.toml`, …) — never a repo-supplied project file — it imports
  servers and settings and no skills, workflows, extensions, or instructions,
  and it refuses outright when a manifest already exists. So the manifest is
  built from configuration you already had, and the import review you consented
  to is its whole surface. The gate still fires for the case it exists for: a
  cloned repo carrying a manifest agentstack did not write. The grant binds the
  bytes `init` wrote, never a disk re-read, and it is declined entirely when an
  `agentstack.lock` or `agentstack.local.toml` was already present — content the
  import never showed you. Nothing is printed about it: the ordinary journey
  stays free of trust vocabulary until you reach for it. `agentstack doctor`
  reports the state and `agentstack trust --revoke` withdraws it.
- **`doctor` stops showing the zero-files gateway section to first-timers.**
  That section appeared once a project "entered the trust lifecycle", which was
  a fair proxy for an advanced user back when trust was always granted by hand.
  With `init` now granting it, the proxy pointed at every newcomer. It appears
  when a CLI is actually connected, and under `--all`.
- **`init` targets the CLIs that actually gave it something.** Every detected
  binary used to become a target, so `apply --write` created a
  `.gemini/settings.json` and an `opencode.json` in the repo of someone who has
  never opened either tool — unexplained files in your project, and diff noise
  for every operation after. `[targets] default` is now the CLIs that
  contributed configuration; the rest are named in the summary as "also seen",
  with why they were left out and how to add one. A machine with agent CLIs but
  no config anywhere still targets everything detected, because an empty target
  list would render nothing and read as a broken import.
- **`apply` shows a summary, not a wall of file content.** A dry run dumped
  every rendered file in full and `--write` reprinted the identical text with
  `✓ wrote` swapped for `→ to apply` — roughly 100 lines of JSON and TOML for
  four targets, burying the facts that matter (which file, how much moved, how
  to undo) in content you cannot meaningfully check. Each target now reads
  `~ +28 / -0 lines (new content)`; `apply --verbose` prints the bodies. In one
  real two-target project the dry run went from 48 lines to 11.
- **One concept, one word: it is a toolset everywhere you read it.** The product
  said "profile" in every command and error, the docs said "toolset", and the
  manifest said `[profiles.*]` — three names for the same thing, so you read one
  word on the site and had to type another in the terminal. The CLI's output,
  help text, and docs prose now say **toolset** throughout, and naming one is a
  visible command: `agentstack toolset create --name backend --server github`.
  On the commands you can see, `--toolset` is the flag (`apply`, `run`, `add
  server`, `add skill`); the old `--profile` spelling keeps working as an alias,
  so nothing you already typed or scripted breaks. Deliberately unchanged: the
  manifest key stays `[profiles.<name>]` so every existing manifest keeps
  working, the JSON contract fields and `profiles-v1` / `profiles-edit-v1`
  feature names stay put, and the fixed panel argv (`create-profile`,
  `use-profile`, `add-*-to-profile`) still runs — `create-profile` is now a
  hidden alias of `toolset create` on the same authority path, producing the
  same consent digest. A CI lint holds the line on both surfaces.
- **Naming a toolset no longer switches to it.** `create-profile` used to write
  the manifest entry, re-lock, *and* render the new toolset into every CLI as a
  side effect of being named — so defining a subset silently changed your setup,
  and the later `session end` returned to that subset instead of your full
  manifest, which is what turned a clean `doctor` into five drift warnings for
  anyone who followed the documented path. It now writes the entry and re-locks,
  and renders nothing. Activate it when you actually want it:
  `agentstack session start <name>` (reversible) or `agentstack use <name>
  --write` (on disk). Panels gate on the new **`toolset-create-v2`** feature
  name; `profiles-edit-v1` keeps its old meaning, and the other three verbs it
  covers are unchanged.
- **A bare non-interactive `create-profile` refuses instead of printing a
  consent envelope.** Piped, in CI, or driven by an agent, it used to dump a
  JSON envelope with a `consent_digest` and a fourteen-element `features` array
  at whoever ran it. The two-step digest contract is right for machines and
  wrong as an answer to a person, so it now lives behind the flag a machine
  passes — `--preview` — and the bare call says, in a sentence, which flag it
  wants.
- **`init`'s closing summary stops recommending a toolset.** It taught
  `create-profile` as the step after `apply --write` + `doctor`; a first-time
  user with a handful of servers has nothing to subset yet, and the suggestion
  sent them into a second concept — and, before the change above, into an
  unannounced render — one command after the part that worked. The summary now
  ends at `apply --write` → `doctor`.
- **`--help` is one screen again.** It listed nine curated commands and then
  printed all ~40 names two lines below, undoing the curation on the same
  screen and making a config manager look like an enterprise platform. The
  task-grouped map moved to `agentstack --help --all`, which now leads with it;
  the default help keeps the Start-here block, the vocabulary note, and one
  pointer.
- **`workflow` left the everyday command list.** The interpreter-boundary
  review passed, so the `(preview)` label stays gone and the command is fully
  reachable — but workflows remain an advanced lane until the repeated-use gate
  closes, and their vocabulary (admission, ceilings, locked child runs) was the
  densest a first-time user met. The short summary is now outcome language; the
  precise version lives in `agentstack workflow --help`.
- AgentStack now positions portability, toolsets, lifecycle diagnosis, and
  recovery as the primary product value. Security remains the enforced
  foundation and appears progressively when an action makes it relevant.
- t3code is the primary graphical integration and launch direction. The CLI
  remains the complete standalone interface and the sole authority for plans,
  writes, consent, recovery, and enforcement.
- **`status` no longer recommends a command that refuses.** Its next step now
  keys off whether the setup is rendered rather than whether it is locked, so
  a finished first run is told to verify it, not to re-run `init` (which
  errors once a manifest exists). A project holding capabilities that are not
  on disk yet is pointed at `apply --write`.
- **`doctor`'s recommended action is ranked, not first-encountered.** A fix
  agentstack can run wins over a hand-off to another tool; section order only
  breaks ties within a class.
- **The bare-launcher quirk is stated once, with a count**, instead of once per
  server — nearly every published MCP server ships as `npx -y …`, so the
  per-server form scaled its noise with the size of a normal setup.
- **`apply --write` survives a broken pipe.** Piping it into `head` (or
  quitting `less` early) used to kill the process between two targets, leaving
  some CLIs rendered and the rest drifted. Output is now expendable; the write
  pass is not.
- **`init` says what it actually did with your tokens.** Lifted values are
  described as *copied* — the original stays in the CLI's own config — and the
  summary names the resulting duplication and the `--scope global` command that
  resolves it. It also closes by showing how to name a first toolset.
- **The first-toolset suggestion is a runnable command, not a manifest block.**
  `init` now prints `agentstack create-profile --name backend --server <one you
  just imported>` instead of a `[profiles.*]` snippet to paste. Hand-editing
  the table leaves the lockfile stale until the next `use --write`; the command
  performs the mutate → re-lock → re-render pipeline, so the printed line is
  both runnable and correct.
- **`lock` refuses a manifest that cannot validate**, instead of pinning it and
  recommending `trust .`. The lockfile is part of the consent surface, so an
  invalid manifest used to reach a consent prompt for a bundle that could never
  be admitted — the refusal only surfaced later at `workflow run`. Same issue
  set, messages, and fixes `doctor` and `apply` already produce.
- **A Codex config the user will never open is a note, not a warning.** Codex
  is detected by binary-on-PATH and lands in `targets.default`, so AgentStack
  renders `.codex/config.toml` for projects that never mentioned Codex — and
  the "Codex will ignore this until you open it once" warning then pinned every
  such project at `needs_attention` on any machine with Codex installed. It
  stays a warning when the user has accepted Codex's own trust prompt for some
  project (they really run it); otherwise it is an advisory.
- **`uninstall` removes the `.gitignore` managed block** it wrote, instead of
  leaving a block naming generated files it just deleted. The separate
  `# agentstack: local secrets` rule stays, because the `.env` it protects
  stays.
- **`uninstall` names what it kept.** `agentstack.toml` and its `.env` survive
  by design — this removes rendered output, not your setup — but "AgentStack is
  uninstalled" reads as "nothing of mine is left", so the summary now says so
  and reports how many secret values the kept `.env` holds.
- **User-facing text says "toolset" (F17).** `use`, `session`, `lock`, and
  `kill` describe toolsets in their help; `use --list` says "Declared toolsets"
  and, when there are none, offers the command that names the first one. The
  hand-authored site pages now match (H2) — the landing page led with
  "Profiles select a task-specific toolset", three names for one thing on the
  first page anyone sees, and `docs.html`, `cookbook.html`, `examples.html`,
  `start.html` and the tutorial carried the same split. A CI check
  (`authored_html_pages_say_toolset`) holds the line: it reads the compiled-page
  list out of `tools/make-docs-pages.py` and lints everything else under
  `docs/`, so a new authored page is covered the day it lands. The manifest
  table stays `[profiles.*]` and flags keep their names — those are API, and
  the panel's argv contract pins them.
- **A prefix-less consent digest is diagnosed as a format problem, not as
  changed content.** `trust --yes --consented-digest <bare hex>` still refuses
  — the acceptance rule is unchanged — but it no longer claims the manifest or
  lock changed and print two visibly identical hashes, which sent users to
  re-preview an unchanged project in a loop. A differently-labelled digest
  (`md5:<same hex>`) remains a genuine mismatch: the accepted alternative form
  is derived from the computed digest, never parsed out of caller input.
- The panel-only verbs (`add-skill-to-profile`, `add-server-to-profile`,
  `use-profile`, `library-index`) moved out of the human command map into a
  labelled integration-contract section of `--help --all`; `create-profile`
  moved into the everyday `Edit` group. Diagnostic output abbreviates `$HOME`
  and folds `..` segments.

### Removed

- **`agentstack setup`**, the hidden alias of interactive `init`, is gone — one
  wizard, one name. (The internal `SetupArgs` type `init` builds is unchanged.)
- The embedded `agentstack dashboard` command and its bundled web assets were
  retired to avoid maintaining a second UI. Reusable read-only snapshot data
  remains available to the MCP and t3code integration paths.
- Superseded audits, roadmap drafts, implementation memories, and the separate
  history ledger were removed. `STRATEGY.md`, `TODO.md`, and this changelog now
  have distinct, non-overlapping roles.

## v0.15.0 — 2026-07-21

**Skills become a governed supply chain.** This release closes the full
skills loop — find, try, install, author, update — on the ecosystem's own
conventions, hardens every remote-ingestion path it opens, and finishes the
docs restructure with a landing page that pitches instead of documenting.

### Added

- **`agentstack add skill <source>`** installs skills from any skills repo —
  `owner/repo`, a git URL, or a local directory — with one preview and one
  write. Sources resolve through the ecosystem's discovery conventions,
  land digest-pinned in the lockfile, and `--write` activates them
  mode-aware on the spot.
- **`agentstack try`** runs a skill without installing anything: stage,
  scan, and emit a wrapper prompt on stdout for piping into any agent CLI.
  **`agentstack lib new`** scaffolds a library skill, closing the authoring
  loop, and `lib add` speaks the same source grammar with staged previews.
- **`lock --update` actually updates**: branch pins refresh, upstream
  deletions are detected and said out loud, and every skip names its
  reason.
- **`finding-skills`** ships in the catalog — the skill that teaches an
  agent to acquire skills through the governed pipeline instead of around
  it.
- **Re-trusting diffs the consent surface**: `trust .` on a changed project
  shows exactly what changed since your last consent, and always names the
  enforcement ceiling.

### Security

Remote skill ingestion got a dedicated hardening round before the new
surface shipped:

- Every git invocation goes through one hardened spawner — profiled
  arguments, no shell, and a clock on every call.
- Display sanitizers strip terminal control from every remotely-sourced
  string, so a skill name or description can neither drive your terminal
  nor spoof agent context.
- One skill-name grammar from `pack.toml` to the filesystem; symlinks are
  rejected at every local boundary; offline reads honor the lock's pinned
  commit rather than whatever the working tree holds.

### Changed

- The developer-UX review closed out across four rounds: every error names
  its exact fix command, `status` addresses capabilities by name,
  `--help --all` prints the real full tree, keychain errors name both
  stores, `set server` is an idempotent upsert, and the `init` confirm gate
  is honest — plan first, one undo batch, rollback on failure, Ctrl-C
  recovery.
- The docs finished their restructure: one canonical home per concept, a
  concepts glossary and a choose page, a how-to layer, two-tier navigation
  — and a round-4 marketing pass that halves the landing page (17 → 10
  screens), moves its doc content to the pages that own it, and closes the
  last coverage gaps, so every command has a teaching surface beyond the
  reference.

### Fixed

- Lockdown evidence lines decode as whole UTF-8 instead of per-byte
  mojibake.
- The GitHub Action's pinned default installs this release's binary, and CI
  guards the stamp.
- The MCP `add_skill` tool pointed agents at `apply`, which never renders
  skills; it now names the verb that does.
- The example demo scripts caught up with `lib add`'s positional source,
  and the CI abort cascade no longer masks their assertion failures.

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
