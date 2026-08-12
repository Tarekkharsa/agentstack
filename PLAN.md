# AgentStack standalone multi-machine plan

> **Not a work queue.** `TODO.md` remains the only work queue (CLAUDE.md); this
> page is design reference, and where the two disagree `TODO.md` wins.

> Implementation status (2026-08-11): the standalone zero-files core,
> default-toolset connection behavior, first/additional-machine flows,
> supervisor-readable readiness, agent skill, cleanup behavior, and landing
> page are implemented in this worktree. The existing release workflow remains
> the macOS/Linux/Windows distribution gate; no T3 Code source was changed.

## Goal

AgentStack must work completely on its own, across one or many machines, with
any supported coding CLI. Supervisors such as stock T3 Code may launch those
CLIs, but AgentStack must not require T3 Code, a T3 UI, or a custom T3 fork.

The user should define a setup once, reuse it everywhere, keep projects clean,
and understand what is portable versus what intentionally stays local.

## Scope decisions

- Build and change **AgentStack only** for now.
- Do not modify T3 Code or revive the custom AgentStack panel branch.
- Use current upstream T3 Code only as a compatibility test because it already
  launches providers inside local and remote execution environments.
- Take the useful idea from T3 Connect: one user can work across several
  independent machines, and each machine owns its filesystem, processes,
  credentials, and runtime state.
- Keep zero-files delivery as the normal experience for MCP-capable tools.
- Make file rendering an explicit compatibility/offline choice.

## The simple product model

Only three layers should be visible to a normal user:

| Layer | What it contains | How it travels |
|---|---|---|
| Project | Toolsets, project policy, capability references, lockfile | Committed with the repository |
| Personal library | Reusable skills, MCP server definitions, instructions, optional personal defaults | Synced through the user's Git remote |
| Machine | Secret values, trust decisions, installed CLIs, gateway registration, machine policy, audit history | Stays on that machine |

This is the complete multi-machine story:

1. The repository brings project intent.
2. The personal library brings reusable capability definitions.
3. The new machine supplies its own secrets and consent.
4. AgentStack connects the installed coding CLIs and serves the selected
   toolset live.

## Target user experience

### First machine

```bash
agentstack init
agentstack status
```

`init` should provide one guided review that:

- detects installed coding CLIs;
- imports reviewed MCP server entries and supported settings, while existing
  native skills remain untouched until the user explicitly adopts them;
- offers to move reusable definitions into the central library;
- creates a sensible toolset and makes it the project default;
- registers the AgentStack gateway for detected MCP-capable CLIs;
- removes only the legacy entries AgentStack adopted;
- creates and verifies the project lock;
- shows the trust review;
- ends with exactly one remaining action, usually a missing secret.

The user should not need to understand `apply`, `render-locally`, leases, or
native provider config formats.

### Additional machine

The existing `agentstack up` command should become the complete new-machine
path:

```bash
agentstack up --library <git-url>
agentstack up --library <git-url> --write
```

It should:

- clone or sync the personal library;
- detect this machine's installed CLIs;
- register their gateways;
- verify the repository manifest and lock without silently repinning them;
- report missing `${REF}` names without exposing values;
- show the local trust review;
- activate the declared default toolset through live delivery;
- leave no project MCP or skill artifacts for MCP-capable tools.

After the first bootstrap, a bare command is enough:

```bash
agentstack up
agentstack up --write
```

### Daily use

```bash
agentstack status     # one truthful screen and one next action
agentstack doctor     # deeper verification when needed
agentstack undo       # recover an AgentStack-managed write
```

When a project has one trusted default toolset, a new agent connection receives
it automatically. Multiple toolsets produce a short choice rather than an
implicit selection.

## Required AgentStack work

### 1. Create one truthful state model

`status`, `doctor`, delivery previews, toolset listings, MCP control-plane
reads, and future integrations must derive from the same typed state:

```text
Configured       yes/no
Locked           yes/no
Trusted          yes/no/drifted
Library          current/missing/behind/dirty
Gateway          connected/missing/partial
Default toolset  rust/none
Live toolset     rust/none
Delivery         live/files/mixed
Secrets          ready/N missing
```

Fix the contradictions found in the current setup:

- no live lease must never be described as "activated";
- a verified project that already has `rust` must not be told to create a
  toolset;
- `x delivery` must not say skills are live while `use --write` quietly plans
  project skill symlinks;
- every recommended command must match the current delivery mode;
- human prose and JSON must agree.

Add a single journey test that runs our exact migration and asserts every
surface at each state transition.

