# MCP 2026-07-28 and Agent Skills — findings and open questions

> **Status:** research notes. Not adopted direction, not a plan, not a queue item.
> Nothing here authorizes work. `TODO.md` stays the only ordered queue.
>
> **Written:** 2026-08-03, from a session that reviewed the MCP 2026-07-28
> release candidate and the Agent Skills specification against this codebase.
>
> **Purpose:** so a later session does not have to re-derive this. Every claim
> below carries its evidence. Claims that were checked and found wrong are kept
> in §6 on purpose, so they are not re-derived either.

---

## 1. Confidence key

Each finding is marked:

- **[verified]** — read in the primary specification text or in this repository's
  source, with the quote or the line reference given here.
- **[reported]** — from a blog post or announcement only. Directionally right,
  details unconfirmed.
- **[open]** — a question this session could not answer.

Do not promote a `[reported]` item to a decision without reading the source.

---

## 2. What changed in MCP 2026-07-28

Primary sources:

- Release candidate announcement — <https://blog.modelcontextprotocol.io/posts/2026-07-28-release-candidate/>
- Tools — <https://modelcontextprotocol.io/specification/2026-07-28/server/tools>
- Caching — <https://modelcontextprotocol.io/specification/2026-07-28/server/utilities/caching>
- Security best practices — <https://modelcontextprotocol.io/specification/2026-07-28/basic/security_best_practices>

### 2.1 Statelessness [verified]

The `initialize` / `initialized` handshake is removed. The `Mcp-Session-Id`
header and the protocol-level session are removed. Protocol version, client
info, and client capabilities now travel in `_meta` on **every** request.

The tools page states it directly:

> Every request **MUST** include the required `_meta` fields.

Servers that need state across calls mint an explicit handle and receive it back
as an ordinary tool argument. The specification is candid about where that state
now lives:

> The model is responsible for carrying `basket_id` forward.

A new attack class, "State Handle Hijacking", is named and mitigated:
servers **MUST NOT** treat possession of a handle as authentication, and
**SHOULD** bind handles server-side to the authenticated user. For
unauthenticated servers the specification concedes the handle "is necessarily
a bearer token".

### 2.2 Transport headers [verified]

Streamable HTTP now requires `Mcp-Method` and `Mcp-Name` headers so
intermediaries can route without reading the body.

### 2.3 `x-mcp-header` [verified] — see §4.2

A **server-authored** JSON Schema may mark a tool parameter with
`x-mcp-header`. The **client** then copies that argument value into an
`Mcp-Param-{name}` HTTP header, visible to every intermediary on the path.

Client duties are syntactic only. The client **MUST** reject a tool whose
header name is empty, malformed, non-unique, or on a non-primitive parameter.
There is no content duty, because the client cannot know which argument holds
a secret. The only protection given is:

> Server developers **SHOULD NOT** mark sensitive parameters (passwords, API
> keys, tokens, PII) with `x-mcp-header`, as header values are visible to
> network intermediaries.

### 2.4 Caching without validation [verified] — see §5.1

`tools/list`, `prompts/list`, `resources/list`, `resources/read`,
`resources/templates/list`, and `server/discover` **MUST** carry `ttlMs` and
`cacheScope` on `resultType: "complete"` results.

`cacheScope` is normative and careful. `"private"` means:

> Cached responses **MAY** be reused for the same authorization context. Caches
> **MUST NOT** be shared across authorization contexts (e.g. a different access
> token requires a different cache).

There is **no** `ETag`, digest, or conditional re-fetch. The design copies HTTP
`Cache-Control` and omits the validator half. A client cannot ask "has this
changed?" — it can only re-fetch in full.

The tools page separately requires the list to be stable:

> This set … **MUST NOT** vary per-connection or as a side effect of other
> requests on the connection. The set **MAY** vary by the authorization
> presented on the request.

So the stability a digest would need is already required. Adding one field
would complete it.

### 2.5 Deprecations [verified]

Roots, Sampling, and Logging are deprecated. Annotation-only: they keep working
in this release and in every version published within a year of it. Stated
replacements:

| Deprecated | Replacement given |
|---|---|
| Roots | Tool parameters, resource URIs, or server configuration |
| Sampling | Direct integration with LLM provider APIs |
| Logging | `stderr` for stdio; OpenTelemetry for structured observability |

**Roots matters to us directly — see §4.1.**

### 2.6 Extensions [reported]

Reverse-DNS IDs, negotiated through an `extensions` map on capabilities, in
separate `ext-*` repositories with delegated maintainers, versioned
independently of the specification. A new Extensions Track in the SEP process.
Two official extensions: MCP Apps (sandboxed iframe UI) and Tasks (moved out of
experimental core).

