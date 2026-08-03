# Packaging: a toolset materialized as something you run

> **Status:** Adopted 2026-08-03, for `STRATEGY.md` v3 §"The shape" —
> *"Packaging is self-run materialization. A toolset and its pinned
> capabilities compose into something you run — a Docker image today,
> your-own-account workers later."* This document settles the artifact format,
> which the strategy deliberately left open. It authorizes nothing beyond
> TODO.md item 7; the hosted multi-tenant runner stays a non-goal, and nothing
> here pushes, publishes, or registers anything anywhere.

## The question the strategy left open

The strategy says a toolset composes into "something you run" and names a
Docker image as today's form. It does not say what is *in* the image, what
happens when the image starts, or which of AgentStack's guarantees survive the
trip. Those are the three questions here, and the honest answers are narrower
than "your setup, containerized" would suggest.

## What a package artifact is

**A package artifact is one container image: a runner base image plus exactly
one added layer that carries one toolset's pinned capability bytes.**

It is built by `agentstack image`, on the user's own machine, with the user's
own Docker. It is not pushed. It has no registry coordinates unless the user
gives it some afterwards. Nothing in it phones home.

The added layer lands under a single fixed root, `/agentstack`:

```text
/agentstack/
  image.json          the descriptor: toolset, harness, posture, every member
                      with its digest and provenance, the required secret refs
  entrypoint.sh       fixed script (below) — requires secrets, then execs
  required-secrets    one validated ${REF} NAME per line; never a value
  manifest/
    agentstack.toml   the project manifest bytes, ${REF} placeholders intact
    agentstack.lock   the lock — the pins the image was built from
  servers/<name>.json each selected server's pinned DEFINITION, ${REF} intact
  instructions/<name> package-carried instruction members, pinned bytes
  home/               the image's HOME, holding the harness's own skills
                      directory with the toolset's pinned skill bodies
```

The Dockerfile is generated, never hand-written and never committed: it is the
rendering of an `ImageSpec` (`crates/runtime/src/image.rs`), the same
backend-agnostic shape `SandboxSpec` already plays for a run. Every value that
reaches it — base image, tag, labels, env names, the exec-form argv — is
validated against a conservative charset first and emitted in JSON exec form,
so nothing repository-derived is ever interpolated into a shell (invariant 7).

The image carries four labels — `org.agentstack.toolset`,
`org.agentstack.posture`, `org.agentstack.lock`, `org.agentstack.version` — so
the artifact can be identified by `docker inspect` without unpacking it.

## Which members compose into it

The unit is **one toolset**, and its members are exactly what the lock pins for
that toolset:

- **Skills** the toolset selects, direct or through a selected package.
- **Servers** the toolset fences, direct or through a selected package —
  carried as pinned *definitions*, not as running processes (see below).
- **Instruction members** carried by a selected package.

