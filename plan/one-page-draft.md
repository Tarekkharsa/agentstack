# AgentStack, on one page

Draft for a decision. Everything the product does, everything a person can do
with it, derived from `target/release/agentstack` 0.18.0-rc.2 and
`crates/core/src/manifest/model.rs` — not from the docs. Every command below
was run. Exit codes are the binary's.

---

## 1. What it is

One manifest declares your agent setup. AgentStack delivers it to every coding
CLI you have — live where the tool can take it, as native config files where it
cannot — and refuses to deliver anything you have not reviewed.

Four ideas cover the product: **Setup** (what you have) · **Toolset** (what
this task needs) · **Status** (is it ready) · **Undo** (how to take it back).

---

## 2. What you can declare

The manifest (`.agentstack/agentstack.toml`, `version = 1`) holds these
tables. This is the whole capability model.

| Table | What it is |
|---|---|
| `[servers.*]` | MCP servers — `http` (url + headers) or `stdio` (command, args, cwd, env). Values may hold `${REF}` secret placeholders. |
| `[skills.*]` | Portable `SKILL.md` directories. From a local path, a git repo, or the central library. |
| `[instructions.*]` | House-rule fragments compiled into each CLI's `CLAUDE.md` / `AGENTS.md`, inside a managed region. Hand-written prose survives. |
| `[settings.<adapter>]` | Native per-CLI settings (permissions, feature flags), merged non-destructively into that CLI's own settings file. |
| `[hooks.*]` | Lifecycle hooks compiled into each hook-capable CLI. **These run code.** |
| `[extensions.*]` | Native harness extensions (e.g. Pi/OpenCode add-ons). One adapter each, pinned strictly. **These run code.** |
| `[workflows.*]` | Governed multi-agent scripts AgentStack itself executes. Inert — not runnable, not even nameable — until trusted and strictly pinned. |
| `[toolsets.*]` | Named subsets of the above. The switchable unit. (`profiles` is the accepted older spelling.) |
| `[packs.*]`, `[package_overrides.*]` | Vendor-pack install ledgers and per-project divergence from a package. Bookkeeping; members ride the normal tables. |
| `[targets]` | Which CLIs commands act on, and the default scope. |
| `[policy]` | Required / forbidden capabilities, source allowlist. A repo can only narrow the machine ceiling, never loosen it. |
| `[guard]`, `[experimental]` | Machine-manifest only. A repo manifest can never enable or widen them. |
| `[delivery]` | The single override: `render-locally`. Delivery is otherwise routed, not chosen. |

Alongside it: `agentstack.lock` pins the exact content you reviewed;
`agentstack.lock.sig` is an optional detached ed25519 signature.

---

## 3. What it delivers to

13 adapters, from `agentstack adapters list`:

`antigravity` · `claude-code` · `claude-desktop` · `codex` · `copilot-cli` ·
`cursor` · `gemini` · `junie` · `kiro` · `opencode` · `pi` · `vscode` ·
`windsurf`

Two lanes, and the router picks per capability and per tool — you do not:

- **Live lane.** Skills and MCP servers are served over the gateway to tools
  that speak MCP. Zero project files. Needs the bridge registered once:
  `agentstack x gateway connect --all --write`.
- **Rendered lane.** House rules, settings, hooks and extensions are always
  written to native files — and everything is written for a tool that reads
  files only (Pi). `agentstack x delivery` shows the routing;
  `agentstack x delivery render-locally --write` forces files everywhere.

---

## 4. The gate

**Nothing activates until you review it.** `agentstack trust` pins a content
digest over the manifest layers *and* the lockfile. A `git pull`, an
`agentstack lock`, an edit — any of them re-gates.

**The gate covers five kinds.** On an untrusted or drifted project, delivery
refuses, names the item, and exits nonzero:

1. MCP server config is not written and no server is spawned or contacted
2. skill files are not materialized
3. instruction fragments are not compiled into `CLAUDE.md` / `AGENTS.md`
4. hooks are not rendered
5. extensions are not rendered

Hooks and extensions run code, so they take the full ceremony every time and
accept no relaxation. Secrets are not resolved for an untrusted project. An
auto-discovered project reaching the gateway gets control-plane tools only.

**Three things stay outside the gate**, because none authorizes new content:
removal and pruning; machine-layer content and the machine manifest;
`[settings.*]` values.

**Lock before trust.** The lockfile is part of the consent digest, so it must
be final before you review. Verified: with a server declared and unlocked,
`trust` then `use --write` leaves the project *drifted again* — `use` pins the
server and invalidates the grant you just made. `lock --write` first, and the
same sequence stays trusted. See finding B in §10.

