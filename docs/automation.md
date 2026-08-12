<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Automation contract

Everything AgentStack can tell you, it can tell a program. This page is the
single list: which commands emit JSON, what each body looks like, and which
contract name to check before you depend on one.

If you are driving the CLI from an agent, a script, or a graphical companion
such as t3code, this is the page to read. If you want the human-facing flags,
see [Every command](reference.md).

## The envelope

Every machine-readable **read** wraps its body in the same two fields:

```json
{
  "schema_version": 1,
  "features": ["status-v1", "diff-v1", "json-reads-v1", "…"],
  "…": "the body's own fields, alongside these two"
}
```

- **`schema_version`** changes only when an existing field changes meaning or
  shape. Adding a field or a feature does not bump it. Decode this first: an
  unknown major means *disable and tell the user to upgrade*, never *guess*.
- **`features`** names end-to-end contracts this binary actually serves. A name
  appears only when its full loop works, so gate an affordance on the name
  instead of sniffing for a field.

The two envelope fields are injected into the body object, so a body key and an
envelope key live at the same level. Do not name a body key `schema_version` or
`features`.

Three rules the envelope lets you rely on:

1. **Check the name, not the field.** `"advisories" in body` tells you the
   binary emitted advisories this time; `features.includes("doctor-advisories-v1")`
   tells you it *can*. Only the second is a contract.
2. **A name never changes meaning.** Widening a contract gets a new name — that
   is why `toolset-create-v2` exists next to `profiles-edit-v1`, and why the
   four reads below got `json-reads-v1` rather than being folded into an older
   name.
3. **Negotiation is presentation only.** No enforcement decision reads these
   fields. The CLI re-validates every precondition on every call whether or not
   you negotiated, so a UI that displays the wrong thing cannot cause the wrong
   thing to happen.

## Errors

A refusal is not a JSON body. When a command cannot answer, stdout stays
**empty**, the reason goes to stderr as human text, and the exit code is
non-zero. Parse stdout only when the exit code is `0`.

```console
$ agentstack use --list --json --manifest-dir /var/empty
error: no agentstack manifest in /var/empty
(a project keeps one at .agentstack/agentstack.toml, or agentstack.toml at the repo root)
…
$ echo $?
1
```

### The refusal a headless run meets first: trust

The reads on this page answer for a project in any trust state: `status --json`
and `use --list --json` report `trust: "untrusted"` and still exit `0`. (The one
exception is the read that spawns — `doctor --probe` refuses to start anything
for a project that is not trusted at its current bytes.) **Writes are
different.** For an untrusted or drifted project the trust gate covers five
kinds, and each one refuses in place, names the item, and points at the same
command:

| A headless call to | Refuses to |
| --- | --- |
| `agentstack apply --write` | write native MCP server config, compile instruction fragments, render hooks, render native extensions |
| `agentstack use --write` | materialize skills (and the server half above) |

Every refusal line has the same shape — *refusing to …: project at `<path>` is
not trusted — review and* `agentstack trust .` *before …* — naming the item it
withheld in parentheses. Trust is recorded per project directory and never
copied, so a clone, a second checkout, or a fresh worktree of a project someone
already approved is untrusted at its new path. An unattended pipeline against
one does not silently render a partial setup: it stops, and the fix is a human
review. Budget for it — clone, `agentstack x install --locked`, then the
digest-bound `trust --preview` → `trust --yes --consented-digest` pair on this
page, before any `--write`. (`agentstack trust .` is the interactive form: it
asks one closing question, so it exits nonzero with no terminal to answer it.)

**Gate on the exit code, and know what it counts.** The exit code counts
*blocked writes*, not printed refusals: a blocked pending write exits `1`
(`error: 1 blocked write on 1 target`), while a project whose managed region
already matches prints the same `✗` line and exits `0`, because nothing needed
writing. A pipeline that wants "did the gate refuse anything?" rather than "was
anything left unwritten?" must read the output, not only `$?`.

