# t3code integration contract

> **Status:** active product and technical design
>
> **Product direction:** [`../../STRATEGY.md`](../../STRATEGY.md)
>
> **Ordered work:** [`../../TODO.md`](../../TODO.md)
>
> **Scope:** AgentStack CLI contracts plus the t3code server and web client

## Decision

t3code is AgentStack's primary graphical experience and launch channel.
AgentStack will not maintain a separate embedded dashboard.

The integration is intentionally asymmetric:

- **t3code owns presentation, navigation, and user interaction.**
- **The AgentStack CLI owns plans, validation, consent, writes, recovery, and
  enforcement.**

The CLI remains a complete standalone product and automation interface.
t3code makes the same capabilities easier to discover and use; it does not
reimplement them.

## User promise

A developer using t3code should understand AgentStack through four jobs:

1. **Setup** — detect coding tools and import the capabilities already present.
2. **Toolset** — choose the named set needed for this project or task.
3. **Status** — know whether the environment is ready and what to do next.
4. **Undo** — safely reverse an AgentStack-managed change.

The first successful journey is:

```text
open project
    ↓
review detected tools and capabilities
    ↓
apply one recommended setup
    ↓
see Ready
    ↓
launch an agent with the selected toolset
```

It must not require knowledge of locks, policy, gateways, Docker, confinement,
or workflows.

## Progressive disclosure

Safety runs from the beginning, but its vocabulary appears only when relevant.

| Product moment | Primary UI | Deeper detail |
| --- | --- | --- |
| New local project | detected tools, proposed setup, files that will change | adapter and ownership details |
| Normal use | selected toolset, readiness, one next action | pins and delivery mode |
| Unfamiliar repository content | Review this project | content-bound digest and exact declared surface |
| Denied action | what was blocked, what is protected, safe next action | matching rule and enforcement limits |
| User asks for isolation | More protection | gateway, sandbox, lockdown, egress |
| Investigation | activity and posture | raw reports and enforcement matrix |

Rules:

1. Do not show a decision unless its consequence matters now.
2. Apply safe defaults when no user choice is needed.
3. Never show a generic denial. Include the blocked action, reason, protected
   boundary, and exact safe next action.
4. Present one recommended path before alternatives.
5. Keep internal names in a Details view and machine-readable payloads.
6. Simplification may hide vocabulary, never enforcement limits.

Preferred labels:

| Internal term | Beginner label |
| --- | --- |
| profile | Toolset |
| doctor | Status / Check setup |
| session | Use temporarily |
| trust grant | Review this project |
| policy / sandbox / lockdown | More protection |
| restore record | Undo |

## Architecture

```text
t3code web client
      │ typed RPC; no argv or arbitrary path
      ▼
t3code server
      │ resolves workspace identity
      │ negotiates schema/action versions
      │ maps a closed action enum to fixed argv
      ▼
AgentStack CLI
      │ plans, validates, previews, writes, records, restores
      ▼
manifest · native configs · machine state
```

### Browser boundary

The browser may send:

- a known workspace identifier already owned by the t3code server;
- a closed action name;
- typed selections constrained by the prior read response;
- a consent digest returned by the exact preview being approved.

The browser may not send:

- an arbitrary filesystem path;
- command-line arguments;
- an executable or shell string;
- a policy fragment;
- a secret value;
- a request to bypass a guard or machine ceiling.

### Server boundary

The t3code server:

- resolves the workspace root from its own session state;
- finds the AgentStack binary through the approved installation path;
- enforces a timeout and output bound;
- decodes explicit JSON schemas;
- maps actions to fixed argument vectors;
- requests the dedicated AgentStack administrative authorization for sensitive
  writes;
- returns structured errors, never raw upstream terminal output as trusted UI.

### CLI boundary

The CLI repeats every precondition independently. A correct frontend is not
part of a security proof.

Every write remains:

- dry-run or previewable;
- restricted by machine policy;
- bound to the server-resolved project;
- recorded when it changes AgentStack-managed state;
- recoverable where the underlying operation supports recovery.

## Versioned contracts

Every read response begins with:

```json
{
  "schema_version": 1,
  "features": ["init-plan", "profiles-v1", "status-v1", "restore-last"]
}
```

Feature names describe usable end-to-end contracts, not the presence of one
field. t3code must disable an action and show the required upgrade when its
contract is absent.

Compatibility rules:

- Unknown response fields are ignored.
- Missing required fields fail the affected feature closed.
- An unknown schema major disables the panel with an upgrade message.
- A newer UI never guesses flags for an older CLI.
- A newer CLI never assumes an older UI performed a missing review step.

## Read contracts

### Setup plan

Backed by `agentstack init --plan`. It returns:

- coding tools found;
- importable servers, skills, and instructions;
- source/origin for each imported item;
- proposed manifest location;
- proposed native destinations;
- secret reference names, never values;
- unsupported or lossy fields;
- warnings and blockers;
- a stable plan identity if a later write needs content binding.

