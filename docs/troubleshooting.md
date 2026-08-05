<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Troubleshooting

Search this page for the text your terminal printed. Every message below is
quoted from the binary, so pasting the line you got into your browser's find
box should land you on the fix.

Two commands answer most of it before you read any further:

```bash
agentstack status    # where this project stands + the one next step
agentstack doctor    # every check, each finding followed by ↳ its fix
```

`doctor` writes nothing. It ends with a triage line — `start with: <command>` —
that names the single highest-value fix, and every finding carries its own
repair after a `↳`. If a message you hit is not on this page, that arrow is the
answer.

## My CLI doesn't see the servers

The most common cause is not an error at all: **a harness reads its config at
startup**. After any write, `apply` says so itself.

```text
→ Restart or reopen your agent CLIs so they pick up the new config.
  undo: agentstack x restore --last --write
```

Restart the CLI first, then work down this list.

Under the default routing there is often **no file to look at** — an MCP-capable
CLI is served live. `agentstack x why <name>` states where that capability came
from, whether it is pinned and approved, which CLIs serve it live, which get a
file, and what it reaches. Start there before hunting for a config on disk.

If a write was *refused* rather than skipped, the cause is almost always the
trust gate — jump to
[it refuses because the project isn't trusted](#it-refuses-because-the-project-isnt-trusted).

**`Claude Code    1 change pending ↳ agentstack apply --write`**

The manifest declares something the native config does not have yet. Nothing is
written until you ask.

```bash
agentstack apply --write
```

**`Claude Code    config present but binary not on PATH`**

agentstack found that CLI's config file but not the CLI itself, so it will
render for a tool you cannot launch. Install the CLI, or drop it from
`[targets].default` in the manifest.

**`No targets to apply to. Set [targets].default or pass --target.`**

Nothing is listed for `apply` to write to. Either name the CLIs in the manifest
or aim one run by hand:

```bash
agentstack apply --target claude-code --write
```

**`Claude Code    not detected (ok unless you use it)`**

A note, not a fault. agentstack ships adapters for many CLIs and reports the
ones it did not find so a machine with 5 of 13 installed is not greeted by 8
warnings. If you *do* use that CLI, it is installed somewhere agentstack does
not look — check that its config lives at the standard path.

**`1 target in sync — wrote 1.` but the CLI still shows nothing**

Check the scope. Project scope writes a repo-local file (`.mcp.json`); global
scope writes the CLI's user-level config. A server rendered into the repo is
invisible to a session you started somewhere else.

```bash
agentstack apply --scope global --write
```

More: [reference — scopes](reference.md#scopes).

**Nothing exists on disk at all, and that is deliberate**

```text
not rendering configs — clean-at-rest keeps them off disk
```

In clean-at-rest mode nothing is generated between sessions; capabilities exist
only inside `agentstack run` or between `agentstack x session start` and
`agentstack x session end`. A missing `.mcp.json` here is the design. Do not
create one. See [delivery modes](concepts.md#delivery-modes).

**`Codex will IGNORE {dir}/.codex/config.toml — the project is not trusted in ~/.codex/config.toml (projects."{dir}".trust_level) ↳ open Codex in this folder once and accept the trust prompt`**

This is Codex's own project-trust list, not agentstack's. agentstack rendered
the file correctly; Codex refuses to read it until you have opened Codex in
that folder once and accepted its prompt.

**`instruction '{fragment}' targets '{target}', which has no instructions file ↳ remove the target or use a supported CLI`**

An `[instructions.*]` fragment names a CLI that has no `CLAUDE.md`/`AGENTS.md`
equivalent to compile into.

**`{cli}    managed region stale (project scope) ↳ agentstack x instructions --write`**

Instruction fragments changed in the manifest but the compiled block in the
CLI's instructions file is old.

```bash
agentstack x instructions --write
```

## A secret won't resolve

agentstack never stores secret values in the manifest — only `${REF}`
placeholders resolved per machine. An unresolved ref **blocks the write**; that
is the intended behaviour, not a bug to route around.

**`✗ unresolved secret LINEAR_TOKEN (server 'linear') ↳ agentstack secret set LINEAR_TOKEN`**

Nothing in the resolution chain (env, varlock, keychain, project `.env`)
holds a value for that name.

```bash
agentstack secret set LINEAR_TOKEN
agentstack apply --write
```

**`✗ not written — unresolved secrets; set them or pass --allow-unresolved`**

The per-target consequence of the line above: that config file was left
untouched. `--allow-unresolved` writes the literal `${REF}` through to the
native config, which is occasionally what you want (the harness expands it
itself) and usually not.

**`error: 1 blocked write on 1 target — fix: agentstack secret set LINEAR_TOKEN (or pass --allow-unresolved)`**

The closing line, and a **nonzero exit** — scripts and CI must not read a
blocked `apply --write` as success. It names every missing ref, so the fix is
copy-pasteable.

```text
✗ secret read failed LINEAR_TOKEN (server 'linear') — keychain read failed: A
  default keychain could not be found. ↳ run `agentstack secret set LINEAR_TOKEN`,
  then re-run
```

Different cause, same fix. The ref exists but its *store* failed — a locked or
missing keychain, a headless machine, a stripped environment. On a machine with
no keychain, put the value in the environment or a project `.env` instead.

**`LINEAR_TOKEN         not found ↳ agentstack secret set LINEAR_TOKEN`**

The `doctor` and `agentstack secret list` form. `secret list` also names the
layer each resolved ref came from, so you can see *which* store answered:

```bash
agentstack secret list
```

**`.env is readable by other local accounts — it holds real token values ↳ chmod 600 {path}`**

A `.env` is the one store that keeps real values in plain text on disk.
Versions before 0.16 wrote it at the ambient umask. Only the owner can fix the
mode:

```bash
chmod 600 .env
```

**`✗ blocked by policy: <entry>`**

Not a missing secret — a refusal. `[policy.secrets]` or `[policy.egress]`
denied that resolution, and `--allow-unresolved` does **not** override policy.
Widen the machine ceiling or narrow what the project asks for.

Three more lines `doctor` prints in the same **Secrets** section speak about the
recommended vault rather than about one ref:

**`varlock  .env.schema opts this project in, but the varlock binary is not runnable`**

The project asked for varlock and it is not there, so **every ref is quietly
falling through to the next store** — the silence this finding exists to break.
Install varlock ([varlock.dev](https://varlock.dev)), or delete
`.agentstack/.env.schema` to drop the layer on purpose.

**`varlock  varlock load failed — …`**

varlock is installed but could not resolve this project's schema. Doctor quotes
the first line of its error; run `varlock load` in the project to see all of it.

**`varlock  not in use — the recommended vault keeps values out of this project entirely`**

Not a defect and never doctor's next action — an invitation. `agentstack init`
offers to write the `.env.schema` that opts in, or drop one in next to the
manifest yourself. It declares names, never values.

More: [reference — secret resolution](reference.md#secret-resolution) and
[unresolved secrets block writes](reference.md#unresolved-secrets-block-writes).

## It says my files drifted

Drift means the native config on disk no longer matches what the manifest would
render. agentstack never silently reconciles it — you choose which side wins.

```bash
agentstack x diff              # show exactly what differs, change nothing
agentstack adopt --write     # the disk is right: pull the edit into the manifest
agentstack apply --write     # the manifest is right: re-render over the disk
```

`diff` reports the same comparison `doctor` does, so the two never disagree.

**`Claude Code    no longer matches what agentstack last wrote ↳ review: agentstack x diff · adopt the on-disk version: agentstack adopt`**

The region agentstack manages changed since its last write. `doctor` states the
fact without guessing the cause — a hand-edit is the common one, but a session
that ended onto a stale baseline reaches the same state, so it does not accuse
you of editing. `agentstack x diff` shows what moved and labels each entry
`managed`, `foreign (kept)`, or `hand-edited`. Then pick a side: `adopt` pulls
the on-disk version back into the manifest so it survives the next apply;
`apply --write` throws it away and re-renders.

**`Claude Code    kept <names> — applied by another manifest ↳ keep them: agentstack adopt · prune them: agentstack apply --prune-foreign`**

**Foreign** entries: written by a *different* project's manifest into the same
global file. `apply` keeps them by default — this is context, not a defect.
Removing them takes the explicit flag, because they are somebody else's
servers.

**`Claude Code    would REMOVE <names> ↳ keep them: agentstack adopt · prune them: agentstack apply --write`**

A pending prune. Those entries exist in the config but no longer in the
manifest, so the next write deletes them. The message names the victims
deliberately; decide before you run it.

**`{name}    changed in {app} (owner) ↳ refresh manifest + re-fan out: agentstack apply --write`**

An **owned** server — one whose defining app rewrites its own config by design.
The app's copy is authoritative; `apply --write` refreshes the manifest from it
and re-fans it out to the other CLIs.

**`Claude Code    rewritten by the app itself (owned server) — refresh the manifest: agentstack apply --write`**

Same situation, benign variant: the file changed but agentstack's managed
region still matches. Live-state churn in configs a running session rewrites
constantly (`~/.claude.json`) is ignored on purpose, so `doctor` does not flap.

**`{name}    content drifted from lock ↳ agentstack lock --write`**

A different kind of drift: the *pinned bytes* of a skill, server, instruction,
extension or workflow changed since `agentstack.lock` was written. Use sites
fail closed until you re-pin.

```bash
agentstack lock --write
```

```text
{name}    content changed since you approved it — the pinned bytes and the files
          on disk differ; re-pin it, then review and re-trust with
          `agentstack trust .` ↳ agentstack lock --write
```

The same drift on a project you have already **trusted**, and the wording says
what that costs you: re-pinning is not the whole fix. `lock --write` accepts the
new bytes and, because the lockfile is part of the consent surface, immediately
marks your grant stale. Two steps, in this order:

```bash
agentstack lock --write    # accept the new bytes
agentstack trust .         # review them and re-grant
```

Full explanation: [I re-locked and it still will not
deliver](#i-re-locked-and-it-still-will-not-deliver).

**`{name}    not locked ↳ agentstack lock --write`** / **`{name}    from library, not locked ↳ agentstack lock --write`**

Something the manifest references has no lock entry at all. Same fix.

**`error: existing config is not valid JSON: expected ident at line 1 column 2`**

agentstack refuses to write into a file it cannot parse, because merging into
broken JSON would destroy it. Open the named file, fix the syntax, re-run. If
the file is unsalvageable, `agentstack x restore <adapter>` puts back its
single-slot backup.

### Settings drift is reported as two legs, with two different fixes

`[settings.*]` values are pinned per key, so `doctor`'s **Settings** section
answers two separate questions and never merges them. Read the `↳` — it tells
you which leg moved.

**Leg 1 — the declaration no longer matches the pin.** Fix with `lock --write`.

```text
Claude Code    2 keys not pinned in agentstack.lock (model, permissions) ↳ agentstack lock --write
Claude Code    1 declared key moved since the lock was written (model) ↳ agentstack lock --write
```

The first line is also what a lockfile written before settings pins existed
reports; it is an advisory, not an error, and the next `lock --write` backfills
it.

**Leg 2 — the declaration no longer matches the file on disk.** Fix with
`apply --write`.

```text
Claude Code    2 keys not yet in {path}/.claude/settings.json (model, permissions) ↳ agentstack apply --write
Claude Code    1 owned key in {path}/.claude/settings.json drifted from the declared value (model) ↳ agentstack apply --write
```

`not yet in` means the merge has not happened; `drifted from the declared value`
means it happened and something changed the key afterwards. Only keys agentstack
declares are named — your own unrelated edits elsewhere in `settings.json` are
never reported as drift.

More: [drift — adopt or apply?](reference.md#drift-adopt-or-apply) and
[concepts — drift](concepts.md#drift).

## It refuses because the project isn't trusted

Untrusted means **inert**: a cloned repository's declarations cannot spawn
servers, enter agent context, or resolve secrets until a human has read them.
A consented `agentstack init` records trust for you, so in practice this gate
shows up in two places — a repo you cloned, and a manifest that changed since
you approved it.

### The five kinds the gate holds back

Every refusal below opens with `refusing to …` and closes the same way: nothing
is written, the run **exits nonzero**, and the fix is always `agentstack trust .`.
The two halves of each `is not trusted` line change to `changed since it was
trusted` when the project *was* approved and its bytes moved since.

**MCP server config** — from `apply --write` and `use --write`. No `.mcp.json`,
no `[mcp_servers]` block. The per-target consequence follows on its own line.

```text
✗ refusing to render MCP servers: project at {dir} is not trusted — review and
  `agentstack trust .` before writing server definitions the harness launches on
  its own ('notes')
✗ not written — the project has not been trusted for this content
```

**Skill files** — from `use --write`. No skill directory or symlink lands in
`.claude/skills/` or its per-CLI equivalents. (`session start` never reaches
this line: it refuses up front with its own message, below.)

```text
✗ refusing to materialize skills: project at {dir} is not trusted — review and
  `agentstack trust .` before putting its words into an agent's context ('notes')
✗ skills not materialized — the project has not been trusted for this content
```

**Instruction fragments** — from `apply --write`. Nothing is compiled into the
managed region of `CLAUDE.md` / `AGENTS.md`: a repo's prose reaches the model
only after a yes.

```text
✗ refusing to render instructions: project at {dir} is not trusted — review and
  `agentstack trust .` before putting its words into the managed region a harness
  reads straight into an agent's context ('house')
✗ instructions not written — the project has not been trusted for this content
```

`agentstack x instructions --write` refuses the same content with a shorter
pair of lines, and the same nonzero exit:

```text
✗ not written — the project has not been trusted for this content
error: 1 instruction file not written — the project has not been trusted for this
content — review and `agentstack trust .`
```

**Hooks**, which the harness runs at full user permission.

```text
✗ refusing to render hooks: project at {dir} is not trusted — review and
  `agentstack trust .` before rendering hook commands the harness runs at full
  user permission ('fmt')
✗ hooks not written — the project has not been trusted for this content
```

**Extensions**, which are executable code. This one aborts the command outright.

```text
error: refusing to render native extensions: project at {dir} is not trusted —
review and `agentstack trust .` before rendering executable extension code
```

Hooks and extensions run code, so they take the full consent ceremony every
time and accept no relaxation at all.

The command's closing line names the count, and the exit code is nonzero — a
script must not read a refused write as success:

```text
Wrote 0 of 1 target; 1 blocked — see the ✗ line for each; see ✗ above.
error: 1 blocked write on 1 target — each ✗ above names the blocker
```

```text
⚠ activated 'default' on 0 targets (wrote 0); 2 targets BLOCKED: Claude Code, Claude Code
error: 2 targets blocked — each ✗ above names the blocker
```

**What the gate does *not* hold back.** Pruning and removal, machine-layer
content, the machine manifest at `~/.agentstack/agentstack.toml`, and
`[settings.*]` values all go through on an untrusted project. Seeing
`✓ wrote 1 setting` next to a refusal is correct, not a leak: none of them
authorizes new content — a settings value declares no code to run, and taking a
server away can only shrink what a CLI can reach.

One caveat on removal: the gate blocks a *target*, not one entry. If the same
config file still has something the gate refuses, the whole write is skipped and
the planned prune waits with it:

```text
− pruning 'notes' (no longer in manifest)
✗ refusing to render MCP servers: project at {dir} is not trusted …
✗ not written — the project has not been trusted for this content
```

When the prune is all that is left, it lands:

```text
− pruning 'notes' (no longer in manifest)
− removed empty {dir}/.mcp.json
1 target in sync — wrote 1.
```

The `doctor` forms of the same state:

```text
⚠ Claude Code    servers not delivered — project not trusted ↳ agentstack trust .
⚠ Claude Code    hooks not delivered — project not trusted ↳ agentstack trust .
```

### The session and probe refusals

```text
error: refusing to start a session: this project is not trusted — review and
trust it with `agentstack trust` (or the UI trust review), then retry
```

```bash
agentstack trust .
```

That prints every server, contact, secret and skill the project declares,
then asks for consent.

```text
error: refusing to start a session: the manifest or lockfile changed since this
project was trusted — review with `agentstack trust` (or the UI trust review),
then retry
```

Trust is bound to content. A `git pull`, or your own edit to the manifest or
lockfile, invalidates the old approval — that re-gate is the feature. Review
what changed, then re-trust.

```text
refusing to probe: this project is not trusted — starting its servers would run
code nobody has reviewed ↳ agentstack trust
refusing to probe: the manifest or lockfile changed since this project was
trusted ↳ agentstack trust
```

`doctor --probe` warns rather than aborting, and finishes the rest of the
report.

**`trusted, but the manifest or lockfile changed since it was last reviewed ↳ agentstack trust`**

The `doctor` form of the same state. `agentstack status` calls it
`trust stale (content changed)` and names the next step:

```text
Next:  agentstack trust .   the content changed since you reviewed it — review and re-trust
```

<a id="lock-first-then-trust"></a>
### `lock --write` invalidates the grant — lock first, then trust

The lockfile is part of the consent surface, so **re-pinning is new consent**.
Running `lock --write` on a project you have already trusted always leaves the
grant stale, and `lock` says so before it says anything else:

```text
⚠ this project is trusted — new pins are new consent, so its trust is now stale; re-review and re-grant with `agentstack trust .`
```

So the order is fixed, and it is the reverse of what most people try:

```bash
agentstack lock --write   # 1. pin the bytes
agentstack trust .        # 2. review and approve the pinned bytes
```

Trusting first and locking after simply throws the approval away. `trust` will
not even let you go the other way round — an unpinned surface is not
approvable:

```text
error: cannot trust {path}: its loadable surface isn't fully pinned — 1 item needs locking or review:
  notes  inline skill unpinned — run `agentstack lock --write`
Pin with `agentstack lock --write`, then review and trust with `agentstack trust .`.
```

<a id="i-re-locked-and-it-still-will-not-deliver"></a>
### "I re-locked and it still will not deliver"

This is the same rule seen from the other end, and it is the most common
surprise. You edited a pinned skill, server or instruction. Delivery failed
closed on the lock:

```text
error: refusing to activate 'default': 1 pinned item changed since agentstack.lock was written —
  notes  skill content drifted from agentstack.lock (locked 0bc689f0eecd, current 49abb4cbcab0)
Review the changes with `agentstack lock`, then run `agentstack lock --write` to accept them (re-locking re-gates the project for auto mode).
```

`apply --write` fails the same way for an instruction fragment, under a
different opening sentence:

```text
error: refusing to compile instructions for {dir}/.agentstack: 1 pinned item changed since agentstack.lock was written —
  house  instruction content drifted from agentstack.lock (locked 1718cc28481d, current a1aecdd87ab5)
```

You then ran `agentstack lock --write`, as instructed — and the next command
refuses again, now with the *drifted* wording:

```text
✗ refusing to materialize skills: project at {dir} changed since it was trusted — review and `agentstack trust .` before putting its words into an agent's context ('notes')
```

Nothing is broken. Re-locking accepts the new bytes; it does not review them.
Accepting content is a machine's job, reviewing it is yours, and only the second
one delivers. Read the parenthesis in the first message literally — *re-locking
re-gates the project*. The complete fix is three commands:

```bash
agentstack lock            # see what moved
agentstack lock --write    # accept the new bytes
agentstack trust .         # review them — this is what unblocks delivery
```

### "I added a skill and now everything else refuses"

`agentstack add … --write` and the panel's edit verbs write the manifest and the
lockfile **and** deliver, in one run. They judge trust as it stood when the
command started, so the thing you just asked for is not refused by the bytes you
just asked it to write:

```text
✓ added 'tidy'.
  ✓ claude-code: 1 skill → {dir}/.claude/skills
```

But they deliberately do **not** re-pin the grant. The new capability is real
content and still owes you a review, so the project is left stale and the *next*
command re-gates:

```text
Status    locked · trust stale (content changed)
Next:  agentstack trust .   the content changed since you reviewed it — review and re-trust
```

```bash
agentstack trust .
```

If the delivery half failed too, `add` names the order for you:

```text
error: 1 target failed to materialize (the manifest and lock writes stand — each ✗
above names the blocker; if it is consent, review with `agentstack trust .`, then
`agentstack use --write`)
```

### Writes that keep your grant

Not every manifest write costs you a review. A write that records a
*preference* — it declares no capability and runs no code — carries valid trust
across itself, so the documented next step is not refused by the bytes the
command just wrote:

- `agentstack x delivery render-locally [--harness <id>] --write`
- the `.gitignore` preference (`[meta] gitignore`, set by `init`)

Both re-pin **only** when trust was valid immediately before the write. An
untrusted project stays untrusted, and a review already pending stays pending —
neither command can create or resolve a grant.

### Other trust messages

**`not trusted — 1 CLI uses the gateway, but this project's 1 server is not proxied ↳ agentstack trust <path>`**

A harness is wired to the gateway, this project declares servers, and none of
them reach the agent — every session here silently gets control-plane tools
only.

**`not trusted for auto mode — untrusted repos get control-plane tools only ↳ agentstack trust`**

The same fact when nothing is wired up yet. Stated as `ok`, because staying
untrusted is a legitimate choice.

```text
error: refusing to trust: stdin is not a terminal — review the declarations
above and re-run interactively, or acknowledge non-interactively with --yes
--consented-digest <surface_digest from `agentstack trust --preview`>
```

Typing the command at a terminal *is* the consent, so a piped or scripted
`trust` has no human in it. For automation, review the surface and pass its
digest back:

```bash
agentstack trust --preview                      # prints surface_digest
agentstack trust --yes --consented-digest sha256:…
```

```text
error: refusing to trust: --yes requires --consented-digest — run `agentstack
trust --preview`, review the surface, and pass its `surface_digest` back
```

`--yes` alone would make "the user saw the review" the caller's claim rather
than a checked fact.

**`cannot trust {path}: its loadable surface isn't fully pinned — N items need locking or review`**

Trust binds to bytes, so everything loadable must be pinned first. Full output
and the ordering rule: [lock first, then trust](#lock-first-then-trust).

**`error: refusing to apply without --consented <digest> — run --preview first, review, then pass the digest it printed`**

From the digest-bound panel actions (`create-profile`, `use-profile`,
`add-server-to-profile`, `add-skill-to-profile`). Run the same command with
`--preview`, read the JSON, then re-run with `--yes --consented <digest>`.

To withdraw consent at any time:

```bash
agentstack trust --revoke
```

More: [trust a cloned repo](howto/trust-a-repo.md) and
[what "trusted" does and does not mean](ENFORCEMENT.md#what-trusted-does-and-does-not-mean).

## I want to undo something

Every write agentstack makes is recorded before it lands. `agentstack undo`
(v0.18.0+) is the interactive front door — your recent changes newest-first,
pick a point and revert; `restore` is the same record as a script-friendly
command:

```bash
agentstack undo                     # timeline: pick a point, revert to it
agentstack x restore                  # list every undoable recorded write
agentstack x restore --last --write   # undo the most recent
agentstack x restore 18c634a4 --write # undo one by its id prefix
```

The ledger looks like this:

```text
Recorded changes (newest first):

  18c6358f  20s ago  project  apply   1 file · Claude Code

Undo one with: agentstack x restore <id> --write (or --last for the newest)
```

Each row names the operation that wrote it — `init`, `apply`, `session start
'backend'` — so three otherwise identical rows can be told apart. `agentstack
restore --list` is an alias for the bare form, since that is what most people
type.

**Restoring a config that a tool broke, not agentstack**

```bash
agentstack x restore claude-code
```

That is the fallback path: one adapter's config from its single-slot backup,
rather than a recorded change.

**What `restore` does not cover.** It reverts agentstack's own recorded config
writes — not side effects a tool already had. A file a server deleted does not
come back. Five actions are not file writes and have their own verb: `gateway
disconnect`, `guard uninstall`, `trust --revoke`, `session end`, and `remove`.
Replacing an already-managed skill with the same name is not snapshotted
byte-exact, so its restore is not promised exact.

To take everything back off at once, see
[undo anything](howto/undo.md) — `agentstack x uninstall` previews first and is
itself undoable.

## A server won't start

**Start here: `agentstack doctor --probe`.**

Everything else on this page reads your configuration. `--probe` actually
starts each stdio server, speaks the MCP `initialize` handshake, and stops it
again — so instead of "your manifest is well-formed" you get a per-server
answer to the question you actually have.

```text
$ agentstack doctor --probe
MCP server startup (--probe)
  ✓ notes          started in 62ms · demo-notes · 3 tools
  ✗ missing        did not start: No such file or directory (os error 2)
  ✗ stuck          no response 10s after starting — killed — waiting for the database…
  ⚠ needs-token    not probed — DEMO_API_TOKEN does not resolve ↳ agentstack secret set DEMO_API_TOKEN
```

How to read each outcome:

- **`did not start`** — the command isn't there, isn't executable, or its `cwd`
  doesn't exist. If the command is a bare launcher, read the advisory below.
- **`exited before the handshake`** — it started and then gave up. The clause
  after the dash is the server's own stderr, which usually names the reason (a
  bad argument, a rejected credential, a missing runtime).
- **`no response …s after starting — killed`** — it came up and then hung.
  Common when a server waits on something that isn't running yet.
- **`not probed`** — a `${REF}` doesn't resolve on this machine, so nothing was
  started; set the secret and re-run rather than reading it as a server fault.
- **`refusing to probe`** — the project isn't trusted at its current bytes.
  Review it with `agentstack trust` first; starting a repo's servers is exactly
  what that gate holds back.

Every probe is bounded: ten seconds per server, then the child is killed with
its whole process group and reaped. Nothing is left running.

One caveat. The probe inherits the environment you ran it from, so a server can
pass `--probe` in your terminal and still fail inside a GUI-launched app — which
is precisely the failure the next advisory is about.

**`N servers use a bare launcher that resolves via PATH: linear (npx). A GUI-launched harness (Claude Code.app, Claude Desktop, VS Code) may inherit a minimal PATH and fail to spawn them. Terminal-launched CLIs are unaffected. To pin them, use an absolute path or a login-shell wrapper: command = "zsh", args = ["-lc", "exec <launcher> …"]`**

This is the single most common "it works in my terminal but not in the app"
failure. Nearly every published MCP server ships as `npx -y …`, and `npx` is
found through `PATH` — which a GUI-launched app does not inherit from your
shell. agentstack states this once as an advisory rather than once per server,
and it does not count against readiness.

Two fixes, in the manifest:

```toml
# absolute path — no PATH lookup at all
command = "/Users/you/.nvm/versions/node/v22.14.0/bin/npx"

# or a login shell, which sources your profile and finds it the way you do
command = "zsh"
args = ["-lc", "exec npx -y linear-mcp"]
```

```text
server 'x': stdio transport ignores `headers`
server 'x': http transport ignores `command`
```

The entry mixes transports. `headers` belong to an `http` server, `command` to
a `stdio` one; the ignored field is silently dropped at render time.

**`server 'x': ${VAR:-default} syntax is unsupported by Codex`**

Codex has no default-value expansion. The manifest renders to every target, so
this is flagged generally even if only one CLI chokes on it.

**`'x' has a cwd that Claude Code can't express — it renders without one (wrap the command in a shell that cd's if the server needs it)`**

Not every native config format has a working-directory field.

```toml
command = "zsh"
args = ["-lc", "cd /path/to/project && exec my-server"]
```

**`{name}    not installed ↳ agentstack x install`** / **`not materialized ↳ agentstack x install`**

A skill or git-hosted capability is referenced but its source was never
fetched into the store.

```bash
agentstack x install
```

**`{cli}    broken skill link '<name>' → <target> (target missing) ↳ remove it: rm <path> · or reinstall the skill it points at`**

A materialized skill is a symlink whose target is gone — usually a store
cleared or a skill removed outside agentstack.

```text
{name}    no SKILL.md in <dir>
{name}    SKILL.md has no frontmatter description ↳ add `description:` so
          search and agents can find it
```

A skill directory that is not a skill yet. The description line is what
`agentstack search` matches and what an agent sees in the loadable index — a
skill without one is effectively invisible.

**Checking a live server rather than its config**

```bash
agentstack doctor --live
```

That adds a real MCP `initialize` handshake to each HTTP server, so a server
that parses but does not answer is caught.

## When the manifest itself is rejected

```text
error: manifest has validation errors — nothing was written; fix the ✗ above,
then re-run `agentstack apply --write`
```

Structural problems, listed above the line with their own `↳` fixes. Nothing
was written, and the command exits nonzero.

```text
error: no profile 'dev' in manifest — check the `[toolsets.*]` tables there for
the exact name
```

The toolset name does not exist. `agentstack use --list` prints the declared
ones with a readiness flag for each.

**`⚠ server 'github' is defined differently by 1 other CLI — kept the first definition imported (the other stays in its CLI's own config)`**

From `agentstack init`. Two CLIs disagreed about the same server name, so the
import kept the first one it read. Nothing is lost — the other definition is
still in its own CLI's config until you apply. Open the manifest, check the
entry that won, and fix it if the wrong one did.

**`{name}: unknown adapter`**

`[targets].default` names a CLI agentstack has no adapter for.

```bash
agentstack x adapters list
```

**`effective machine policy unavailable — drift rendering is BLOCKED`**

The machine-level manifest at `~/.agentstack/agentstack.toml` could not be
read, and project policy can only narrow the machine ceiling — so with no
ceiling there is nothing to narrow. Fix that file first.

## Still stuck

```bash
agentstack doctor --all      # every section, including the ones this project doesn't use
agentstack doctor --deep     # also scan every skill body for hidden-unicode / injection findings
agentstack doctor --ci       # everything, plus a nonzero exit on any error
agentstack doctor --json     # machine-readable, for a UI or a bug report
agentstack x explain <name>    # what one server, skill, or instruction actually is
```

By default `doctor` hides sections for features this project does not use and
says how many it hid. `--all` shows them.

- [FAQ](faq.md) — the questions that come up in the first week
- [Concepts](concepts.md) — every term in two or three plain sentences
- [Reference](reference.md) — the complete command inventory
- [Undo anything](howto/undo.md) — every reversal path in one place