Pruning, machine-layer content and the machine manifest are outside this gate.

## Reads

Every command below is read-only: it does not write a file, render a config,
resolve a `${REF}` into a value, or start a server — with two deliberate,
named exceptions noted in their rows (`doctor --probe` starts each stdio
server and stops it again; `lease status` runs `/bin/ps` on macOS to read a
process start time).

| Command | Contract name | Body |
| --- | --- | --- |
| `agentstack status --json` | `json-reads-v1` | `version`, `clis_detected`, `manifest`, `project`, `next_action` |
| `agentstack search <q> --json` | `json-reads-v1` | `query`, `results[]` |
| `agentstack x adapters list --json` | `json-reads-v1` | `adapters[]` |
| `agentstack x session list --json` | `json-reads-v1` | `sessions[]` |
| `agentstack doctor --json` | `status-v1` | `state`, `next_action`, `sections`, `errors`, `warnings`, `trust`, `protection` |
| `agentstack doctor --json` | `doctor-advisories-v1` | top-level `advisories` count; section lines may carry `level: "advisory"` |
| `agentstack doctor --json` | `doctor-mode-v1` | top-level `mode` (`static` / `clean-at-rest` / `zero-files`) and `activation` (`locked` / `never_activated`) — the same derived readings `status` prints, so no prose-matching. `activation` answers "was this project ever activated", i.e. does a lockfile exist; it is **not** a liveness reading |
| `agentstack doctor --json` | `doctor-liveness-v1` | top-level `live_state` (`live` / `not_live`), `locked`, `default_toolset`, `live_toolsets[]` — whether the lease registry holds a live record for this project right now. Additive: `activation` keeps its `doctor-mode-v1` values, so gate on this name for the runtime reading |
| `agentstack doctor --json` | `doctor-cli-coverage-v1` | per-CLI coverage — which detected CLIs the current delivery mode actually configures |
| `agentstack status --json` / `doctor --json` | `status-honesty-v1` | `state` never reports ready over unverified coverage — gate on this name before trusting `state: "ready"` |
| `agentstack doctor --probe --json` | `doctor-probe-v1` | top-level `probe` object. **This one spawns**: it starts each stdio server, speaks the MCP `initialize` handshake, and stops it again |
| `agentstack use --list --json` | `profiles-v1` | `path`, `trust`, `profiles[]` with readiness |
| `agentstack use --list --json` | `sessions-v1` | per-entry `active`, plus the top-level `session` object |
| `agentstack x diff --json` | `diff-v1` | `targets[]`, `drifted`, `kept`, `owner_refreshes`, `scope`, `warnings` |
| `agentstack x diff --json` | `diff-ownership-v1` | per-target `managed`, `hand_edited`, `foreign_untracked` |
| `agentstack x diff --json` | `diff-existence-v1` | per-target `existed_before` — splits "never rendered here / file absent" from "the manifest moved ahead of a rendered file" |
| `agentstack x restore --json` | `restore-last` | `entries` (newest first) and `adapter_backups` |
| `agentstack undo --json` | `json-reads-v1` | `entries[]` (newest first) — the same recorded writes `restore --json` lists, keyed for timeline display |
| `agentstack workflow list --json` | `workflow-observe-v1` | `workflows[]` with per-entry trust and lock state |
| `agentstack workflow list --json` | `workflow-serial-roles-v1` | per-entry `serial_roles` |
| `agentstack workflow list --json` / `workflow explain --json` | `workflow-role-selection-v1` | per-entry `role_details[]` — each role's `harness`, `model`, `effort`, `serial`, and any declared value that would not reach the child. `explain` carries the envelope too; it is the deeper per-workflow read and **re-gates on trust** |
| `agentstack workflow runs --json` | `workflow-observe-v1` | `runs[]` from the machine-global runs directory |
| `agentstack x lease status --json` | `lease-status-v1` | `leases[]` — the machine-level runtime lease registry, each row's `liveness` derived at read time from the PID and that process's start time. `unknown` never means live. Writes nothing; on macOS it does run `/bin/ps` per recorded PID to read a start time, because there is no `/proc` to read instead |
| `agentstack x delivery --json` | `delivery-routing-v1` | `default` plus one `harnesses[]` row per targeted CLI with its per-kind `routes[]` (where the bytes go) and that harness's own `bridge_registered`. Decide on those two typed fields; the row's `summary` and `why` are display copy and must never be matched on |
| `agentstack x image --json` | `image-plan-v1` | the packaging plan: every pinned `members[]` entry, `required_secrets` (names only), `blockers`, `buildable` |
| `agentstack status --json` | `library-sources-v1` | `project.shadowed_names[]` — one sentence per capability name more than one linked library source holds. Always present, `[]` when nothing collides |
| `agentstack status --json` | `instruction-channels-v1` | `project.instruction_channels[]` — one row per targeted CLI, including the ones with no instruction channel at all |
| `agentstack status --json` | `package-members-v1` | `project.packages[]` — the effective member set this project pinned, after its overrides. Inserted only when a package is selected |
| `agentstack status --json` | `needs-your-yes-v1` | `project.needs_your_yes` — present only when calls were actually refused here since the last yes. Carries a count and the fix, never a card |
| `agentstack status --json` | `update-offer-v1` | `project.updates` — an offer, never a currency claim: the check is offline, so a missing key is not "up to date" |
| `agentstack init --plan` | `init-plan` | the detection plan, with `plan_digest` |
| `agentstack trust --preview` | `trust-preview` | the full reviewed surface, with `surface_digest` |
| `agentstack trust --preview` | `trust-server-blockers-v1` | known server/executable blockers, each with a `fix` of `agentstack lock --write` or `edit-manifest` |
| `agentstack trust --preview` | `trust-review-card-v1` | the per-item review card a graphical client renders — the first-time surface and, on a re-gate, the changed-lines diff |
| `agentstack trust --preview` | `trust-card-diff-v1` | `review.items[]` and `review.removed[]` — the card itself, structured, with a `change` marker per item |
| `agentstack trust --preview` | `trust-card-groups-v1` | `review.groups[]`, holding **indices** into `review.items`, plus `review.question` — the one closing question. There is no per-group or per-item question, accept, or block, and there never will be |
| `agentstack library-index` | `profiles-edit-v1` | the central-library catalog (skills + servers) |