### 2. Make a default toolset part of the project contract

Add an explicit default, for example:

```toml
default_toolset = "rust"
```

- Trust covers the selected default and its current lock pins.
- A trusted gateway connection automatically opens it inside that connection's
  capability fence.
- Changing the default invalidates trust like any other capability change.
- A running connection keeps its frozen selection until it is restarted.
- With no default and several toolsets, AgentStack presents the names and asks
  for a choice.

### 3. Separate live use from file installation

The current `use --write` wording mixes capability selection with file
materialization. Replace the normal mental model with two explicit operations:

```text
agentstack toolset open rust       live, process-scoped, zero files
agentstack toolset install rust    intentionally materialize supported files
```

Compatibility aliases can remain temporarily, but documentation and next-step
guidance must stop presenting file installation as the zero-files path.

For tools that can receive skills and MCP servers through the gateway, neither
operation should create project capability folders unless the user explicitly
selects installation.

### 4. Turn `init` into the complete first-machine flow

Today the landing page teaches a separate advanced gateway command after
`init`. Move that decision into the guided setup review.

`init` should preview and, after human approval:

- adopt existing configuration;
- create the library-backed project shape;
- connect detected gateways;
- create a default toolset;
- clean adopted legacy entries;
- lock and prepare the trust review.

Keep each write recorded and reversible. The preview must state which files are
changed, which entries are retained, and which empty managed directories will
be removed.

### 5. Turn `up` into the complete additional-machine flow

`up` currently focuses on rendering. Change it to respect routed delivery and
compose the actual new-machine operations:

1. library status/sync;
2. CLI detection;
3. gateway connection;
4. lock verification;
5. machine secret readiness;
6. local trust review;
7. default live toolset readiness.

Add:

- `--library <url>` for first bootstrap;
- `--json` with a stable machine-readable state;
- preview/consent for anything written;
- idempotent recovery after an interrupted bootstrap;
- no automatic push from a new machine.

### 6. Make library sync understandable

Keep Git as the transport, but expose simple state rather than Git knowledge:

```bash
agentstack x lib sync --status
agentstack x lib sync
```

Report:

- current source and remote;
- clean/dirty state;
- ahead/behind counts;
- exact capabilities changed by a pull;
- content scan findings;
- projects whose lock pins now need review.

Library sync must never carry secret values, trust records, audit history,
machine keys, or OS-specific generated files.

### 7. Add a supervisor compatibility contract

AgentStack does not need to know T3 Code internals. It needs a small generic
contract that any supervisor can use:

- discover AgentStack and its version;
- ask for project readiness as JSON;
- start a provider with a selected/default toolset;
- pass optional supervisor, environment, project, thread, and run identifiers
  for audit attribution;
- receive a clear refusal when trust, lock, policy, gateway, library, or secrets
  are incomplete.

Direct CLI use follows the same path. The supervisor metadata is optional and
must not change authorization.

### 8. Simplify the agent-facing skill

The always-loaded core should be short:

1. Read AgentStack status and delivery through the control plane.
2. If delivery is live, never run `use`, `apply`, or `render-locally`.
3. Use the trusted default toolset or ask the user which declared toolset.
4. Browse and load skills dynamically.
5. Change only the manifest or library; never generated provider files.
6. Preview writes and leave trust, lock acceptance, and consent to the human.

Move detailed explanations into loadable references. Include concise recipes
for adding a library capability, changing a toolset, setting a missing secret,
reviewing drift, bootstrapping a machine, and removing legacy configuration.

## Stock T3 Code compatibility

No T3 changes are required for the first implementation.

Stock T3 Code already runs provider CLIs on the environment that owns the
project. AgentStack should work because:

- AgentStack is installed on that environment;
- the gateway is registered in each provider's normal global configuration;
- T3 launches the ordinary provider binary with the project/worktree as its
  working context;
- the gateway discovers the project manifest from that context;
- AgentStack's guard hooks and machine policy remain local to that environment.

Validate this against a clean checkout of current T3 `origin/main`, never the
custom AgentStack UI branch.

Test matrix:

| T3 path | Provider | Expected AgentStack behavior |
|---|---|---|
| Local desktop environment | Codex | Default toolset served live; audit local |
| Local desktop environment | Claude Code | Same project/toolset fence |
| T3 Connect remote environment | Codex | Remote machine library, secrets, trust, and audit used |
| Desktop-managed SSH environment | OpenCode | Remote machine owns all execution and policy |
| Two environments concurrently | Mixed providers | No state or audit attribution crosses machines |
| T3 Full-access provider mode | Supported providers | AgentStack guard remains the pre-tool-use gate |

