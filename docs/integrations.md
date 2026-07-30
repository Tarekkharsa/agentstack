<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Integrations

The AgentStack CLI is the complete, standalone interface: every plan, write,
consent check, and enforcement decision lives there. An **integration** is an
optional surface that calls those same stable read APIs and fixed actions to
make the product easier to discover and use where you already work. An
integration presents; it never becomes the enforcement boundary, and it never
becomes a second implementation.

Today there is one.

## t3code

t3code is AgentStack's optional graphical companion. The obtainable AgentStack
CLI is the launch channel; as of v0.17.0 the integrated t3code fork has no public
download, so this page documents the compatibility contract for source users
rather than presenting an unavailable app as the way to begin. When a packaged
t3code build becomes public, it can make AgentStack useful where people already
start and supervise coding agents without becoming a second authority system.

### The released CLI serves the whole panel

The half that is public — the CLI — is already complete for this integration.
The panel gates each of its features on a contract the CLI advertises, and the
**published `v0.17.0` release serves every one of them**, so a source-built
t3code needs no locally-built AgentStack behind it:

| Panel capability | Contract | In released v0.17.0 |
| --- | --- | --- |
| Setup — render the plan, apply it | `init-plan`, `apply-setup` | yes |
| Status — one state, one next action | `status-v1`, `doctor-advisories-v1` | yes |
| Undo — revert this project's last write | `restore-last` | yes |
| Toolsets — browse, create, add, activate | `profiles-v1`, `profiles-edit-v1`, `toolset-create-v2` | yes |
| Toolsets — edit membership, rename, delete | `profiles-edit-batch-v1`, `toolset-rename-v1`, `toolset-delete-v1` | yes |
| Use temporarily | `sessions-v1` | yes |
| Review this project | `trust-preview`, `trust-consent` | yes |
| Drift review | `diff-v1`, `diff-ownership-v1` | yes |
| Library remove | `library-remove-v1` | yes |
| Workflow monitor (read-only) | `workflow-observe-v1` | yes |
| Serial-role scheduling warning | `workflow-serial-roles-v1` | yes |
| Startup test — actually start the servers | `doctor-probe-v1` | yes |

Two names in `FEATURES` are deliberately absent from this table.
`json-reads-v1` names the `--json` form of `status`, `search`, `adapters list`
and `session list` — an integrator contract for callers that scrape those
screens, which the panel does not use because it reads the richer payloads
directly. `profiles-edit-v1` covers the digest-bound add verbs, listed above
with the toolset rows.

Check any build yourself — the envelope is part of the read:

```bash
agentstack doctor --json | jq '{schema_version, features}'
```

A panel feature whose contract is absent disables that action with upgrade
guidance; it never guesses or degrades silently.

### What works today

AgentStack already manages the native configuration read by the coding CLIs
t3code launches. Static activation and clean-at-rest sessions therefore apply
to those launches without t3code reimplementing configuration logic.

Run:

```bash
agentstack doctor
```

When t3code is installed, doctor checks the supervisor integration, including
provider guard coverage and home-directory overrides that can move a CLI away
from the configuration AgentStack manages.

For per-session run identity, create a transparent launcher:

```bash
agentstack shim make claude
```

Point the matching t3code provider's binary-path setting at the generated shim.
Each launched session then appears in `agentstack report runs` and receives its
own run report.

### The panel journey

The native t3code panel implements the first launch slice end to end:

1. **Setup** — an uninitialized project shows the coding tools and importable
   capabilities detected by `agentstack init --plan`, and one action applies
   that reviewed plan. The apply is bound to the exact plan shown: if a CLI
   config changes in between, the CLI refuses and asks for a fresh review.
2. **Status** — one state (Ready, Needs attention, or Needs setup) with the
   single recommended next action; the full doctor report stays available as
   the detail layer.
3. **Undo** — the panel shows this project's most recent AgentStack-managed
   write and can revert it, by identity, without touching other projects.
4. **Toolset** — browse the library, add a capability to a named toolset, create
   one, and use it temporarily. The panel negotiates the stable machine
   contracts for toolsets and sessions and disables edits against an
   incompatible CLI.

Reads and actions are version-negotiated: each CLI response names its schema
version and usable contracts, and a mismatched pair disables the affected
action with upgrade guidance instead of guessing.

Safety appears progressively:

- Normal local setup does not start with policy or sandbox configuration.
- Unfamiliar repository content introduces a contextual “Review this project”
  step bound to the exact previewed surface.
- A blocked action explains what was blocked, what is protected, and the exact
  safe next action.
- Gateway, sandbox, and lockdown choices live under a later “More protection”
  path with honest coverage labels.

### The integration boundary

t3code owns presentation. The AgentStack CLI owns decisions and authority.

- Reads use explicit, versioned JSON schemas.
- Workspace identity is resolved by the t3code server, never supplied as an
  arbitrary browser path.
- Writes are a closed enum of actions mapped to fixed CLI arguments.
- The CLI repeats every validation and consent check.
- Secret values never enter the browser payload.
- A frontend bug may break the UI, but it cannot grant more authority.

Trust is the clearest example. A preview returns a digest of the immutable
content snapshot that produced it. A grant action must return that digest, and
the CLI refuses stale or missing consent. The digest proves content
consistency, not human attention; t3code's dedicated `agentstack:admin`
authorization — required for every authority-changing action, granted only to
administrative sessions, never implied by an open browser tab — is the
separate human-authority boundary. Both halves are enforced independently:
read-only status and planning need no administrative authority, and a
frontend bug can break the panel but cannot mint a grant.

### Limits

- There is no public t3code package or supported acquisition path today. The
  CLI journey is complete without it; do not make a launch or onboarding claim
  depend on the private fork.
- t3code injects its own browser-preview MCP endpoint directly into sessions,
  outside native CLI configuration. AgentStack can gate calls on governed
  paths, but the endpoint is not declared in the project manifest or lockfile.
  That endpoint is not currently treated as a governed cross-harness workflow
  launcher. Using t3code MCP for child launch and supervision is a separate
  research item and must preserve AgentStack's admitted execution plan,
  authority, cancellation, and evidence path.
- t3code's most permissive provider modes can disable the coding CLI's own
  approval prompts. AgentStack guard coverage matters more in those sessions;
  doctor reports missing coverage.
- A source-built t3code may keep state in a different location from the
  packaged app, so doctor may not observe that development state.
- Read and write parity across CLI/t3code versions is feature-negotiated.
  Unsupported combinations must fail with an upgrade message, never guess.

The CLI remains a complete standalone interface. t3code makes the same product
easier to discover and use; it does not become a second implementation.