## Consent-bound actions

These are not reads. Each one previews first, returns a digest over exactly what
it proposes, and then refuses to apply if the inputs moved underneath it. Pass
the digest back to apply.

| Command | Contract name | Notes |
| --- | --- | --- |
| `agentstack init --yes --consented-plan <digest>` | `apply-setup` | refuses when the detected inputs drifted since the plan |
| `agentstack trust --yes --consented-digest <digest>` | `trust-consent` | grants bound to the previewed bytes; refuses stale or missing digests |
| `agentstack add-skill-to-profile` | `profiles-edit-v1` | re-locks and re-renders |
| `agentstack add-server-to-profile` | `profiles-edit-v1` | re-locks and re-renders |
| `agentstack use-profile` | `profiles-edit-v1` | re-locks and re-renders |
| `agentstack create-profile` | `toolset-create-v2` | writes the manifest entry and re-locks, and renders **nothing** — naming a toolset is not activating it |
| `agentstack edit-profile` | `profiles-edit-batch-v1` | one preview + one digest over a batch of membership edits |
| `agentstack toolset rename` | `toolset-rename-v1` | renames the toolset everywhere it appears; memberships kept |
| `agentstack toolset delete` | `toolset-delete-v1` | deletes the toolset; the servers and skills in it stay declared |
| `agentstack set-mode` | `set-mode-v1` — **superseded, refuses** | the Mode axis retired; delivery is routed, `agentstack status` reports it, and `agentstack x uninstall` removes rendered files |
| the managed-`.gitignore` prompt on `apply` / `use --write` | `gitignore-opt-out-v1` | records a durable per-project opt-out from the managed block |
| `agentstack remove-from-library` | `library-remove-v1` | machine-wide, not project; recoverable from `lib/.trash` |
| `agentstack remove-capability` | `manifest-remove-v1` | removes a project definition and memberships, then re-locks and re-renders; library untouched |
| `agentstack x restore --last --write` | `restore-last` | undoes the newest recorded write |