**What it costs when it refuses.** Verified exits: `apply --write` on an
untrusted project → `1`. `use --write` → `1`, "3 targets blocked". `doctor
--ci` with an unresolved secret → `1`. `add skill --write` writes the manifest
and lock but exits `1` because materialization is gated — the manifest write
stands, the delivery does not.

**Non-interactive consent is bound to bytes.** `trust --preview` emits JSON
with a `surface_digest`; the grant is `trust --yes --consented-digest
<digest>` and refuses on any mismatch. `init --plan` / `init --consented-plan`
is the same contract for import. `toolset create --preview` / `--yes
--consented <digest>` is the same for toolsets.

---

## 5. The journeys

Every command verified against the binary.

### First run

```
agentstack init                      # guided: detects CLIs, imports their configs,
                                     # lifts inline tokens to ${REF}, asks where
                                     # values live, previews, applies, verifies
agentstack init --yes                # promptless; stops after the manifest
agentstack init --global             # machine manifest + guard defaults instead
agentstack status                    # where it stands, and the one next step
agentstack x gateway connect --all --write   # register the bridge, once per machine
```

### Add a capability

```
agentstack search <query>                    # catalog, marks what you already have
agentstack add server github --type http \
  --url https://api.githubcopilot.com/mcp/ \
  --header 'Authorization=Bearer ${GH_PAT}'  # previews
agentstack add server ... --write            # writes the manifest
agentstack add skill owner/repo --skill pdf --write
agentstack add from <catalog-or-registry-id> --write
agentstack secret set GH_PAT                 # OS keychain
```

`add` previews by default; `--write` commits. `add server --write` does **not**
lock — see below.

### Review and activate

```
agentstack lock --write              # pin skills + servers  (do this FIRST)
agentstack trust .                   # read the card, answer
agentstack use --write               # activate: servers live, skills materialized
```

Headless, the same three steps:

```
agentstack lock --write
D=$(agentstack trust --preview | jq -r .surface_digest)
agentstack trust . --yes --consented-digest "$D"
agentstack use --write
```

At a terminal, `agentstack yes` is all of it in one review. It refuses without
a TTY and prints the explicit path above — by design.

### Switch context

```
agentstack toolset create backend --server github --skill demo-skill   # asks, then writes
agentstack use --list                # every toolset + readiness
agentstack use backend --write       # switch
agentstack x session start backend   # temporary — everything goes back on end
agentstack x session end             # (--all for every session on the machine)
agentstack x session freeze --name backend-frozen   # pin what actually loaded
```

### Run

```
agentstack run claude-code --plan            # the whole plan, mutates nothing
agentstack run claude-code                   # HOST / PROTECTED — the default
agentstack run claude-code --toolset backend # apply for the run's life, revert after
agentstack run claude-code --prompt "..."    # headless
agentstack x report runs                     # live tracked runs
agentstack x kill <run-id>
agentstack x report run <run-id>             # flight recorder: lifecycle, egress, calls
```

Four postures, and the binary labels each honestly:

| | What it is |
|---|---|
| `--unprotected` | `HOST / ADVISORY`. No trust check, no lock verify, no policy admission. The escape hatch — **interactive only**: it takes neither `--plan` nor `--prompt` (see below). |
| *(default)* | `HOST / PROTECTED`. Trust, strict lock verify, policy admission, frozen grant — all before launch. Not isolation: the harness runs as you, on the host. |
| `--sandbox` | Container, project mounted, HTTPS at the policy proxy. The bridge still permits direct connections that ignore the proxy. Needs `--features sandbox` + Docker. |
| `--lockdown` | Implies `--sandbox`. Internal Docker network, no host route, no internet; the only peer is the egress-proxy sidecar. |

`--plan` and `--prompt` do not accept every posture, and the gaps are intended.
`--plan` previews the pre-launch gate, so `--unprotected` — which switches that
gate off — has nothing to preview; the protected plan launches nothing either
and walks exactly the checks the escape hatch skips. `--prompt` is headless
delivery, and headless delivery lives only in the protected contract
(grant-committed argv, bounded output evidence), so `--unprotected`,
`--sandbox`, and `--lockdown` all refuse it. Net effect: an unattended run is
always a governed one, and dropping protection always has a person at the
terminal.

### Undo

```
agentstack undo                      # recent changes, newest first
agentstack undo --to 2 --write       # back to before change 2
agentstack x restore --last --write  # the same ledger, one step
agentstack x unrender --write        # take back server files apply no longer writes
agentstack x uninstall --write       # the guaranteed exit; --keep-home keeps the ledger
```

The revert is itself recorded. There is no JSON path that performs a revert.

### New machine

```
git clone … && cd …
agentstack up                        # detect CLIs, verify against the lock, render
agentstack trust .                   # a fresh machine has no grant
agentstack secret set <REF>          # what the lock cannot carry
```