Nothing else. The project's workspace is not copied, hand-written CLI configs
are not copied, and `[instructions.*]` fragments the manifest declares
project-wide are not compiled into the image — see
[Deliberate limits](#deliberate-limits) for why.

Every member's bytes come from the content-addressed store, addressed by the
digest the lock records, and re-verified against that address at read time.
This is the same rule
[`pinned-serving-and-library-drift.md`](pinned-serving-and-library-drift.md)
sets for the serve and rendered lanes, applied to a third reader: **the live
library directory is never read.** A library that has moved ahead changes
nothing about what an image built today contains.

Skill bodies are laid down as **copies**, never links — a link into the host's
store would be a dangling path inside the image, and copying is what makes the
delivered bytes the reviewed bytes.

## What runs when you run it

`ENTRYPOINT` is `/agentstack/entrypoint.sh`; `CMD` is the toolset's harness
launch binary, taken from the same adapter descriptor field `agentstack run
--sandbox` uses (`detect.bin`). So `docker run <tag>` starts the harness with
the toolset's approved skills already present in its skills directory, and
`docker run <tag> <argv…>` replaces the argv the harness is launched with.

The entrypoint is a **fixed** POSIX script — a compile-time constant, with no
project content interpolated into it at any point. It reads
`/agentstack/required-secrets`, checks each name with `printenv "$name"`
(an argument, never code — no `eval`, ever), and refuses with exit 78 naming
every missing name before it `exec "$@"`s the harness. A packaged image that
is missing a secret does not start.

## How it reuses the sandbox and egress contract

The image is a **runner image**, which is a role the shipped sandbox already
has. `agentstack run --sandbox` launches its container from
`AGENTSTACK_SANDBOX_IMAGE` (defaulting to `agentstack/sandbox:latest`), mounts
the project at `/workspace`, points `HTTPS_PROXY` at the egress proxy, and —
for a trusted run — routes MCP through the host-side gateway. A packaged image
is a drop-in for that variable:

```sh
AGENTSTACK_SANDBOX_IMAGE=<tag> agentstack run claude-code --sandbox --lockdown
```

Three consequences, and they are the whole reuse argument:

1. **No container orchestration is duplicated.** Building an image is the only
   new mechanism. Networks, mounts, the proxy, the sidecar, the internal
   lockdown network, teardown, and the flight recorder are untouched.
2. **No egress policy is duplicated.** The image declares no network anything.
   Egress is decided where it is decided today: by `EgressGuard::decide` inside
   the proxy, from the compiled machine ∩ project ruleset.
3. **The build defaults `FROM` to the same `AGENTSTACK_SANDBOX_IMAGE` value the
   sandbox would have run**, so the packaged image is that image plus the
   toolset, and never a second, divergent notion of "the runner".

The image's `WORKDIR` is `/workspace` — the same mount point
`build_sandbox_spec` uses — so a packaged image behaves identically whether the
workspace is mounted over it or not. The toolset's own bytes deliberately live
under `/agentstack`, **not** under `/workspace`, precisely because a sandbox
run mounts the host workspace over that path and would hide them.

## The posture label, and what it does not promise

The artifact carries a posture label drawn from the shipped vocabulary
(`commands::sandbox::Posture` / `grant::GrantPosture`), never a new word
invented for packaging. The label it carries is **`Posture::Sandbox`**, printed
in its shipped form:

```text
SANDBOX / PROXIED · DIRECT ROUTE OPEN
```

**What it means here, exactly:** this is the posture the artifact is *prepared
for* — the strongest one its contents assume. It is not a claim about any
particular run.

**Posture is a property of the run, never of the image.** An image is inert
bytes; every mechanism in the label is supplied by whoever starts the
container. Specifically:

- **A bare `docker run <tag>` earns none of it.** There is no egress proxy, no
  `HTTPS_PROXY`, no allowlist, no run log, and no gateway. What a bare run has
  is the container boundary itself — which is real, kernel-enforced isolation
  of the filesystem, and nothing more. Calling that "sandboxed" in AgentStack's
  vocabulary would be false, and no surface does.
- **`--lockdown` is stronger and is deliberately not claimed.** The same image
  run under `agentstack run --sandbox --lockdown` gets `LOCKDOWN / ENFORCED ·
  NO DIRECT ROUTE`. The artifact does not carry that label, because
  topological confinement is established by the internal network and the
  sidecar, both of which are the runner's. Understating is safe; overstating is
  not.
- **Everything `ENFORCEMENT.md` says about the `--sandbox` column still
  bounds this.** Host advisory checks are not confinement, recording is not
  prevention, and an allowed destination can still exfiltrate. Packaging adds
  no enforcement of any kind. It changes *where the reviewed bytes are* — not
  what happens to a process that has them.

## Secrets: required at run time, never baked

Invariant 5 is the sharpest constraint on this feature, and it is met
structurally rather than by care at the call site.

- **Nothing in the build resolves a `${REF}`.** The build never constructs a
  secret resolver and never calls one. There is no code path from
  `agentstack image` to the keychain, to varlock, to a `.env` file, or to the
  process environment's secret values.
- **Server definitions are carried verbatim**, `${REF}` placeholders intact,
  from `ResolvedServer::server` — the same structure whose documented contract
  is that placeholders are preserved. They are written as JSON under
  `/agentstack/servers/`, which is data the runner reads, not native harness
  configuration. This is the reason server definitions are *not* rendered into
  the harness's own config inside the image: the one existing path that does
  that (`render_server`) resolves `${REF}` through a `ScopedResolver` and
  writes concrete values to disk, which is correct for a local machine and
  categorically wrong for a distributable artifact.
- **What the image carries instead is the list of NAMES.**
  `/agentstack/required-secrets` holds one `${REF}` name per line, each one
  re-validated against `core::refs::is_ref_name` before it is written; a name
  that fails validation fails the build rather than being written.
- **The run supplies the values, from the running user's own chain.** Under
  `agentstack run --sandbox` they are resolved host-side by the gateway and the
  container receives only an endpoint and a per-run token — resolved values do
  not enter the container at all, exactly as today. Under a bare `docker run`
  they must be in the environment the user passes, and the entrypoint refuses
  to start the harness without them.

One honest note about the entrypoint guard: `agentstack run --sandbox` clears
the image entrypoint (`entrypoint: Some(vec![])`, so the spec's command is
authoritative regardless of base image), which means the guard does not fire on
that path. That is correct rather than a hole — on that path secrets are
resolved host-side and never needed inside the container. The guard exists for
the bare `docker run` path, which is the only one where the container is
expected to hold them.

## Reproducibility: what is claimed, and what is not

**Claimed.** The AgentStack layer is content-determined. Every member is copied
out of the content store by the digest the lock records, verified against that
digest at read time; the descriptor, the required-secret list, the entrypoint,
and the generated Dockerfile are pure functions of the manifest, the lock, and
the flags. Build the same lock twice on two machines and the *bytes AgentStack
puts in the image* are identical, and `image.json` lets anyone check that claim
member by member without running the image.

**Not claimed — and this is the part it would be easy to overstate.**

- **The image is not bit-reproducible.** A Docker build stamps layer metadata
  (creation timestamps, image ids) that vary per build. Two builds of the same
  plan produce different image digests. Nothing here changes that, and no
  surface says otherwise.
- **The base image is not pinned by this feature.** `FROM` defaults to a
  floating tag (`agentstack/sandbox:latest` unless `AGENTSTACK_SANDBOX_IMAGE`
  says otherwise). Whatever that tag resolves to at build time is what you get,
  along with every package its own layers install. Pin it by passing `--from`
  a digest reference if you want that axis closed; AgentStack will not pretend
  it closed it for you.
- **Reproducible content is not reproducible behaviour.** The pinned bytes of a
  skill are prose a model reads, and a pinned server definition names an
  upstream that can change beneath it. `ENFORCEMENT.md` §Servers already states
  this for the ordinary path; packaging inherits it unchanged.

## Failure modes, and what fails closed

The build refuses — before writing a build context and before touching the
Docker daemon — when:

| Condition | Why it refuses |
|---|---|
| The toolset is not declared in the manifest | A missing toolset must never broaden to "everything", the rule `frozen_runtime_servers` already enforces |
| A selected skill has no lock entry | Unpinned content has no reviewed digest to build from; baking it in would be inventing consent |
| A selected skill's pinned snapshot is absent or does not hash to its own name | The approved bytes cannot be produced on this machine — a signal, not a gap to fill |
| A selected server fails to resolve, or its library pin does not match the lock | Same fail-closed set `frozen_runtime_servers` produces for a run |
| A package member's pinned deposit cannot be verified | Identical rule, applied to members |
| The project is not trusted at its current bytes | Baking skill bytes into an image is putting them where an agent reads them; invariant 3 forbids that before the gate passes |
| A `${REF}` name fails `is_ref_name` | A name that cannot be validated cannot be written into the artifact |
| A name, tag, or base image fails the artifact charset | Nothing unvalidated reaches a generated file |

Two refusals are deliberately *not* fail-closed in the same way:

- **The dry run still shows the whole plan when it is unbuildable.** Blockers
  are listed with the member they belong to, and then the command exits
  non-zero. A plan you cannot read is a worse failure than a plan that refuses.
- **A missing Docker daemon is reported, not hidden.** `--write` stages the
  complete build context first; if `docker` is not on PATH or the daemon does
  not answer, the command names which of the two it is, names the staged
  context directory, prints the exact `docker build` line that finishes the
  job, and exits non-zero. Planning and validation never need a daemon at all.

## Invariant walk

**2 — policy only narrows.** Packaging compiles no policy and carries none. The
effective ruleset a packaged run enforces is still `machine ∩ project`,
compiled by `render::ruleset_for` at run time from the machine ceiling and the
project manifest. An image cannot widen it, because an image is not an input to
it.

**3 — untrusted repository content is inert.** The build requires the project
to be trusted at its current bytes and refuses otherwise, so no unreviewed
skill body reaches the layer where an agent would read it. The dry run reads
the lock and the store to *name* members, which is what `status` and the review
card already do for untrusted projects; it resolves no secret, spawns no
server, and contacts nothing.

**4 — pinned byte changes re-gate.** No cache and no partial-trust path is
introduced. Members are read from the content store by the digest the lock
records and re-verified at read time; a member whose bytes moved has moved the
lock, which flips the consent digest, which re-gates through the ordinary
review card before an image can be built at all. An already-built image is a
frozen record of one lock state, and `image.json` plus the
`org.agentstack.lock` label say which.

**5 — secrets never serialize.** Covered in full above. The build has no
resolver, `${REF}` placeholders travel verbatim, only names reach
`required-secrets`, and the run-time guard refuses to start without the values.
The witness
(`no_secret_value_can_reach_the_image_or_the_build_context`) walks every byte
the build writes.

**7 — all repository content is hostile input.** Toolset, member, and server
names are validated by the shipped name contract before they can become path
segments or label values; every string reaching the generated Dockerfile is
charset-validated and emitted in JSON exec form; the entrypoint is a fixed
constant that interpolates nothing and uses `printenv` rather than `eval`; and
the `docker` invocation is argv, never a shell line.

**8 — claims match enforcement.** The artifact's posture label is the shipped
`Posture::Sandbox` string, and the copy states in the same breath that posture
belongs to the run and that a bare `docker run` earns the container boundary
and nothing else. `ENFORCEMENT.md` carries the same statement, so the
authoritative matrix and this document cannot drift apart.

## Deliberate limits

Named here rather than discovered later.

- **`[instructions.*]` fragments are not compiled into the image.** The only
  shipped path that compiles them at global scope merges them into the
  *builder's own* `~/CLAUDE.md`-class file, and shipping that merge would put a
  person's private machine notes into a distributable artifact. Instructions
  are also per-(CLI, model) and rendered-lane by design. Package-carried
  instruction members are still **carried** under `/agentstack/instructions/`
  and named in the descriptor, so nothing is silently dropped — they are
  present and uncompiled, which is exactly what the descriptor says.
- **Hooks and extensions never enter a package artifact.** They are executable
  kinds and keep the full consent ceremony in a package or out of one; an image
  is not a consent surface, and building one is not a review.
- **No image is pushed, tagged remotely, signed, or registered.** `agentstack
  image` produces a local image and stops. Distribution is the user's, on their
  own account, deliberately.
- **The workers target is not built.** The strategy names "your-own-account
  workers later"; this document covers the Docker image only, and the
  `ImageSpec` seam is where a second target would attach.

## See also

- [`../ENFORCEMENT.md`](../ENFORCEMENT.md) — the authoritative posture and
  enforcement matrix, including the §"Packaged images" section this document
  is the design behind.
- [`pinned-serving-and-library-drift.md`](pinned-serving-and-library-drift.md)
  — the pinned-serving rule this feature applies to a third reader.
- [`package-layer.md`](package-layer.md) — a *package* is a library
  composition; a *package artifact* is the image built here. Two nouns, one
  unfortunate word, kept distinct on purpose.
