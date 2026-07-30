<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Automation contract

Everything AgentStack can tell you, it can tell a program. This page is the
single list: which commands emit JSON, what each body looks like, and which
contract name to check before you depend on one.

If you are driving the CLI from an agent, a script, or a graphical companion
such as t3code, this is the page to read. If you want the human-facing flags,
see [Every command](reference.html).

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
$ echo $?
1
```

## Reads

Every command below is read-only: it does not write a file, render a config,
resolve a `${REF}` into a value, or start a process — with one deliberate,
named exception noted in its row.

| Command | Contract name | Body |
| --- | --- | --- |
| `agentstack status --json` | `json-reads-v1` | `version`, `clis_detected`, `manifest`, `project`, `next_action` |
| `agentstack search <q> --json` | `json-reads-v1` | `query`, `results[]` |
| `agentstack adapters list --json` | `json-reads-v1` | `adapters[]` |
| `agentstack session list --json` | `json-reads-v1` | `sessions[]` |
| `agentstack doctor --json` | `status-v1` | `state`, `next_action`, `sections`, `errors`, `warnings`, `trust`, `protection` |
| `agentstack doctor --json` | `doctor-advisories-v1` | top-level `advisories` count; section lines may carry `level: "advisory"` |
| `agentstack doctor --probe --json` | `doctor-probe-v1` | top-level `probe` object. **This one spawns**: it starts each stdio server, speaks the MCP `initialize` handshake, and stops it again |
| `agentstack use --list --json` | `profiles-v1` | `path`, `trust`, `profiles[]` with readiness |
| `agentstack use --list --json` | `sessions-v1` | per-entry `active`, plus the top-level `session` object |
| `agentstack diff --json` | `diff-v1` | `targets[]`, `drifted`, `kept`, `owner_refreshes`, `scope`, `warnings` |
| `agentstack diff --json` | `diff-ownership-v1` | per-target `managed`, `hand_edited`, `foreign_untracked` |
| `agentstack restore --json` | `restore-last` | `entries` (newest first) and `adapter_backups` |
| `agentstack workflow list --json` | `workflow-observe-v1` | `workflows[]` with per-entry trust and lock state |
| `agentstack workflow list --json` | `workflow-serial-roles-v1` | per-entry `serial_roles` |
| `agentstack workflow runs --json` | `workflow-observe-v1` | `runs[]` from the machine-global runs directory |
| `agentstack init --plan` | `init-plan` | the detection plan, with `plan_digest` |
| `agentstack trust --preview` | `trust-preview` | the full reviewed surface, with `surface_digest` |
| `agentstack trust --preview` | `trust-server-blockers-v1` | known server/executable blockers, each with a `fix` of `agentstack lock` or `edit-manifest` |
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
| `agentstack remove-from-library` | `library-remove-v1` | machine-wide, not project; recoverable from `lib/.trash` |
| `agentstack remove-capability` | `manifest-remove-v1` | removes a project definition and memberships, then re-locks and re-renders; library untouched |
| `agentstack restore --last --write` | `restore-last` | undoes the newest recorded write |

## Payload shapes

### `status --json`

The orientation screen, keyed. Branch on `project`: it is `null` whenever the
manifest is absent or unreadable, and `manifest.error` says which.

```json
{
  "version": "0.17.0",
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
- `trust_relevant` says whether trusting would change what this project can do
  here. It is `false` for a static project with no bridge, whose configs render
  either way. Do not push a user toward a review that buys them nothing.
- `mode` is `"static"`, `"clean-at-rest"`, or `"zero-files"`.
- `secrets` carries a **count** and the **names** that resolve from no layer.
  A value never appears. `null` means the reading was not taken.
- `next_action` is the one step to surface. It is never a command that would
  refuse.

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
  alongside your central library and the embedded catalog. It writes nothing.

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
  state a supervising UI died in — offer `agentstack session end`, which
  restores every file the session touched.
- Both `started_unix` and `age_seconds` ship: a poller wants the stable start
  time, a one-shot caller wants the age.

## JSON that is not part of this contract

Some commands emit JSON that is evidence or analysis rather than control-plane
state. These carry **no envelope** and **no feature name**, and their shape may
change without a `schema_version` bump. Treat them as reports, not APIs:

- `agentstack explain <name> --json`
- `agentstack report run --json`, `report runs --json`, `report calls --json`,
  `report wire --json`
- `agentstack optimize --json`
- `agentstack workflow report --json`, `workflow explain --json`

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
