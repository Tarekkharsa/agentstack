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

## Next features to discuss

Walkthrough continues; discussion sections land here as we go:
- guard (feature 3) — includes P3 above
- trust gate + gateway (feature 4)
- profiles / library / packs (feature 5)
- locked runs / sandbox / lockdown (feature 6)
