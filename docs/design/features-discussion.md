# Features discussion

A running product-design discussion. Nothing here is scheduled work — items
move to `TODO.md` when we decide to build them. Each section records: what's
true today (verified), what we want instead, decisions already made, and what's
still open.

Started 2026-07-18, during the feature-by-feature walkthrough.

## Guiding principles (the north star)

1. **Onboarding is the most important surface of the product.** Setup/init is
   where a user decides agentstack is trustworthy or confusing. Every minute
   spent making the first run self-explanatory pays back more than any other
   feature work.
2. **Every choice explains itself.** Any prompt with options carries a short
   help line under *each* option describing what choosing it actually does —
   in plain words, at the moment of choice ("your tokens will be moved into
   the macOS keychain; view them in Keychain Access; change one later with
   `agentstack secret set`"). No option the user has to research first.
3. **The machine is the user's.** We are setting up their whole machine, so
   they must always be able to answer: *what did agentstack change, where,
   and how do I edit or undo it later?* Every setup that writes something
   ends with a summary of exactly that.
4. **Explain the why, not just the what.** Where the design is asymmetric on
   purpose (deny lists auto-seed, allow lists start empty), say so in one
   honest sentence at the moment the user meets it.

## First-run experience (init / setup)

### Investigated facts (2026-07-18, v0.12.0)

- `init` lifts inline tokens into `${REF}`s and stores values **only** in the
  OS keychain. `--no-keychain` silently **discards** the lifted values —
  they go nowhere, and the user must re-find each token and run
  `agentstack secret set` by hand. No command ever writes a `.env` file,
  though the resolver chain reads one as its last fallback.
- Plain project `init` never mentions guard. `init --global` seeds
  `[guard]` + the `[policy.filesystem]` deny list with a decent message, but
  nothing explains why deny is auto-seeded while `allow_roots` starts empty.
- Out-of-workspace guard denials name the mechanism (`[guard] allow_roots`)
  but not the exact TOML line or file to edit. (The `.env` denial does
  better — it names the machine manifest path.)
- The three delivery modes (static / clean-at-rest / zero-files) are never
  named together anywhere in CLI output. Two of the names each appear once,
  incidentally, inside unrelated `--help` text. No recommendation, no choice.
- `doctor` prints `resolved` without naming which backend resolved a secret;
  `setup` preflight and `secret list` both name the source.
- Init's "Detected 6 CLI(s)" means *binaries on PATH*, even when zero configs
  were found to import — reads like more than it is.

### P1 — The overview

Interactive `init`/`setup` opens with a short plan before doing anything:
detect CLIs → import existing configs → lift tokens to `${REF}`s → write the
manifest — and names what will NOT be touched. In `setup`, nothing is written
before the confirm step (already true; now it's *said*).

**Decided:** yes in principle. **Open:** exact wording; whether plain
non-interactive `init` prints a one-line version.

### P2 — Secret storage is a visible, explained choice

When init lifts tokens (and in `agentstack secret set`), the user chooses
where values live. Every option carries its help text:

- **Project `.env`** *(default)* — "Your tokens are written to `.env` next to
  the manifest, in plain text. agentstack keeps this file out of git and its
  guard blocks agents from reading it. Edit it with any editor."
- **macOS keychain** — "Your tokens are migrated into the system keychain
  (service `agentstack`). Nothing secret sits in a file. View or change them
  in Keychain Access, or with `agentstack secret set <NAME>`."
- **Skip / decide later** — "Only `${REF}` placeholders are written. Nothing
  runs until you provide values (env, varlock, keychain, or `.env`) —
  `agentstack doctor` lists what's missing."

Varlock is not a storage option (agentstack never writes to providers) but the
prompt mentions it: "already using 1Password or a secrets manager? Drop a
`.env.schema` in the project and refs resolve through varlock instead."

**Decided:** the default is the plain `.env` — it's what users already know,
and familiarity wins at onboarding. Keychain is the recommended-but-optional
step up, sold by its help text, not by being the default. (Recorded context:
keychain is the more secure store; the deny list + managed gitignore are what
make the `.env` default defensible. Revisit if external users ever include
teams with compliance needs.)
**Also decided:** `--no-keychain`'s silent value-drop is a bug to fix
regardless — lifted values must always land somewhere the user chose, or the
user must be told exactly what was not stored and why.
**Open:** flag names for the non-interactive path (e.g.
`--secrets env|keychain|skip`); whether `secret set` gets `--env-file`.

### P3 — Guard that teaches

- The seed moment (init --global / setup) explains the asymmetry in one
  sentence: "the deny list only ever *narrows* what agents can touch, so we
  seed it for you; `allow_roots` only ever *widens*, so it starts empty and
  only you add to it — a denial will show you exactly how."
- Every out-of-workspace denial prints the exact fix inline:
  the TOML line (`allow_roots = ["<parent-dir>"]`) and the file to edit
  (`~/.agentstack/agentstack.toml`), matching what the `.env` denial already
  does.

**Decided:** yes. **Open:** whether project `init` should mention guard at
all, or leave it to `setup`/`init --global` (current lean: setup only — a
project init shouldn't advertise machine-level machinery).

### P4 — The modes are a visible choice

At the end of `setup` (after the first apply has succeeded — the user has
seen it work), present the three modes as an explicit choice, each with its
help text:

- **Static** — "Rendered configs stay on disk, kept out of git. Works with
  every CLI, zero moving parts. This is what you have now."
- **Clean-at-rest** — "Nothing generated exists between sessions;
  `agentstack session start` materializes your profile and `session end`
  reverts it. Your repo stays pristine for git."
- **Zero-files** — "Nothing is ever written; the gateway serves servers and
  skills live over MCP, trust-gated per repo. Best when you work across many
  repos."

Choosing one prints the exact command it maps to (or applies it, if setup can).
Bare `agentstack` orientation shows the project's *current* mode, so the
answer to "which mode am I in?" is never archaeology.

**Decided:** visible choice with help text — not a silent default, not buried
in docs. **Open:** whether the choice appears in every setup or only when a
repo context makes clean-at-rest/zero-files relevant; whether the
recommendation adapts (repo with git → suggest clean-at-rest; many trusted
repos → mention zero-files).

### P5 — Doctor names the secret source

`doctor` reports `resolved from keychain` / `varlock` / `env` / `.env`,
matching `setup` and `secret list`. One-line parity fix.

**Decided:** yes.

### P6 — Truthful detection wording

"6 CLI binaries on PATH · 1 config imported" instead of "Detected 6 CLI(s)".
The import count is the achievement; the PATH scan is just context.

**Decided:** yes.

### P7 — "What happened to this machine" (the transparency close)

Every setup path that writes ends with a machine-change summary:

- every file written or modified, with its path
- where secrets went (which store, which names)
- what got seeded (guard/deny) and where to edit it
- the one-liner to undo (`agentstack restore --last --write`) and to
  re-inspect any time (`agentstack doctor`, bare `agentstack`)

This is principle 3 made concrete: the user should never wonder what a
"machine setup" tool just did to their machine.

**Decided:** yes in principle. **Open:** exact format; whether the summary is
also persisted (e.g. `doctor --last-setup` or a pointer to the history log
that `restore` already reads).

## Doctor + drift (feature 2)

### Investigated facts

- The content scan runs at capability entry (`lib add`, `add from`,
  `install` — flagged content blocks unless `--allow-flagged`) and on demand
  (`doctor --deep`); `doctor --ci` always includes it. `init` does not scan —
  correctly, since init imports server definitions, not skill bodies.
- `--deep` is discoverable only via help text; nothing suggests it at the
  natural moment (right after the first third-party skill lands).
- `doctor --ci` is the team/CI surface (what the GitHub Action runs); solo
  users have little direct use for it. Fine — but onboarding shouldn't
  advertise it to them.

### P8 — Teach `--deep` at the right moment

Three moments, covering discovery without ever offering a do-nothing step:

1. **Setup's closing doctor step asks** (maintainer proposal, 2026-07-18):
   when the project has skills, the wizard offers the deep scan as an
   explicit yes/no with help text underneath ("reads every skill and
   instruction body for hidden Unicode and prompt-injection tricks; slow on
   big libraries; re-run anytime with `agentstack doctor --deep`"). When
   there are zero skills, setup does not ask — no empty questions. Init
   itself stays promptless (it's the scriptable primitive; at init time
   there is no content to scan yet).
2. **Capability entry announces**: after an add/install brings in
   third-party skill content, print one line — "content scanned — re-scan
   anytime with `agentstack doctor --deep`".
3. **Doctor's summary** can note when the last deep scan ran, so "never
   scanned" is visible instead of silent.

**Decided:** 1 and 2 yes. **Open:** whether doctor tracks last-deep-scan
time (3) or stays stateless.

### P9 — Lock-drift must explain itself

Rule to surface: **lock drift is an error until you re-lock, and re-locking
re-gates trust** (new pins = new consent). Users must meet this rule stated
plainly, not discover it as a mysterious red X:

- The doctor error for lock drift explains in one line why it's an error and
  what re-locking implies for trust.
- The modes/setup material (P4) mentions it wherever the lockfile is
  introduced: "the lock is the consent anchor — editing it is a re-decision."

**Decided:** yes. **Open:** whether `agentstack lock` itself should print
"this will require re-trusting" *before* writing when the project is
currently trusted (lean: yes — consent-affecting actions announce themselves).

### P10 — Doctor's drift check must agree with diff (found live, 2026-07-18)

Observed on the maintainer's machine: `agentstack diff --scope global` reports
all targets in sync while `doctor` still warns "edited on disk since last
apply" for Claude Code and Codex. Cause: those native configs double as live
state stores (`~/.claude.json` is rewritten continuously by running
sessions), and doctor's edited-on-disk signal compares the whole file against
the last-apply record, while diff compares only the managed content. Result:
a permanently flapping warning on any machine where the CLI is actually in
use — which trains users to ignore doctor.

Fix direction: doctor should use the same managed-content comparison diff
uses (or auto-clear the warning when diff reports in-sync), and reserve
edited-on-disk for changes inside the managed region.

**Decided:** bug, fix it. **Open:** none — the comparison logic exists in
diff already.

## Guard (feature 3)

### P11 — Disclose the defaults and their source files

Users must be able to answer "what is blocked, and where is that written?"
without reading source code:

- **Install/seed moment**: `guard install` (and setup's guard step) prints
  the default deny list it is activating and names the file it lives in —
  "these rules are TOML in `~/.agentstack/agentstack.toml`; each project's
  `.agentstack/agentstack.toml` can ADD entries, never remove them."
- **Status**: `guard status` already lists deny globs and allow_roots — add
  the source file next to each layer (machine vs project) so the layering is
  visible.
- **Denials**: the deny-glob denial already cites its source file; the
  destructive-command and workspace denials should too (destructive patterns
  are built-in — say so: "built-in rule; deny/allow lists: <file>").

**Decided:** yes. Complements P3 (denials teach the exact allow_roots fix).

### P13 — Hook-conformance fixes (audited vs official docs, 2026-07-18)

Audit of all 9 hook integrations against each CLI's current official docs:
6 fully correct (Claude Code, Gemini, Copilot CLI, OpenCode, Pi; Codex
protocol-correct). Fix batch, severity order:

1. **VS Code write-gate gap (security)**: `WRITERS` in guard.rs lacks
   `replace_string_in_file`, `multi_replace_string_in_file`, `apply_patch` —
   VS Code in-place edits classify as reads, so workspace confinement never
   runs for them (deny-globs still fire). Add the three names + a test.
2. **Codex dual-write**: `guard install` writes `~/.codex/hooks.json` while
   the manifest hook-renderer writes inline `[hooks]` in config.toml; Codex
   loads both → guard fires twice. Pick one seam.
3. Cursor: adopt documented `beforeReadFile` + `beforeMCPExecution` blocking
   hooks (we currently gate shell only — no read/MCP gating on Cursor).
4. Windsurf: adopt `pre_mcp_tool_use` (same exit-2 mechanism, mechanical).
   Note: Windsurf docs moved to docs.devin.ai (Cognition) — track.
5. Codex deny: switch to the preferred stdout-JSON decision envelope (same
   shape as Claude's); delete the stale "rejects unknown JSON" comment.
6. Docs: VS Code hook support is Preview + org-disableable — say so in
   README/ENFORCEMENT instead of asserting settled coverage.
7. Cosmetic: trim off-schema fields from Copilot/Cursor deny payloads.

Optional design thread: Claude's `permissionDecision:"ask"` (soften some
denials to interactive approval) and `updatedInput` — same shape OpenClaw's
hook offers (P12); if guard's protocol grows richer, grow it once for both.

**Decided (2026-07-18): GREEN-LIT — "fix the guard batch" is the next
implementation session.** Items 1-7 land together, full gates, and the diff
gets the security-sensitive flag (it touches guard enforcement) for
line-by-line review. OpenClaw (P12) stays queued behind it. **Open:** the
richer-protocol design thread (answer once, for Claude `ask`/`updatedInput`
and OpenClaw's param-rewriting hook together).

## New harness coverage (researched 2026-07-18)

### P12 — OpenClaw adapter (+ Cline as second)

Research verdict: **OpenClaw** is the top candidate for adapter #14 —
massive adoption (~350k+ stars in five months), MCP servers in
`~/.openclaw/openclaw.json` (`mcp.servers.<name>`, JSON5), skills read from
`~/.agents/skills` (the same convention our Codex adapter already renders —
partial support exists today for free), a real blocking `before_tool_call`
plugin hook (guard coverage via the OpenCode/Pi native-plugin-file pattern),
and a security posture aligned with ours (tool policy, sandbox-by-default
for non-main sessions, "third-party skills are untrusted" in its own docs).

Open items before green-light (verify against a real install): (1) JSON5
read/write — a new adapter-engine format capability, not just a descriptor;
(2) the configurable workspace root (AGENTS.md/skills paths depend on it);
(3) whether any project-scoped config exists or it's global-only; (4) its
hook can *rewrite* params, richer than guard's allow/deny — decide if the
protocol grows. Churn risk is real (three renames in three months) — budget
re-verification.

**Second**: Cline (4–5M installs, PreToolUse hooks since v3.36, nested
VS-Code globalStorage config path). **Not worth building**: Aider (no gate),
Zed/Warp (category already covered), Roo Code (shut down 2026-05, archived —
recorded so it isn't proposed again). **Flag if ever adapted**: Amp syncs
threads to Sourcegraph servers by default — a pre-existing egress path our
audit story should name.

**Decided:** nothing yet — awaiting the hook-conformance audit of the
existing 9 before adding a 10th hook surface.

### P12a — Hermes Agent (researched 2026-07-18)

Hermes Agent (Nous Research, github.com/NousResearch/hermes-agent, MIT,
~216k stars, v0.18.2 Jul 2026, Python local CLI + gateway) is a second
adapter candidate *alongside* OpenClaw, not a fork of it. Global-only config
at `~/.hermes/config.yaml` with an `mcp_servers:` YAML key (stdio + HTTP,
OAuth/mTLS fields); SKILL.md skills in `~/.hermes/skills/` plus an opt-in
`skills.external_dirs` that can point at the shared `~/.agents/skills`
convention our Codex adapter already renders; and a genuine blocking
`pre_tool_call` hook (Python plugin or shell script, JSON stdin/stdout,
`{"action":"block"}`), plugins default-deny. Ships `hermes claw migrate`
(OpenClaw migration).

Verdict: merits an adapter now. OpenClaw-first still pays — the engine
primitives (plugin-file hook pattern, shared-skills-dir wiring) generalize —
but the descriptor is new work, not copy-paste. Open items: confirm no
project-scoped config exists (inspect `hermes_cli/mcp_config.py`); no
AGENTS.md-analog found for instructions delivery; `${VAR}` interpolation in
mcp env unconfirmed.

**Status: DRAFT — awaiting maintainer review.**

## Trust gate + gateway (feature 4)

### Investigated facts (2026-07-18, v0.12.0)

- `agentstack trust .` has no `--dry-run` — the grant *is* the review, and it
  is rich. Run against a scratch two-server manifest:

  ```
  Trusting /private/tmp/…/proj for the zero-files bridge.

  This project declares — review what auto-mode may run/contact:
    ▶ github: runs `npx -y @modelcontextprotocol/server-github`
    ▶ filesystem: runs `npx -y @modelcontextprotocol/server-filesystem /tmp`
    secrets referenced: GITHUB_TOKEN

  ✓ trusted at sha256:e07c838c….
  Editing the manifest or lockfile invalidates this — re-run `agentstack trust` after reviewing changes.
  Pinned skill/server content that drifts is blocked at use time until re-locked.
  Withdraw anytime with `agentstack trust --revoke`.
  ```

  It distinguishes *runs local code* (`▶ runs …`) from *contacts network*
  (`→ contacts …`), names every secret ref, flags in-process native
  extensions with an ALL-CAPS `EXECUTABLE` warning, and — when the project
  declares `[policy]` — prints "policy requested by this project (can only
  narrow the machine layer)". The consent moment teaches machine-vs-repo
  layering, but only for repos that declare policy; a repo with no `[policy]`
  block never learns the machine ceiling exists at trust time.

- Re-trust after editing the manifest re-lists the **whole** surface with no
  diff. Adding an `evil` server and re-running `trust` printed:

  ```
    ▶ github: runs `npx …server-github`
    ▶ filesystem: runs `npx …server-filesystem /tmp`
    ▶ evil: runs `python3 ./evil.py`
    secrets referenced: GITHUB_TOKEN
  ```

  The newly-added `evil` line is formatted identically to the two already-
  trusted servers — no `NEW`/changed marker. The user must eyeball the full
  list to spot what a `git pull` slipped in, which is exactly the case trust
  exists to catch.

- Untrusted-repo experience splits by audience. The **agent** gets a good
  note — `tools_search` returns "No proxied tools available. This project
  (…) is not trusted for auto mode, so none of its MCP servers are proxied
  (spawned or contacted). Ask a human to review the manifest and run
  `agentstack trust …`" — and `agentstack_list` exposes `bridge.trust:
  "untrusted"` (machine-readable). The MCP `tools/list` still serves 21
  control-plane tools; the declared `github` server is simply absent, inert.
  The stderr line ("control-plane tools only … Run `agentstack trust`") is
  honest but most clients don't surface stderr.

- The **human** running bare `agentstack` in an untrusted project sees trust
  as a one-word status with the wrong next step:

  ```
    Status    not locked (never activated) · untrusted
    Next:  agentstack setup   finish the first run — preview, apply, activate
  ```

  "untrusted" is never defined, and `Next` points at `setup`, not `trust` —
  nothing tells the human that trusting is what turns the repo's servers on
  for the bridge.

- `trust --list` flags lapse well: `⚠ … · manifest or lockfile changed since
  trusted — re-run `agentstack trust` there`.

- `gateway connect <harness>` is dry-run by default and shows the exact JSON
  diff plus an honest zero-file-limit note. But the teaching that ties the
  gateway to trust — "Each repo now only needs a trusted manifest:
  `agentstack trust <dir>` unlocks its servers for the bridge. Untrusted
  repos get control-plane tools only." — prints **only after `--write`**, not
  in the dry-run preview the user reads first.

### P14 — Re-trust should diff, not re-list

Facts: re-running `trust` after a manifest edit reprints the entire surface
with the new `evil` server formatted identically to already-trusted ones — no
change marker. Proposal: when a project was previously trusted, mark each line
against the last pinned digest (`+ added`, `~ changed`, `- removed`) for
servers, secrets, skills, extensions, and policy, so a `git pull`'s new
executable is visually obvious instead of buried in a flat list. First-trust
stays the full flat review (nothing to diff against).

**Status: DRAFT — awaiting maintainer review.**

### P15 — Keep the run/contact/secret/policy consent model; extend the policy line

Facts: the trust review already separates `▶ runs` from `→ contacts`, names
secret refs, ALL-CAPS-flags in-process extensions, and states "policy
requested … can only narrow the machine layer" — principle 4 done well. Gap:
that machine-vs-repo layering line appears only when the repo declares
`[policy]`. Proposal: adopt this surface as the canonical consent template,
and always print one line naming the machine policy file
(`~/.agentstack/agentstack.toml`) as the ceiling — so a user consenting to a
policy-free repo still learns at the consent moment that a machine layer
exists and where it lives.

**Status: DRAFT — awaiting maintainer review.**

### P16 — Untrusted teaching for the human, not just the agent

Facts: the agent-facing untrusted note is informative; the human's bare
`agentstack` shows only the word "untrusted" with `Next → setup`. Proposal:
when a manifest exists and is untrusted (or changed-since-trusted), bare
orientation says what that means in one line ("its servers are inert — the
bridge exposes control-plane tools only until you review it") and makes
`agentstack trust .` the next step, distinct from `setup`.

**Status: DRAFT — awaiting maintainer review.**

### P17 — `gateway connect` teaches the trust-unlock in the dry-run, not after write

Facts: the "every repo now needs a trusted manifest … untrusted repos get
control-plane only" pointer prints only on the `--write` path; the dry-run
preview a user reads first omits it. Proposal: include the trust-unlock line
in the dry-run output (the moment the user is deciding whether to register the
bridge is when they need to know trust is the per-repo gate), not only after
the change is committed.

**Status: DRAFT — awaiting maintainer review.**

## Profiles + library + packs (feature 5)

### Investigated facts (2026-07-18, v0.12.0)

- `lib add` is transparent: dry-run by default, records provenance, and warns
  honestly. Adding a skill from a temp dir:

  ```
    ⚠ source /private/tmp/…/myskill is a temporary directory — the recorded provenance will dangle once it is cleaned up (the library copy is unaffected)
  ✓ added 'greet' (path) in the central library
    copied /private/tmp/…/myskill → …/lib/skills/greet
    the library copy is now canonical — edits to the source have no effect
    checksum 5820fb5e5de0
  ```

- `lib list` carries a provenance column
  (`path:/Users/…` or `git:host/repo@rev#subpath`), but a dangling temp-dir
  source is shown as a live `path:/…` forever with no "source gone" marker:

  ```
  Skills
    greet   A demo greeting skill…   path   5820fb5e5de0   path:/private/tmp/…/myskill
  ```

- **The manifest-local vs library-by-name distinction is a live trap.**
  Declaring `[skills.greet]` (an inline block) with a library skill of the
  same name in `~/.agentstack/lib` does *not* resolve to the library copy —
  it is treated as an inline skill missing a source:

  ```
  error: resolving skill 'greet' for profile 'dev': skill has neither `path` nor `git` source
  ```

  The library reference works only when the name is listed in the profile's
  `skills = ["greet"]` with **no** `[skills.greet]` block:

  ```
  Activating profile 'dev' (scope: project) — 0 server(s), 1 skill(s)
    → 1 skill(s) to symlink into …/.claude/skills
  ```

  The error never hints that a library skill named `greet` exists, nor that
  the inline block is what shadowed it. Inline-wins-over-library precedence is
  likewise silent — no shadowing warning at activation.

- Profile discovery is fragmented. There is no "list profiles" command. Bare
  orientation shows a **count** only ("2 profile(s)"), never names. Names
  surface through `setup`, `explain`, and — most usefully — the
  disambiguation error itself:

  ```
  error: several profiles declared — name one: agentstack use <profile> (dev, prod)
  ```

  A plain "what profiles do I have and what's in each" has no home.

- `add from` surfaces provenance in output ("found {name} ({source}) —
  {id}"; git packs show tag + commit12), and pack install/upgrade print
  next-steps and gate house-rule instructions behind opt-in
  `--with-instructions`. `lib sync` blocks pushing a literal secret. These are
  the model — nothing to fix.

### P18 — A first-class profile listing

Facts: no command names profiles; bare orientation shows only a count; users
learn names by triggering the multi-profile error. Proposal: `agentstack use
--list` (or `agentstack profiles`) that names each declared profile, its
server + skill counts, and which is currently active — so "which profiles
exist and what's in them" stops being archaeology through the manifest.

**Status: DRAFT — awaiting maintainer review.**

### P19 — The inline-vs-library resolution error must teach

Facts: `[skills.greet]` with no `path`/`git` errors "skill has neither `path`
nor `git` source" even when a library skill named `greet` exists, with no
pointer to the by-name form. Proposal: when an inline skill has no source and
a library skill of that name exists, say so and show the fix ("`greet` is in
your central library — drop the `[skills.greet]` block and list it in the
profile's `skills = […]` to reference the library copy"). Optionally warn at
activation when an inline skill shadows a same-named library skill.

**Status: DRAFT — awaiting maintainer review.**

### P20 — `lib list` marks dangling provenance

Facts: a temp-dir/path source that no longer exists is shown as a live
`path:/…` indefinitely; the add-time warning is the only signal it will
dangle. Proposal: `lib list` checks whether a `path:` source still exists and,
when gone, renders it "source gone — library copy canonical" instead of a dead
absolute path, so provenance reads honestly long after the add.

**Status: DRAFT — awaiting maintainer review.**

### P21 — State the two mental models in one place

Facts: "by-name library reference" (resolved fresh, `checksum`/`rev` locator)
vs "vendored pack copy" (`[packs.<name>]` ledger with source/version/rev) is
coherent in code and honestly surfaced per-command, but never contrasted in
one place — and P19's trap grows straight out of that gap. Proposal: a short
help/doc paragraph (and a line in `explain`) that names the two models and
when each applies, so a user forms the mental model before hitting the inline
block.

**Status: DRAFT — awaiting maintainer review.**

## Locked runs + sandbox + lockdown (feature 6)

### Investigated facts (2026-07-18, v0.12.0)

- There is no `sandbox`/`lockdown` subcommand — all three are flags on
  `run`: `--locked` (host, fail-closed gate; routes to `locked.rs`),
  `--sandbox`/`--lockdown` (container; `sandbox.rs`), plus `--plan`.

- `run --locked --plan` leads with an honest posture + limits block, then a
  redacted proposed grant. Green (trusted + locked) tail:

  ```
  → plan for `run claude-code --locked` (nothing will be mutated)
    posture: HOST / PROTECTED
    ℹ protected host run: content trust, strict lock verification, and policy
       admission are enforced BEFORE launch … Not kernel isolation: the harness
       runs as you, on the host; the harness/interpreter binary itself is an
       unpinned $PATH executable; evidence is a cooperative local audit trail.
    ✓ no ambient user/global-scope MCP entries for this harness …
    ✓ trust: explicitly trusted
    ℹ commitment key: will be created on first live run
    proposed grant:
      project: …/proj
      harness: claude-code (0 redacted argument(s))
      servers: filesystem
      inputs: 0 skill(s), 0 instruction(s), 0 executable pin(s), 0 extension(s)
      digest: (bound on first live run, once the commitment key exists)
  ✓ live launch would proceed
  ```

  On the green path only *trust* gets a `✓` line — the other gates
  (locked-inputs verified, policy fits the ceiling, rendered extensions) are
  not enumerated; those per-gate teaching lines exist only in the **live**
  run, not in `--plan`, the "one auditable description of what a run would do".

- The refusal path teaches with the fix inline. Untrusted:

  ```
  error: a live `run claude-code --locked` would be REFUSED — 1 blocker(s):
    [trust] project is not trusted — run `agentstack trust .` after reviewing
  ```

  After editing the manifest (drift), the refusal is:

  ```
    [trust] configuration changed since it was trusted — re-review and re-trust
  ```

  Note: because trust pins the manifest **and** the lockfile, editing the
  manifest trips `[trust]` *before* `[locked-verify]` is ever reached. A user
  who changed a pinned input is told only to "re-trust" — never that the lock
  is now stale and that re-locking re-gates trust (the P9 rule). The
  lock-drift-specific messages ("skill content drifted from agentstack.lock…"
  `→ agentstack lock`) fire only when trust still holds but a pinned input's
  bytes drift underneath a matching manifest (e.g. a git skill upstream).

- `--sandbox --plan` / `--lockdown --plan` run fine with no Docker and no
  `sandbox` feature (as designed), and the posture labels are bluntly honest:

  ```
  ▶ sandboxing claude-code (run r-98e8e91e3b) — bundle trusted
    posture: SANDBOX / PROXIED · DIRECT ROUTE OPEN
    🛡 egress is routed through the AgentStack proxy…
  ```
  ```
    posture: LOCKDOWN / ENFORCED · NO DIRECT ROUTE
    🔒 lockdown: no host route, no internet — the container's only peer is the egress sidecar.
  ```

- But `run --sandbox` **live** without the feature draws the entire sandbox
  banner (`▶ sandboxing … trusted`, posture, workspace, egress) and only
  *then* errors:

  ```
  ▶ sandboxing claude-code (run r-44a33f7aa0) — bundle trusted
    posture: SANDBOX / PROXIED · DIRECT ROUTE OPEN
    workspace: …/proj/.agentstack → /workspace read-only …
    🛡 egress is routed through the AgentStack proxy…
  error: sandbox support is not compiled into this build — rebuild with `cargo build --features sandbox` …
  ```

  The banner implies a container is starting before the prerequisite check
  reveals none can. (Docker-daemon-down maps to "cannot reach Docker (…) — is
  the daemon running?" at the same late point.)

- **Severe (reported, not a proposal):** the workspace mount resolves to the
  **manifest's parent directory**, not the project root. With the recommended
  nested `.agentstack/agentstack.toml` layout, `--sandbox` mounts
  `…/proj/.agentstack → /workspace` — the agent sees only the `.agentstack`
  folder, not the project's code. A legacy root `agentstack.toml` correctly
  mounts the project root. Confirmed in scratch both ways.

### P22 — `--locked --plan` should enumerate every gate, like the live path

Facts: the green plan shows posture + a single `✓ trust` + the grant + "would
proceed"; the per-gate `✓` teaching (locked inputs verified, policy fits the
ceiling, rendered extensions verified) appears only in the live run.
Proposal: have `--plan` print the same per-gate `✓` lines the live path does,
so the "one auditable description" actually describes each admission decision
in user terms on the happy path, not just the refusals.

**Status: DRAFT — awaiting maintainer review.**

### P23 — A manifest edit should point at re-lock *and* re-trust together

Facts: editing the manifest trips `[trust]` first (trust pins manifest +
lock), so the refusal says only "re-review and re-trust" — the lock is now
stale but the user is never told to `agentstack lock`, nor that re-locking
re-gates trust (P9). Proposal: when the refusal is caused by a manifest change
that also invalidates the lock, name both steps and their order ("the lock is
stale — `agentstack lock`, review, then `agentstack trust .`; re-locking is
itself a re-decision"), so the two content-bound anchors aren't taught as one.

**Status: DRAFT — awaiting maintainer review.**

### P24 — Check the sandbox prerequisite before drawing the banner

Facts: `run --sandbox` without the `sandbox` feature (and, at the same late
point, with Docker down) prints the full sandboxing banner before erroring.
Proposal: run the feature/daemon prerequisite check first and fail with the
rebuild/daemon instruction *before* any "▶ sandboxing …" output, so the user
is never shown a container start that cannot happen. `--plan` keeps working
without Docker (it explicitly describes, not launches).

**Status: DRAFT — awaiting maintainer review.**

### P25 — Keep the posture labels and limits block as the disclosure template

Facts: `HOST / PROTECTED`, `SANDBOX / PROXIED · DIRECT ROUTE OPEN`,
`LOCKDOWN / ENFORCED · NO DIRECT ROUTE`, and the "Not kernel isolation …
cooperative local audit trail" block are principle 4 (explain the why) at its
best — "ENFORCED" is reserved for lockdown by design. Proposal: adopt this
run/contact honesty as the template for every posture disclosure across the
product (trust, guard, gateway), so a user reads the same blunt vocabulary
about what is and isn't actually enforced everywhere.

**Status: DRAFT — awaiting maintainer review.**

## Next features to discuss

Walkthrough continues; discussion sections land here as we go. Covered so far:
- first-run (feature 1) — P1–P7
- doctor + drift (feature 2) — P8–P10
- guard (feature 3) — P11, P13 (+ P3 above); new harness coverage P12/P12a
- trust gate + gateway (feature 4) — P14–P17 (DRAFT)
- profiles / library / packs (feature 5) — P18–P21 (DRAFT)
- locked runs / sandbox / lockdown (feature 6) — P22–P25 (DRAFT)

Still to walk: recorder / reports / audit, secrets lifecycle, adapters +
`export`/`import`, dashboard + proxy.
