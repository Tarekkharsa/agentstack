<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Use with t3code

t3code is AgentStack's primary graphical integration and launch channel. The
goal is to make AgentStack useful where people already start and supervise
Claude Code, Codex, Cursor, and OpenCode—without asking them to learn a second
dashboard or the complete AgentStack command surface.

## What works today

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

## The panel journey

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
4. **Toolset** — choosing a named profile and using it temporarily is the next
   slice; today profiles remain a CLI flow (`agentstack use --list --json` is
   already stable for external pickers).

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

## The integration boundary

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

## Limits

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