Not read in full. Out of scope for us — see §7.

### 2.7 Authorization [verified]

The security best practices document is extensive and normative: confused
deputy (the OAuth proxy variety), token passthrough, SSRF, local server
compromise, OAuth URL validation, mix-up attacks, localhost redirect
impersonation, scope minimization. Mostly MUST-level with concrete mechanisms.

---

## 3. Agent Skills — what the specification actually contains

Primary source: <https://agentskills.io/specification>

Frontmatter, complete [verified]:

| Field | Required | Note |
|---|---|---|
| `name` | Yes | Max 64 chars, lowercase, must match parent directory name |
| `description` | Yes | Max 1024 chars |
| `license` | No | License name or bundled file reference |
| `compatibility` | No | Max 500 chars. **Free prose.** |
| `metadata` | No | Arbitrary string→string map |
| `allowed-tools` | No | Space-separated string. **Experimental.** |

Observations that matter to us:

- **`compatibility` is prose, not a contract.** The specification's own example
  is `Requires git, docker, jq, and access to the internet`. No resolver can act
  on it. The slot for a dependency contract exists and was filled with text.
- **Version is a convention inside a free-form map.** The specification's
  example puts `version: "1.0"` under `metadata`. Untyped, unvalidated, not
  comparable.
- **`allowed-tools` is experimental and untyped.** The example value is
  `Bash(git:*) Bash(jq:*) Read` — one vendor's syntax. "Support for this field
  may vary between agent implementations." Silent ignore and hard fail are both
  conformant.
- **No integrity field.** No hash, digest, or signature anywhere in the format.
- **No security language at all.** No security section. No RFC 2119 keywords.
  On `scripts/` the specification says only: "Contains executable code that
  agents can run."

Progressive disclosure is specified precisely: ~100 tokens of `name` +
`description` at startup, `SKILL.md` body under 5000 tokens recommended on
activation, referenced files on demand.

---

## 4. Impact on this repository

All line references verified by reading the source on 2026-08-03.

### 4.1 We speak the protocol that was removed

| Site | What it does |
|---|---|
| `crates/cli/src/gateway_http.rs:12` | Module doc: "an `Mcp-Session-Id` header on every response" |
| `crates/cli/src/gateway_http.rs:211` | Handles `initialize`; defaults `protocolVersion` to `"2025-03-26"` |
| `crates/cli/src/gateway_http.rs:297` | Emits the `Mcp-Session-Id` response header |
| `crates/cli/src/gateway.rs:396`, `:404` | Sends and reads `Mcp-Session-Id` upstream |
| `crates/cli/src/mcp.rs:84`, `:151` | Same, on the other client path |
| `crates/cli/src/mcp_server.rs:291` | `"initialize" => auto.note_client_capabilities(&req)` |
| `crates/cli/src/mcp_server.rs:292` | `"notifications/initialized" => …` sends `auto.roots_request()` |
| `crates/cli/src/mcp_server.rs:~299` | `auto.client_has_roots` gates the transparent `tools/list` path |

Test fixtures pin `2025-03-26` and `2025-06-18` throughout
(`sandbox_lockdown.rs:264`, `trust_at_dispatch.rs:51`, `lease_registry.rs:47`,
`yes_on_lease_path.rs:582`, `codemode.rs:38`, `execution.rs:909`).

Three consequences, in order of depth:

1. **Capability learning changes shape.** `note_client_capabilities` hangs off
   a message that no longer arrives. The fact moves from once-per-connection to
   once-per-request `_meta`. Anything caching a per-connection decision from it
   needs review — this is not a rename.