`profile` in these command and contract names is the older spelling of
**toolset**; they name the same object.

## Payload shapes

### `status --json`

The orientation screen, keyed. Branch on `project`: it is `null` whenever the
manifest is absent or unreadable, and `manifest.error` says which.

```json
{
  "version": "0.18.0",
  "clis_detected": ["Claude Code", "Codex CLI", "Gemini CLI"],
  "manifest": {
    "path": "/repo/.agentstack/agentstack.toml",
    "present": true,
    "loaded": true,
    "error": null
  },
  "project": {
    "servers": 2,
    "skills": 0,
    "targets": { "pinned": [], "fanout": 6 },
    "toolsets": ["dev", "writing"],
    "session": {
      "profile": "dev",
      "started_unix": 1785179964,
      "age_seconds": 27,
      "abandoned": false
    },
    "locked": true,
    "trust": "trusted",
    "trust_relevant": false,
    "mode": "static",
    "gateway_connected": false,
    "rendered": true,
    "secrets": { "referenced": 1, "unresolved": ["NOTION_TOKEN"] }
  },
  "next_action": {
    "command": "agentstack doctor",
    "why": "verify the wiring — every warning names its fix"
  }
}
```

Field notes:

- `targets.pinned` is `[targets].default`; `targets.fanout` is how many detected
  CLIs a command would reach when nothing is pinned. An empty `pinned` with a
  `fanout` of 6 is *"nothing pinned, six CLIs detected"* — one number cannot say
  that.
- `trust` is `"trusted"`, `"drifted"`, or `"untrusted"` — the same three values
  `use --list --json` uses.