If a stock T3 launch path prevents AgentStack from discovering the project or
gateway, fix the generic AgentStack supervisor contract first. Only consider a
T3 contribution after AgentStack's standalone behavior is complete and proven.

## Documentation landing-page plan

### Review verdict

The current page looks polished and has a strong headline, but it is not yet a
simple first-time-user explanation.

Problems observed on the live page:

- The install command gives stable `v0.17.1`, while much of the page demonstrates
  prerelease `v0.18.0-rc.2` commands. A user cannot reproduce the advertised
  experience after following the primary installation instruction.
- The page teaches the product as six stages plus four long terminal stories.
  This is too much before the user understands the three portable/local layers.
- The hero promises live skills, while later `use` language and transcripts can
  still imply project skill materialization.
- The page does not explain the additional-machine journey or central-library
  sync.
- `share/receive` reads like the multi-machine solution, but it is a reviewed
  content handoff, not personal library synchronization.
- The animated proof panel is blank during its initial delay and becomes dense,
  small terminal text when it starts.
- On a narrow mobile viewport, the long install URL breaks awkwardly across
  words and dominates the first screen.
- Advanced topics such as guard internals, confinement, sharing signatures, and
  version negotiation compete with the primary setup story.

### New landing-page structure

#### 1. Hero: the result

Suggested direction:

> **Your AI coding setup, on every machine.**
>
> Define MCP servers, skills, and instructions once. AgentStack serves the right
> toolset to Codex, Claude Code, OpenCode, and other coding CLIs—directly or
> through supervisors such as T3 Code—without copying project config files.

Primary CTA: **Set up my first machine**  
Secondary CTA: **Add another machine**

Show one short, immediately visible proof instead of a delayed full transcript:

```text
Project manifest + personal library
                 ↓
      Mac · Linux · remote machine
                 ↓
       Codex · Claude · OpenCode
```

#### 2. How it works: three layers

Show the Project / Personal library / Machine table from this plan. This gives
the user the full model before commands or advanced features.

#### 3. Two short journeys

First machine:

```bash
agentstack init
agentstack status
```

Additional machine:

```bash
agentstack up --library <git-url>
agentstack up --library <git-url> --write
agentstack status
```

Each journey should have one sentence explaining secrets and trust remain local.

#### 4. Daily experience

Use four small cards only:

- **Automatic:** trusted default toolset opens live.
- **Understandable:** status shows one truthful next action.
- **Portable:** repo + library recreate the setup on another machine.
- **Recoverable:** undo removes AgentStack-managed changes safely.

#### 5. Compatibility

State clearly:

- works directly with supported coding CLIs;
- works when those CLIs are launched by stock T3 Code or another supervisor;
- no T3 Code or UI is required;
- file-only tools receive the compatible rendered lane.

#### 6. Safety, briefly

One short section: manifests and locks travel; secrets and trust do not; unknown
repositories stay inert. Link to detailed security and enforcement pages.

Move the six-step climb, long transcripts, guard internals, sandbox details,
sharing signatures, and version-contract discussion to documentation or demos.

### Landing-page acceptance checks

- A new visitor can answer these in ten seconds: what it does, what travels,
  what stays local, and whether it works without T3 Code.
- The install command and every shown command belong to the same available
  release.
- The first-machine and additional-machine paths are executable as printed.
- Zero-files is the default story; rendered files are described as a fallback.
- Desktop and mobile hero layouts show complete commands without broken words.
- The proof content is useful immediately, including with reduced motion or
  JavaScript disabled.
- The page contains one primary CTA per journey and no prerelease disclaimer in
  the main flow.

## MCP 2026-07-28 stateless migration

Implemented on 2026-08-11. AgentStack now has one RMCP-owned protocol boundary
for its local stdio server, authenticated sandbox HTTP bridge, and HTTP/stdio
upstream clients. The 2026-07-28 path is stateless; dated 2025 clients and
servers continue through negotiated legacy lifecycle support.

### Decision: use the official Rust SDK

Use the official `rmcp` crate, starting from stable `3.1.2`, instead of adding
another hand-written protocol era. It has the same Rust 1.88 minimum as this
workspace and ships dated conformance for both `2025-11-25` and
`2026-07-28`. Pin it through `Cargo.lock`; upgrades remain deliberate,
reviewed dependency changes.

Create a small internal `agentstack-mcp` crate at `crates/mcp`. It owns the
`rmcp` dependency and translates between RMCP protocol types and AgentStack's
existing domain services. Do not spread SDK types or macros through trust,
policy, manifest, or recorder crates.