2. **The roots path loses both its legs.** Automatic project discovery depends
   on `notifications/initialized` (removed) to send `roots_request()`, and on
   Roots itself (deprecated, replacement: "tool parameters, resource URIs, or
   server configuration"). The `cwd`-walk-up ladder already exists as the
   fallback for roots-incapable clients. Under 2026-07-28 it becomes the only
   path. Whether that is a loss of accuracy is **[open]**.
3. **A closed hole may have moved.** `crates/cli/src/delivery.rs:13` records the
   analysis that MCP's `initialize` result can carry an instruction, and that we
   handle it. There is no `initialize` result in 2026-07-28. Either the surface
   moved to `server/discover`, or that comment is now stale text that will
   mislead a reader. Invariant 8 applies to our own comments.

**Timing:** the old versions keep working — the deprecations are annotation-only
with a ≥12-month window, and nothing forces a cutover date. But every server we
proxy will migrate, and we cannot proxy what we cannot parse.

### 4.2 `x-mcp-header` versus invariant 5 — the open risk

Invariant 5: secrets never serialize; manifests carry `${REF}`; resolution
happens at call time.

`x-mcp-header` creates a path where a resolved secret leaves in an HTTP header
that intermediaries read, because an **upstream server's schema** asked for it.
The leak is not in the manifest. It is at dispatch, in a header, chosen by the
untrusted party.

**[open] — the question to answer:** when we resolve a secret into a tool
argument and the upstream schema marks that parameter with `x-mcp-header`, does
the value pass through egress policy unread?

Relevant, not yet read: `crates/egress/src/proxy.rs`, `crates/egress/src/sni.rs`.

If the answer is "unread", the shape of the leak is one our current design may
not cover. If we already strip or inspect unknown `Mcp-Param-*` headers, this
is closed and the note can be deleted.

### 4.3 Skills — where we already fill the gap

Nothing to change. Recorded because it is evidence for the current direction,
not a lane:

- `compatibility` (prose) and `allowed-tools` (experimental, untyped) are the
  slots our manifest fills with a checkable contract.
- The format has no integrity field. Our lockfile does.
- Observed live in this session: loading `think-again` returned
  `library skill 'think-again' is not pinned in agentstack.lock — run
  `agentstack lock` to pin it`. Our tooling detects the condition the format
  has no field for.
- Also observed: a changed manifest or lockfile correctly refused to proxy and
  named the human step (`agentstack trust <path>`). Invariant 4 working.

---

## 5. Design questions, not tasks

### 5.1 Pinning versus TTL [open]

We pin bytes and re-gate on change. 2026-07-28 says a tool list is cacheable
with `ttlMs`, **MAY** change over time, and **MAY** vary by the authorization on
the request.

For a remote HTTP MCP server, what does the lockfile pin?

- Pin the tool list → a legitimate TTL-driven change fires a re-gate the user
  cannot distinguish from an attack.
- Pin only the endpoint → the tool-definition-change-after-approval attack stays
  open.

The specification gives no help: it copied `Cache-Control` and omitted `ETag`.
We already own the missing half. The answer is a design decision and belongs to
the maintainer.

### 5.2 The pattern worth naming [verified]

Across 2026-07-28, wherever the threat is a **third party** — SSRF, token
passthrough, mix-up, redirect impersonation, handle guessing — the specification
is rigorous and MUST-level with real mechanisms. Wherever the threat is **the
server itself**, it switches to asking the server to behave:

- "Servers **MUST**: … Sanitize tool outputs" (tools, Security Considerations)
- "Server developers **SHOULD NOT** mark sensitive parameters…" (`x-mcp-header`)
- "should ensure that the `cacheScope` correctly reflects the intended
  visibility" (caching, Security Considerations)

Sanitization by the party under suspicion is not a control. This is invariant 8
stated from the other side, and it is the clearest available argument for why a
host-side enforcement boundary exists at all.

---

## 6. Claims checked and found WRONG — do not re-derive

Kept deliberately. An earlier pass in this session asserted all four; two were
falsified by the primary sources.

| Claim | Reality |
|---|---|
| "`cacheScope` leaves the auth subject to implementers" | **Wrong.** `"private"` is normative: caches **MUST NOT** be shared across authorization contexts. A Security Considerations section names the leak and requires per-primitive access controls. |
| "Skills have no license, metadata, or capability fields" | **Wrong.** `license`, `compatibility`, `metadata`, and `allowed-tools` all exist. The real criticism is their *form* — see §3. |
| "`allowed-tools` is a Claude Code host extension, not in the spec" | **Wrong.** It is in the specification, marked experimental. |
| "MCP is silent on untrusted server content" | **Overstated.** Clients **MUST** treat tool *annotations* as untrusted unless the server is trusted, and MCP Apps sandboxes server-rendered UI. What is missing is a trust label on tool *result content* — the `annotations` there are `audience`, `priority`, `lastModified`, which are display metadata. |
| "The spec ignores the confused deputy problem" | **Wrong term.** The specification covers the OAuth confused deputy thoroughly. The uncovered case is different: content returned by server A causing a privileged call to server B. Use a different name for it. |

---

## 7. Explicitly out of scope

Recorded so a later session does not treat these as implied work:

- MCP Apps and Tasks extensions. Not read, not needed.
- Anything competing with or forking the Agent Skills format.
- Any new capability lane. The strategy governs; this file does not.

---

## 8. Unread, if this is picked up

- `crates/egress/src/proxy.rs`, `crates/egress/src/sni.rs` — for §4.2.
- MCP `server/discover`, elicitation, and the multi-round-trip (`mrtr`) pages.
- MCP `prompts` specification. An earlier pass suggested `prompts` is redundant
  with Skills and should be removed. That claim is **unexamined**, and the
  caching page shows `prompts/list` is still a first-class cacheable operation.
- MCP Apps and Tasks extension specifications.