- `trust_relevant` is a **prompting hint, not a capability reading**. It is
  `true` when a bridge is registered for a harness or the derived `mode` is
  `zero-files` / `clean-at-rest` — the states where an untrusted project is
  served nothing at all, so `trust .` is the one next step worth pushing. It is
  `false` for a static project with no bridge. That `false` does **not** mean
  trusting buys nothing there: the trust gate refuses MCP server config,
  instruction fragments, hooks, extensions and skill materialization in every
  mode (see [Errors](#errors)), so an untrusted static project still cannot
  render. Use `trust` — the three-value state — for "can this project write?",
  and `trust_relevant` only to decide how loudly to ask.
- `mode` is `"static"`, `"clean-at-rest"`, or `"zero-files"`.
- `secrets` carries a **count** and the **names** that resolve from no layer.
  A value never appears. `null` means the reading was not taken.
- `next_action` is the one step to surface. It is never a command that would
  refuse. On `status --json` it is an object; on `doctor --json` it is a bare
  command string **or `null`** when no step is runnable verbatim, so a consumer
  must handle null there.

### `search --json`

```json
{
  "query": "notion",
  "results": [
    {
      "name": "smithery-notion",
      "id": "ai.smithery/smithery-notion",
      "description": "A Notion workspace is a collaborative environment…",
      "source": "registry",
      "kind": "server",
      "details": null,
      "in_manifest": false,
      "trust": {
        "namespaced": true,
        "runs_code": false,
        "needs_secret": true
      },
      "add_command": "agentstack add from ai.smithery/smithery-notion"
    }
  ]
}
```

Field notes:

- `kind` is `"server"`, `"skill"`, `"pack"`, `"extension"`, or `"hook"`, and it
  decides the shape of `details`: `null` for a server, `{server, skills,
  instructions}` for a pack, `{path, git}` for a skill, `{target}` for an
  extension, `{event, matcher}` for a hook.
- `trust` is three booleans, not a rendered sentence. `runs_code` is the one to
  filter on; for `kind: "extension"` it means in-process code at full user
  permission, ungoverned at runtime.
- `add_command` is `null` when there is nothing to offer — either
  `in_manifest` is already true, or it is an extension, which is referenced by
  name in `[extensions.*]` rather than added.
- Descriptions ship whole. The 70-column truncation is for a terminal.
- This read reaches the **network**: it queries the official MCP Registry
  alongside your linked library sources and the embedded catalog. It writes nothing.

### `adapters list --json`

```json
{
  "adapters": [
    {
      "id": "claude-code",
      "display": "Claude Code",
      "installed": true,
      "config_present": false,
      "status": "installed",
      "origin": "built-in"
    }
  ]
}
```

Field notes:

- `status` is `"installed"`, `"config_only"` (a config exists but the binary is
  not on PATH), or `"not_detected"`. `installed` wins over `config_present`.
- `origin` is `"built-in"`, `"user"`, or `"user-override"`. The last means a
  descriptor in `~/.agentstack/adapters/` is shadowing a built-in, so the
  behaviour you see is that file's.

### `session list --json`

```json
{
  "sessions": [
    {
      "dir": "/repo/.agentstack",
      "profile": "dev",
      "scope": "project",
      "started_unix": 1785179964,
      "age_seconds": 14,
      "abandoned": false
    }
  ]
}
```

Field notes:

- `abandoned` is the CLI's judgment, not a threshold to re-invent. It is the
  state a supervising UI died in — offer `agentstack x session end`, which
  restores every file the session touched.
- Both `started_unix` and `age_seconds` ship: a poller wants the stable start
  time, a one-shot caller wants the age.

### `delivery --json`

```json
{
  "default": "automatic",
  "harnesses": [
    {
      "id": "claude-code",
      "display": "Claude Code",
      "mcp_capable": true,
      "render_locally": false,
      "override": "none",
      "bridge_registered": false,
      "summary": "skills + MCP servers planned live (not connected) · house rules + settings + hooks written to files",
      "routes": [
        {
          "kind": "servers",
          "lane": "dynamic",
          "why": "the live channel here can carry it on demand",
          "full_ceremony": false
        }
      ]
    }
  ]
}
```

`routes` carries one row per capability kind — skills, servers, instructions,
settings, hooks — trimmed to one here.

Field notes:

- `lane` is the routing — where the bytes for that kind go. It is **not** an
  activation reading: `dynamic` does not say a lease is open, a bridge is
  registered, or the project is trusted.
- `bridge_registered` is **this harness's own** bridge state, never a
  project-wide any-of: one connected CLI delivers nothing to the harnesses that
  have no bridge. Branch on it whenever you would otherwise be tempted to read
  liveness out of the prose. It is still not a full activation reading —
  `lease-status-v1`, `doctor-cli-coverage-v1` and the trust surfaces answer the
  rest, each with its own limits.
- `summary` and `why` are **display copy, and both are conjugated by
  `bridge_registered`**. With no bridge a live route's `why` gives the
  rationale ("the live channel here can carry it on demand") instead of a
  delivery claim, and `summary` says "planned live (not connected)". Neither
  field's shape moved, so this stayed `delivery-routing-v1` — which is exactly
  why **neither may be matched on**: the same routing reads two ways.
- `full_ceremony` says a kind is executable (hooks, extensions), never that a
  ceremony has happened.

### `edit-profile --preview` (`profiles-edit-batch-v1`)

The one membership verb that can also take things *out* of a toolset, and the
only one whose cost does not scale with the number of changes: every add and
every removal lands as one manifest write under one `consent_digest`, followed
by a single re-lock and re-render.

```json
{
  "profile": "backend",
  "add_skills": ["rust-testing"],
  "add_servers": [],
  "remove_skills": [],
  "remove_servers": ["github"],
  "skills": ["rust-testing"],
  "servers": [],
  "empties_toolset": false,
  "action": "edit-profile",
  "consent_digest": "sha256:b428c87f…2111d8",
  "note": "Review, then apply with --yes --consented sha256:b428c87f…2111d8…",
  "schema_version": 1,
  "features": ["…"]
}
```

Field notes:

- `add_*` / `remove_*` echo the requested deltas; **`skills` and `servers` are
  the RESULT** — the membership the toolset would have after the batch. A panel
  draws an end state, so the digest binds the picture the reader was looking at,
  not the list of operations that implies it.
- `empties_toolset` marks the case where the batch would leave nothing behind.
  Emptying a toolset is allowed but is not a no-op: an empty toolset resolves to
  nothing, so activating it serves nothing. Say so rather than presenting it as
  an ordinary edit.
- Apply with `--yes --consented <consent_digest>`; the verb refuses if the
  manifest moved underneath the preview.

## JSON that is not part of this contract

Some commands emit JSON that is evidence or analysis rather than control-plane
state. These carry **no envelope** and **no feature name**, and their shape may
change without a `schema_version` bump. Treat them as reports, not APIs:

- `agentstack x explain <name> --json`
- `agentstack x report run --json`, `report runs --json`, `report calls --json`,
  `report wire --json`
- `agentstack x optimize --json`
- `agentstack workflow report --json` (`workflow explain --json` left this list
  when it gained the envelope and `workflow-role-selection-v1`)

The append-only evidence files are the same kind of thing: JSON Lines with no
envelope and no feature name, self-describing per row, and free to gain
variants. Three moved recently and are worth knowing if you parse them:

- `~/.agentstack/runs/<id>/events.jsonl` — each row carries its own `event`
  tag. Toolset-fence refusals arrive as `"event": "fence_refused"` (server,
  tool, the `toolset` that would expose it, and the reason), deliberately
  **not** as a `tool_call` with a denied outcome, so a refusal never inflates
  the count of calls a run made. `agentstack x report run <id>` prints them in
  their own **Fence refusals** section.
- `~/.agentstack/audit/calls.jsonl` — the guard's three fail-closed *system*
  refusals are filed under synthetic `system: <tag>` subjects
  (`system: machine-config-unreadable`, `system: machine-policy-unavailable`,
  `system: hook-payload-unreadable`) with no project. Rule denials name the
  call instead, under one of exactly four machine-authored prefixes:
  `bash: `, `read: `, `write: `, `other`. Nothing derived from a hook payload
  can produce the `system: ` prefix, so the two kinds cannot be confused.
- `~/.agentstack/audit/trust.jsonl` — one identity-only row per trust-store
  mutation. The action set is `grant`, `regrant`, `repin`, `revoke`, `decide`
  and `undecide`; the last two record and withdraw a standing re-gate answer
  and re-pin nothing, carrying the digest the entry already stood on.

What those files prove about enforcement — recorded is not prevented — is
[the enforcement matrix](ENFORCEMENT.md)'s question, not this page's.

## Guarantees

- **Reads never write.** Nothing on the reads table creates, edits, or deletes a
  file. The one command that starts a process (`doctor --probe`) says so, has
  its own contract name, and refuses to spawn anything for a project that is not
  trusted at its current bytes.
- **`--json` changes rendering, not behaviour.** The flag never causes a
  different reading, a different gate, or a different side effect.
- **Secrets never serialize.** Payloads carry `${REF}` names and resolution
  counts. A secret value is not in any body on this page.
- **Repository content is sanitized on the way out.** Names, descriptions, and
  paths that originate in a manifest, a registry response, or the central
  library pass through an escape stripper before they reach you, so a hostile
  string cannot repaint your terminal or your UI.
- **Removing a feature name is a breaking change.** It requires a
  `schema_version` bump, so a name you gate on today keeps working until that
  number moves.