RMCP owns:

- JSON-RPC/MCP models, validation, response envelopes, and error codes;
- protocol-version discovery and modern/legacy lifecycle negotiation;
- stdio framing and Streamable HTTP behavior;
- `server/discover`, per-request `_meta`, standard MCP headers, `resultType`,
  cache hints, MRTR, and sessionless modern requests;
- upstream client lifecycle, using modern discovery first and the legacy
  initialize handshake as fallback.

AgentStack continues to own:

- project discovery, manifests, locks, trust, and consent freshness;
- toolset selection and capability fencing;
- policy compilation and the single dispatch decision;
- secret resolution and per-server authorization;
- namespaced dynamic tool discovery and schema filtering;
- process-group lifecycle, timeouts, concurrency bounds, and fail-closed
  cleanup;
- call audit, run evidence, and user-facing refusal messages.

Use a manual `rmcp::ServerHandler` adapter rather than generating a static tool
server with macros: AgentStack's tool surface is dynamic, trust-gated,
toolset-filtered, and assembled from upstream servers at runtime.

### Dependency boundary

Start with `default-features = false` and enable only the server, client, stdio,
child-process, Streamable HTTP client, and Streamable HTTP server features the
bridge needs. Do not enable OAuth, elicitation, or other protocol surfaces
until AgentStack actually exposes them.

RMCP's built-in HTTP client currently uses `reqwest 0.13`, while AgentStack
directly uses `0.12`. First try upgrading AgentStack's direct dependency so the
release contains one HTTP/TLS stack. If that creates unacceptable adapter or
platform churn, implement RMCP's transport abstraction over the existing HTTP
client temporarily. Do not silently ship two reqwest major versions as the
permanent answer.

Tokio is already in AgentStack's default dependency graph through reqwest, but
the current gateway API is synchronous. Keep the async runtime inside
`agentstack-mcp`; do not force unrelated domain crates to become async merely
to satisfy the SDK.

### Implemented sequence

1. **Freeze today's behavior — complete.** Wire fixtures cover the stdio
   server, HTTP bridge, upstream stdio client, upstream HTTP client, trust
   refusal, toolset fence, secret refusal, and process-tree cleanup.
2. **Introduce the adapter crate — complete.** `crates/mcp` pins RMCP 3.1.2
   with only the required features and maps the
   current control-plane and dynamic upstream tools onto one manual
   `ServerHandler`; protocol SDK types stop at this crate.
3. **Migrate the server side — complete.** Stdio is served through RMCP in
   dual-era mode. A modern client uses `server/discover` and per-request
   metadata; an older CLI
   receives the same initialize/initialized behavior it receives today.
4. **Migrate the sandbox HTTP bridge — complete.** RMCP's
   `StreamableHttpService` replaced the custom protocol dispatcher. It remains
   wrapped by AgentStack's token check,
   body/concurrency limits, and policy-filtered gateway. Modern requests return
   no `Mcp-Session-Id`; legacy session support stays enabled for old clients.
5. **Migrate upstream clients — complete.** RMCP lifecycle `Auto` prefers
   `2026-07-28` and falls back to `2025-11-25`. AgentStack's custom
   process-group termination, bounded stderr diagnostics, deadlines, and
   secret/policy boundary remain around the SDK transport.
6. **Make application state explicit — complete.** The trusted default is
   derived on every modern request. A non-default selection must be
   launch-pinned or use a
   visible, integrity-bound context handle; it must not live only in an MCP
   connection. Modern lease mutations are refused with that guidance.
   Skill-load records remain audit state, not hidden authority.
7. **Replace modern roots discovery — complete.** No server-initiated
   `roots/list` on the modern path. Prefer an explicit project coordinate from
   the launcher, then the already-supported `AGENTSTACK_MANIFEST_DIR`, with cwd
   fallback only where the harness contract makes it reliable. Keep roots on
   the legacy path.
8. **Remove the old wire implementation — complete.** The hand-written live
   lifecycle, session header, SSE parsing, stdio client, and JSON-RPC envelope
   paths are gone. A small raw request helper remains under `cfg(test)` only so
   domain-level unit tests can exercise business dispatch without a socket.

### Acceptance evidence

- Official MCP conformance passes every capability AgentStack advertises:
  legacy initialize 3/3, legacy tools/list 3/3, modern tools/list 3/3, and
  modern stateless behavior 21/21 applicable checks. Four tests in the broad
  stateless fixture require diagnostic tools AgentStack does not expose;
  prompts, resources, OAuth, elicitation, logging, and subscriptions are not
  advertised merely to make an unrelated conformance bucket run.