Or, carrying secrets yourself:

```
agentstack x export --secrets -o bundle.age
agentstack x import bundle.age
```

### Team

```
agentstack x share <name>            # signed .astack bundle; signing is not a flag
agentstack x receive <bundle>        # staged inert in .agentstack/quarantine/, carded, then you decide
agentstack x publisher trust <key> --label <name>   # a recognized key shortens the card, never skips it
```

A bundle's manifest and lock verify its files; they are not merged. Servers,
toolsets and policy do not cross over.

### CI

```
agentstack install --locked          # fails if resolving would change the lock
agentstack doctor --ci               # nonzero on any finding; implies --deep and --all
```

Or the published action: `uses: Tarekkharsa/agentstack@v0.18.0-rc.2`.

### When something is wrong

```
agentstack doctor                    # what is wired, missing, changed
agentstack doctor --probe            # actually start each stdio server (the one read with side effects)
agentstack x diff                    # manifest vs on-disk configs
agentstack x why <name>              # origin, pin, who said yes, where it is live
agentstack x explain <name>          # provenance, secrets, writes, safety signals
agentstack adopt --write             # keep a hand-edit: pull it back into the manifest
agentstack x optimize                # inert servers, dead grants, narrowing suggestions
```

---

## 6. The rest of the toolbox

`agentstack --help` shows fifteen verbs. `agentstack x` is the other
thirty-seven, and each also runs at its own name.

| Group | Verbs |
|---|---|
| Set up | `up` `adapters` `settings` `self` `completions` |
| Edit | `set` `remove` `install` `lib` `export` `import` |
| Share | `share` `receive` `publisher` |
| Render | `instructions` `session` `diff` `unrender` `uninstall` `delivery` |
| Undo | `restore` |
| Protect | `explain` `why` `guard` `sign` `verify` |
| Run | `kill` `shim` `workflow` `image` `gateway` `mcp` `try` |
| Inspect | `report` `lease` `optimize` `proxy` |

Worth knowing by name:

- **`lib`** — the central library: skills, servers, extensions and hooks you
  keep across projects. `lib link` any folder as a source, `lib sources` for
  precedence and shadowing, `lib sync` to move it between machines as a git
  repo (secrets never travel — server defs stay `${REF}`), `lib trash` because
  every `lib remove*` is recoverable, `lib pack-init` to publish one.
- **`guard`** — machine-level destructive-command guard, wired into every
  hook-capable CLI. `guard install`, then `agentstack guard test rm -rf /`
  exits nonzero. Cooperative, not enforced: a process the harness never routes
  through hooks bypasses it.
- **`workflow`** — declare (`workflow declare`, one transaction, one
  rollback), cost it before running (`workflow explain`), run it
  (`workflow run`, resumable), read the evidence tree (`workflow report`).
  It stops before `trust` on purpose.
- **`try`** — `agentstack try owner/repo --skill pdf | claude`. Stages, scans,
  emits a wrapper prompt. Touches no manifest, lock, or config.
- **`image`** — compose one toolset into a container image carrying the exact
  bytes you reviewed. `${REF}`s stay unresolved; a start-up guard refuses to
  launch without them. Nothing is pushed.
- **`proxy`** / **`report wire`** — localhost relay in front of the Anthropic
  API, and a ranking of what was actually observed.
- **`sign`** / **`verify`** — detached ed25519 over `agentstack.lock`.
- **`mcp`** — run AgentStack itself as an MCP server. `--auto-project` is what
  `gateway connect` registers; `--transparent` advertises upstream tools
  instead of hiding them behind `tools_search`.

Plus twelve fixed, digest-bound actions a graphical panel invokes
(`add-skill-to-profile`, `use-profile`, `library-index`, …). Not part of the
everyday surface.

Machine-readable reads carry contract names — `json-reads-v1`,
`delivery-routing-v1`, `image-plan-v1`, `lease-status-v1` — and `undo --json`
lists the feature flags a client can check.

---

## 7. What does not compress

Three things are genuinely reference material, and shrinking them would make
them lies.

- **The enforcement matrix** — six policy dimensions × five modes, with a
  four-value legend (`enforced` / `coarse` / `unsupported` / `cooperative`)
  and per-cell notes. The cells are the product's honesty. → `docs/ENFORCEMENT.md`
- **The full flag reference** — ~52 verbs, 66 subcommands, 12 panel actions.
  Nobody reads it start to finish; they grep it. → `agentstack --help --all`,
  `docs/reference.md`
- **The adapter support matrix** — which of 13 CLIs supports MCP, skills,
  hooks, extensions, settings, at which scopes. → `docs/adapters.md`

That is the finding: **the product fits on a page; its truth tables do not.**