The UI groups this as Tools found, Capabilities found, Files AgentStack will
manage, and Values still needed.

### Toolsets

Backed by `agentstack use --list --json`. Each row returns:

- stable profile identifier;
- display name;
- selected harness;
- selected servers and skills;
- readiness;
- trust/pin status when relevant;
- whether it is currently active;
- one actionable reason when it cannot start.

The UI says Toolset. Details may say that the stored object is a profile.

### Status

Backed by a stable status/doctor JSON contract. The primary response is:

- overall state: `needs_setup`, `needs_attention`, `ready`, or `active`;
- selected toolset;
- concise findings;
- exactly one recommended next action;
- optional deeper sections.

The first panel view must not dump every doctor section. Advanced and
informational checks stay collapsed unless they block the current action.

### Activity and recovery

Read contracts expose:

- the last AgentStack-managed writes;
- active sessions/runs;
- the exact restore identifier;
- honest evidence coverage labels.

Activity is not prevention and must not be presented as proof that ungoverned
paths were observed.

## Action contract

The first slice permits only:

```text
apply_reviewed_setup
check_status
start_toolset
end_toolset
restore_last_write
grant_reviewed_project
revoke_project_grant
```

Each action has a server-owned mapping to a fixed AgentStack command. Adding an
action requires:

1. a documented user outcome;
2. a stable CLI contract;
3. direct-call tests that bypass the frontend;
4. recovery behavior;
5. an authorization decision;
6. version negotiation.

There is no generic `run_command` action.

## Consent and administrative authority

Project review has two independent halves:

1. **Content consistency.** The preview and `surface_digest` derive from one
   immutable byte snapshot. The grant returns that digest. The CLI refuses a
   missing, wrong, or stale digest.
2. **Human authority.** t3code requires the dedicated `agentstack:admin`
   authorization for trust, machine-guard, workflow-control, or equivalent
   sensitive writes.

The digest does not prove a human looked. Administrative scope does not prove
the bytes stayed unchanged. Both checks are required.

If either half is unavailable, the panel fails closed and gives the exact
terminal or upgrade path.

## Denial experience

A structured denial contains:

```json
{
  "title": "Could not read a protected file",
  "action": "Read project credentials",
  "protected": "Machine secret boundary",
  "reason": "Blocked by the machine filesystem ceiling",
  "next_action": {
    "label": "Use a secret reference instead",
    "kind": "show_help",
    "target": "secret-references"
  },
  "details": {
    "rule": "machine.filesystem.deny",
    "posture": "HOST / PROTECTED"
  }
}
```

The UI renders the first four fields immediately. Rule and posture belong in
Details. A next action that changes authority must use another closed action;
it cannot be a generated command string.

## Delivery plan

### Slice 0 — correctness prerequisites

- Complete immutable consent-snapshot review.
- Complete consent-digest plumbing in t3code.
- Establish `agentstack:admin`.
- Add CLI/UI compatibility failures.

### Slice 1 — launch experience

- Capability negotiation.
- Setup plan.
- Reviewed apply.
- Concise status.
- Restore last write.
- Parity tests against the terminal journey.

### Slice 2 — recurring use

- Toolset picker.
- Start/end temporary activation.
- Active-state recovery when t3code closes or restarts.
- Launch an agent with the selected toolset.

### Experimental — t3code MCP workflow bridge

t3code's MCP may eventually serve as an optional transport for launching and
supervising other coding harnesses from an AgentStack workflow. This is useful
only if it reduces duplicated harness integration while preserving the exact
same authority path.

AgentStack must first resolve, admit, and freeze the child `ExecutionPlan` and
`AuthorityGrant`. The MCP call may receive only a narrow launch request or
opaque reference to that admitted plan. It must not accept browser-supplied
argv, workspace paths, policy, secret values, or authority. The response must
provide a stable child identity, status, cancellation, result, and evidence
linkage.

This is not part of the launch experience. It requires capability negotiation,
fails closed when unavailable or incompatible, and keeps direct CLI child
launch as the reference implementation and fallback. The research and witness
tests are tracked in [`../../TODO.md`](../../TODO.md#t3code-mcp-harness-bridge--research-only).

### Slice 3 — contextual safety

- Review unfamiliar project content.
- Structured denial cards.
- More protection entry point.
- Honest enforcement/posture detail.

### Deferred

- Profile authoring.
- Workflow authoring and supervision.
- Generic policy editing.
- Secret-value entry in the browser.
- Organization administration.

These require evidence from the first two slices and their own narrow designs.

## Acceptance criteria

The integration is ready for launch when:

- a new user can reach Ready from t3code in under five minutes;
- the terminal and t3code produce the same setup plan and resulting files;
- no arbitrary path or argv crosses the browser boundary;
- older/newer CLI and UI combinations fail with useful upgrade guidance;
- every material write previews its scope and has a visible recovery path;
- every denial explains the safe next action;
- normal setup requires no advanced security decision;
- direct RPC tests prove the CLI/server checks still hold without the UI;
- the CLI remains fully usable when t3code is absent.