- Stdio and Streamable HTTP pass modern-only, legacy-only, and automatic
  fallback tests.
- Two consecutive modern HTTP requests can land without a protocol session and
  receive the same trusted default toolset without shared protocol-session
  state.
- Existing adapter registrations remain unchanged: each CLI still invokes the
  same `agentstack mcp` gateway command.
- Old upstream MCP servers initialize and run; modern upstreams receive
  no legacy handshake or session header.
- The MCP-specific trust, lock, policy, secret, toolset-fence, audit, timeout,
  request-size, concurrency, and child-cleanup witness remains green.
- `cargo test --workspace --all-targets --no-fail-fast` passes across the full
  workspace; strict Clippy, formatting, and diff-whitespace checks are clean.
- Dependency review shows one reqwest 0.13 stack and a minimal RMCP feature
  set. The stripped thin-LTO macOS arm64 release binary is 31,608,848 bytes
  (30.14 MiB) in this worktree. A five-run `--version` sample measured one
  1.69 s cold launch and four warm launches below `/usr/bin/time`'s 0.01 s
  resolution. A pre-migration binary was not captured, so no fictional delta
  is reported.

Changing only `PROTOCOL_VERSION` would be incorrect: modern responses require
new wire fields, server-to-client roots are deprecated, and the current lease
store is connection-hidden application state. Compatibility must be selected
per era at the transport boundary while trust, lock, policy, and gateway
dispatch remain the one shared authority path.

## Delivery phases

### Phase 1 — Truthful zero-files core

- shared state model;
- correct status/doctor/delivery/use language;
- default toolset;
- automatic trusted live lease;
- journey regression tests.

### Phase 2 — First and additional machine flows

- complete `init` orchestration;
- routed, zero-files-aware `up`;
- `up --library` bootstrap;
- simple library status/sync;
- per-machine secret and trust guidance.

### Phase 3 — Generic supervisor support

- stable readiness JSON;
- optional run attribution metadata;
- provider launch/binding contract;
- direct CLI and stock T3 compatibility tests.

### Phase 4 — Agent and documentation experience

- short agent-facing core skill;
- loadable operational recipes;
- rebuilt landing page;
- first-machine, second-machine, and stock-T3 walkthroughs.

### Phase 5 — Cross-platform validation

- macOS, Linux, and Windows;
- Codex, Claude Code, OpenCode, and other detected adapters;
- direct provider launches, stock T3 local, T3 Connect remote, and SSH remote;
- offline, stale library, missing secrets, trust drift, and interrupted bootstrap.

## End-to-end acceptance journey

1. On machine A, import existing Codex and Claude MCP configuration once.
2. AgentStack creates a library-backed project, default `rust` toolset, lock,
   gateway registrations, and trust review.
3. Start Codex directly. It receives the `rust` toolset without project MCP or
   skill folders.
4. Clone the repository on machine B and run
   `agentstack up --library <git-url>`, review it, then repeat with `--write`.
5. B receives the same library definitions, reports only its missing secrets
   and trust decision, then becomes ready.
6. Open the same project through unmodified T3 Code on B. The provider receives
   the same default toolset through B's local AgentStack gateway.
7. Run different projects on A and B concurrently. Each uses its own secrets,
   trust, policy, leases, and audit history.
8. Change a library skill on A and sync it. B reports the update and affected
   locks before using new bytes.
9. Uninstall project delivery. AgentStack removes only its artifacts, including
   empty managed parent directories, and preserves user-owned files.

## Definition of done

- AgentStack requires no T3 component and has no T3 runtime dependency.
- A supervisor can use AgentStack through the same public CLI/control-plane
  contracts as a direct user.
- First-machine setup is guided by `init`; additional-machine setup is guided
  by `up`.
- Normal MCP-capable use creates no project capability artifacts.
- Status never says live or activated without a live toolset binding.
- Repositories and libraries are portable; secret values, trust, machine policy,
  keys, and audits remain local.
- Stock T3 Code local and remote provider sessions work without custom UI code.
- The landing page explains the complete model simply and only advertises
  commands available from its primary install path.

## Out of scope for now

- Any custom T3 Code UI or T3-specific RPC.
- Modifying the T3 Code repository.
- Syncing secret values, trust records, or audit data.
- A hosted AgentStack account or cloud control plane.
- Automatically trusting a repository on another machine.
- Replacing Git as the central-library transport.