---

## 8. Closing — the four answers

### Does it fit?

Mostly. This page is **426 lines, 2,843 words** — call it twelve to fifteen
minutes. It covers every capability kind, all 13 adapters, both delivery
lanes, the whole trust gate, nine journeys, all four run postures, and every
verb by name. That is one sitting, but it is at the ceiling of one: past about
500 lines a reader starts scrolling to find things instead of reading, and
this page would then be a reference wearing a page's clothes.

Where the length actually goes: §5 (journeys) is 130 lines and §6 (the rest
of the toolbox) is 60. Those are the two that grow with every verb shipped.
The gate (§4) and the model (§2) are 75 lines together and would not grow.

The docs it replaces total **~5,400 lines** of Markdown across eleven pages
and nine how-tos, before counting the HTML twins.

Honest verdict: the maintainer is right, with two asterisks. The *product*
compresses to a page. The three reference tables in §7 do not, and should not
be tried. And the page only stays a page if §5 and §6 are held to a fixed
budget — a new verb has to earn its line by taking one from somewhere else.

### What had to be left out?

Left out and **it was padding**:

- Nine how-to pages (892 lines). Each is one journey from §5 with prose
  around it. `docs/howto/add-a-server.md` is 77 lines for four commands.
- `docs/choose.md` (76 lines) — "which mode do I need?" is the run-posture
  table in §5, four rows.
- `docs/concepts.md` glossary (419 lines) — every term it defines is defined
  in place here, where it is used.
- The overlap between `start.md`, `concepts.md` and the README, which restate
  the four ideas three times.

Left out and it is a **real need**: the three tables in §7, plus
`docs/workflows.md` (governed workflows have their own threat model and
evidence contract — §6 names the verbs, which is the right depth for a
one-pager), and `docs/migrations.md` (recipes for moving off other tools;
genuinely separate audience).

Borderline: `docs/troubleshooting.md` (877 lines). Most of it is error
messages the binary already prints with a `↳ fix:` pointer. It should shrink
to the residue that the binary cannot say inline, not disappear.

### Which pages does this make redundant?

Name them:

- `docs/howto/` — **all nine**, absorbed into §5.
- `docs/choose.md` — absorbed into the run-posture table.
- `docs/concepts.md` — absorbed into §2 and §4; keep nothing.
- `docs/start.md` — absorbed into §5 "First run"; §1 covers the framing.
- `docs/faq.md` — spot-checked, every entry is answered above or is a
  troubleshooting entry wearing a question mark.
- `docs/integrations.md` — §3 (adapter list) plus §6 (`gateway`, `mcp`,
  `shim`) is the whole of it.

Survives, unchanged: `docs/ENFORCEMENT.md`, `docs/reference.md`,
`docs/adapters.md`, `docs/workflows.md`, `docs/migrations.md`, and a much
smaller `docs/troubleshooting.md`.

### Where did the binary disagree with the docs?

Four, all reproducible.

**A — `status` sends you to `trust` before `lock`, and the ordering costs a
second ceremony.** After `add server --write`, `status` prints `not locked
(never activated)` on one line and `Next: agentstack trust .` on the next.
Following that hint gets you trusted, then `use --write` pins the server, the
lockfile changes, and you are drifted again. The binary's own `agentstack yes`
error message states the correct order (`adopt` → `lock` → `trust` → `use`).
`status` should point at `lock --write` while the project is unlocked.

**B — `add server --write` does not lock; `add skill --write` does.** Same
verb family, opposite behaviour, and B is what makes A bite. Reproduced:

```
init --yes; add server github … --write; add skill <path> --write
trust --yes --consented-digest <D>   → trusted
use --write                          → drifted
```

Lockfile diff after `use --write`: `- server = []` becomes a `[[server]]`
entry for `github`. Inserting `lock --write` before `trust` makes the same
sequence stay trusted.

**C — `up` and `apply --write` disagree on the same condition.** With
everything routed live and no bridge registered, `apply --write` exits `1`
("error: nothing was delivered"); `up` prints "rendering stopped early —
nothing was delivered" and exits `0`. `up` is the documented new-machine and
CI-adjacent command, so a setup that delivered nothing passes silently.

**D — the undo timeline does not list skill materialization.** After `use
--write` reported "wrote skills to 3 locations", `agentstack undo` and
`restore --list` both showed exactly one entry: `init`. `restore --help` says
it reverts "servers, settings, hooks, instructions" — skills are absent from
that list, so this may be intended, but "Undo: take it back" is then a
narrower promise than the verb makes. Either record skill writes, or say in
`undo` that materialized skills are not in the ledger.

---

*Draft. Verified against agentstack 0.18.0-rc.2 in an isolated `HOME`;
no repository state was touched.*
