# Instruction variants: one fragment, per CLI and per model

> **Status:** Adopted 2026-08-03, for queue item 4 of
> [`STRATEGY.md`](../../STRATEGY.md) — *"Instructions target CLI and model."*
> That bullet is the requirement; this document fixes the model it left to be
> designed: the variant schema and its precedence, how a variant is pinned, how
> a harness's instruction channel is described honestly, how the model is
> determined and what happens when it is not, and what `status` says. It
> authorizes no work beyond that item.
>
> **Rests on:** [`research/dynamic-instructions-2026-08.md`](research/dynamic-instructions-2026-08.md)
> — the per-harness survey of every mechanism that can put instruction content
> into a model's context. Every claim below about what a harness consumes comes
> from that file, and nothing here claims more than it found.

## What the research changed

[`automatic-delivery.md`](automatic-delivery.md)'s delivery matrix routed
instructions to the rendered lane and justified it with four words: *"MCP
cannot inject these"*. That justification is **false as a protocol claim**. The
MCP `initialize` result has a purpose-built `instructions` field, and
AgentStack's own gateway has been populating it all along
(`crates/cli/src/mcp_server.rs::initialize_instructions`, the ambient skill
index, capped at 50 entries and 160 characters per description) — confirmed
live with Claude Code.

What the research found in its place is narrower and per-harness:

- MCP-`instructions` **consumption is confirmed for Claude Code only**. For
  Codex CLI, Gemini CLI, Cursor, Copilot CLI and OpenCode it is *unconfirmed
  either way* — they are MCP clients, but no source says they surface the field
  into the model's context.
- **Every harness surveyed has at least one non-project channel** — a global
  file, an env var, or a CLI flag.
- **No harness has native per-model instruction conditioning.** The closest is
  Claude Code's `SessionStart` hook receiving a `model` field, which is
  undocumented, explicitly not guaranteed to be present, and reaches us only
  through an executable-kind mechanism that always carries the full consent
  ceremony.

The correction to the delivery matrix is recorded in `automatic-delivery.md`
itself, in that document's own amendment style. The lane does not change; the
reason does, and this document holds the argument.

## The variant schema

An instruction fragment gains an ordered list of **variants**, each selected by
`cli`, by `model`, or by both.

```toml
[instructions.house]
path = "./instructions/house.md"
targets = ["claude-code", "codex"]

  [[instructions.house.variant]]
  cli = "claude-code"
  model = "opus"
  path = "./instructions/house.claude-opus.md"

  [[instructions.house.variant]]
  cli = "codex"
  path = "./instructions/house.codex.md"

  [[instructions.house.variant]]
  model = "opus"
  path = "./instructions/house.opus.md"
```

`[[instructions.<name>.variant]]` is an array of tables — the spelling the lock
already uses for `[[package.member]]`, and the ordinary TOML idiom for an
ordered list under a named entry. A variant carries exactly a selector and a
`path`; it is the same kind of thing the base entry is, so it is declared the
same way. Both selector keys are optional, but a variant with neither is
refused: it would be a second base body with no way to choose between them.

The base `path` is the fragment when nothing matches, and it stays required for
an inline fragment. `targets` is unchanged and keeps its own job — `targets`
decides **whether** a fragment reaches a CLI at all, `variant` decides **which
bytes** it sends once it does. Merging them would make a fragment's absence and
a fragment's specialization the same word.

### Precedence: most specific wins

> **Exact `(cli, model)` beats `(cli)` beats `(model)` beats the base body.**

Four levels, tested in that order, first match at the most specific level that
matches. Two properties make it predictable:

- **`cli` outranks `model`.** A harness difference is a difference in what the
  instruction file *means* — Codex reads `AGENTS.md`, Claude Code reads
  `CLAUDE.md`, and their built-in prompts differ — while a model difference is
  a difference of degree within one harness. When an author has said something
  about a CLI and something else about a model, the CLI statement is the one
  that cannot be wrong.
- **Ties break by declaration order.** Two variants with the *identical*
  selector resolve to the first one declared, exactly as the first linked
  library source wins a name
  ([`linked-library-sources.md`](linked-library-sources.md) §"The precedence
  rule"). It is a typo either way, but a typo with one deterministic answer
  beats a typo with two.

Worked example, against the fragment above:

| CLI | model | selected | why |
|---|---|---|---|
| `claude-code` | `opus` | `house.claude-opus.md` | exact `(cli, model)` |
| `claude-code` | `sonnet` | *(no `cli`-only variant)* → `house.md` | no `(claude-code, sonnet)`, no `(claude-code)`, no `(sonnet)` |
| `codex` | `opus` | `house.codex.md` | `(cli)` beats `(model)` |
| `codex` | *unknown* | `house.codex.md` | `(cli)` still matches; the model is simply not consulted |
| `gemini` | `opus` | `house.opus.md` | no `cli` match, `(model)` matches |
| `gemini` | *unknown* | `house.md` | the base body — the least specific match |

The last two rows are the whole of the unknown-model rule: **an unknown model
never matches a `model` selector, and never guesses one.** It falls to the most
specific variant that matches on `cli` alone, and to the base body if there is
none.

### The same schema in a library source

A linked library source
([`linked-library-sources.md`](linked-library-sources.md)) holds instructions in
its `instructions/` folder, as a **directory body** per fragment — because a
fragment with variants is several files, and the folder taxonomy already
distinguishes bodies (directories: `skills/<name>/`, `extensions/<name>/`,
`packages/<name>/`) from single definitions (files: `servers/<name>.toml`,
`hooks/<name>.toml`).

```text
<source>/instructions/house/
  instruction.toml
  house.md
  house.claude-opus.md
```

```toml
# <source>/instructions/house/instruction.toml
path = "house.md"

[[variant]]
cli = "claude-code"
model = "opus"
path = "house.claude-opus.md"
```

The `[[variant]]` grammar is identical to the manifest's, parsed by the same
code — one grammar, one hostile-input gate, one precedence function. Every path
in the file is resolved inside the body directory and containment-checked;
`..`, an absolute path, and a symlink escape are refused before any read.

A project selects a library fragment with a **sourceless** manifest entry,
mirroring extensions exactly (`ExtensionOrigin::Library` — a manifest entry
that declares no `path` resolves its body from the library):

```toml
[instructions.house]           # no path — resolved from the linked sources by key
targets = ["claude-code"]
```

Resolution walks the ordered sources and takes the first that holds an
instruction of that name — `PATH` semantics, the same first-match-wins rule
every other library kind uses, with collisions surfaced rather than hidden.

**A qualified `<source>:<name>` spelling is deliberately not added here**, for
the same reason it is absent for extensions and hooks: those kinds are declared
by *manifest key*, not selected from a reference list, so there is nowhere for
the qualifier to live that would not be a new field. This is a named debt
carried from item 3, not a new one.

## Every variant body is pinned

A variant is content, and content is pinned. The lock's `[[instruction]]` entry
gains one `[[instruction.variant]]` per declared variant:

```toml
[[instruction]]
name = "house"
path = "./instructions/house.md"
checksum = "<64 hex>"

  [[instruction.variant]]
  cli = "claude-code"
  model = "opus"
  path = "./instructions/house.claude-opus.md"
  checksum = "<64 hex>"
```

Four properties, each load-bearing:

- **The same pinning act.** Every checksum comes from `Store::pin_instruction`,
  which hashes the file's raw bytes *and deposits them* into the
  content-addressed store. No second digest path exists and none may be added
  ([`package-layer.md`](package-layer.md) §"The lock expansion" says the same
  thing about package members).
- **A drifted variant fails closed.** `resolve::instruction_lock_status`
  compares the base body **and every variant body** against its pin. Any
  mismatch is `ChecksumDrift`, which marks the project Changed, blocks
  `apply --write` / `instructions --write` at the existing
  `verify::ensure_instructions_compilable` gate, and re-gates through the
  ordinary review card. A variant nobody is currently selecting still fails
  closed: it is bytes the consent covers, and consent is over content, not over
  what happened to be chosen today.
- **No silent fallback.** A selected variant whose bytes cannot be verified is
  **excluded and named** — never quietly replaced by the base body. Falling
  back would deliver prose the author did not choose for this CLI while
  reporting success.
- **Additive at lock version 2.** `variant` is a `#[serde(default)]` array, so
  every lock written before this parses unchanged, and an older binary that
  rewrote the variants away would change the lock bytes — which flips the trust
  digest and forces a review rather than losing the pins silently. This is the
  same argument the executable, extension, workflow and package pins already
  carry.

## Channels: confirmation-gated, never over-claimed

### The rule

> **Deliver through the best channel a harness is *known* to consume, and say
> exactly how well it is known.**

Three states, and every surface distinguishes them:

| State | What it means | What AgentStack does |
|---|---|---|
| **confirmed** | Someone observed this harness consuming this channel | It may carry instructions dynamically — when it can carry them *correctly* (see below) |
| **unconfirmed** | Documented or protocol-level, never verified here | Never used as though it worked; the harness's confirmed file channel carries the fragment, and the surface says so |
| **none** | The harness has no instruction destination at all | Nothing is delivered, and the surface says that plainly |

### Where the data lives

On the adapter descriptor, never in the delivery code. `InstructionsSpec` gains
an optional `live` block:

```yaml
# crates/adapters/descriptors/claude-code.yaml
instructions:
  global: ~/.claude/CLAUDE.md
  project: CLAUDE.md
  live:
    channel: mcp-instructions
    confirmation: confirmed
    note: >-
      Observed live: the gateway's own initialize `instructions` reached the
      agent as an MCP-server instruction block.
```

Adding a harness, or upgrading a channel from `unconfirmed` to `confirmed`, is
therefore a **descriptor change** — a YAML edit and a line of evidence, with no
branch anywhere in the delivery path that names a CLI.

The absence of an `instructions:` block at all is the third state: the harness
carries no instruction channel, and no surface may imply coverage for it.

### The honest matrix, as implemented

Drawn from the research, and exactly what ships in the descriptors:

| Harness | Instruction file | Live channel | Confirmation |
|---|---|---|---|
| Claude Code | `~/.claude/CLAUDE.md` · `CLAUDE.md` | MCP `initialize` `instructions` | **confirmed** — observed live |
| Codex CLI | `~/.codex/AGENTS.md` · `AGENTS.md` | MCP `initialize` `instructions` | unconfirmed |
| Copilot CLI | `~/.copilot/copilot-instructions.md` · `AGENTS.md` | MCP `initialize` `instructions` | unconfirmed |
| OpenCode | `~/.config/opencode/AGENTS.md` · `AGENTS.md` | MCP `initialize` `instructions` | unconfirmed |
| Junie | `~/.junie/AGENTS.md` · `.junie/AGENTS.md` | MCP `initialize` `instructions` | unconfirmed |
| Pi | `~/.pi/agent/AGENTS.md` · `AGENTS.md` | *(not an MCP client)* | — |
| Gemini CLI, Cursor, and the six other registered adapters | — | — | **no instruction channel** |

Six of thirteen adapters carry an instruction channel. The matrix says so on
every surface that reports it, and `status` names each of the other seven
rather than omitting them — an adapter that silently disappears from a coverage
list reads as covered.

**One wire fact the copy must never blur.** A *global*-scope rendered file
(`~/.claude/CLAUDE.md`) keeps bytes out of the repository, which is what the
clean-project goal asks for — and it is still a persisted file. It is not
dynamic, it is not "served live", and no surface may call it either.

## Do instructions route dynamically for Claude Code? No — and why

Claude Code's MCP `instructions` channel is confirmed, so this is a real
choice rather than a physical impossibility. Instructions **stay in the
rendered lane**, for four reasons that are properties of the channel, not
caution:

1. **The channel cannot know the model, which is the whole point of a
   variant.** `initialize` carries `clientInfo` — a client name and version —
   and nothing about the model behind it. Routing instructions through it would
   force every fragment to the least specific match on every connection, which
   is the exact capability this item exists to add. A channel that structurally
   defeats the feature is not the feature's delivery path.
2. **It fires before any selection.** `initialize` happens once, at connection
   time, before a lease names a toolset — and MCP has no "instructions changed"
   notification to correct it afterwards. Putting project prose there would put
   selected content into agent context with **no lease behind it**, which is
   precisely the fence
   [`automatic-delivery.md`](automatic-delivery.md) §"Failure semantics 1"
   closes: no lease → control-plane tools only.
3. **It is already load-bearing, and bounded.** The field carries the ambient
   skill index that makes on-demand loading discoverable, under an explicit
   50-entry / 160-character-per-description cap. Instruction prose would
   compete with the discovery surface for the same budget.
4. **Equivalence is unproven.** What is confirmed is that the field *reaches
   the model*, as an MCP-server-scoped block. That it carries the same standing
   as a `CLAUDE.md` house rule — through compaction, across turns, at the same
   priority — is not confirmed by anything, and invariant 8 does not let the
   copy assume it.

So the delivery matrix's **lane is unchanged and its justification is
replaced**: not *"MCP cannot inject these"* (false), but *"no live channel a
harness is known to consume can carry an instruction per model or behind a
lease"* (true, and per-harness data rather than a protocol claim). The planner,
the matrix, and the honesty copy all say that one thing, and no surface
describes an instruction as going live "via gateway" — because none does.

This is the conservative answer, and it is reversible in the direction that
matters: the day a harness offers a channel that is both confirmed *and*
model-aware, the descriptor gains it, the reason changes, and the argument
above is the checklist the change has to pass.

## How the model is determined

The model is never sniffed, and never guessed. It comes from exactly two
declarations, in this order:

1. **An explicitly selected toolset's `model`.** `[toolsets.<name>] model =
   "opus"`, when that toolset is the one the command names (`run <cli> --toolset
   backend`, `use backend`, `apply --profile backend`,
   `instructions --toolset backend`). *(The manifest key is still `profiles`;
   `toolset` is the CLI's vocabulary for the same noun, with `--profile` kept as
   the alias. That gap predates this document and is not resolved here.)* This
   is the selection act the strategy names — *"the model switch is AgentStack's own
   orchestration: automatic where the harness exposes model identity, explicit
   toolset switch elsewhere"* — and the toolset is already the single selection
   noun (project selection, lease unit, and workflow role are one noun).
2. **The harness's own managed setting.** `[settings.<cli>] model = "..."`,
   the value AgentStack itself compiles into that CLI's native config. If we
   wrote it, we know it.

Anything else is **unknown**, and unknown is a first-class answer: the least
specific matching variant is used and the surface says which one and why.
Deliberately *not* consulted:

- **A default toolset nobody named.** A default is not a selection.
- **Trailing `run` arguments.** `run claude-code -- --model opus` passes argv
  through opaquely and by design; parsing it would mean re-implementing each
  harness's flag grammar and being confidently wrong on the first upgrade.
- **Claude Code's `SessionStart` `model` field.** Undocumented, not guaranteed
  present, and reachable only through an executable-kind mechanism that always
  carries the full consent ceremony. Building the feature on it would make a
  hook a prerequisite for correct instructions.

Every surface that reports a selected variant reports the **source** of the
model with it (`from toolset backend`, `from settings.claude-code`, or `model
unknown`), so a wrong variant is diagnosable from the line that chose it rather
than by reading three files.

**One honest tension, stated rather than hidden.** A toolset's `model` is an
*intent* and `[settings.<cli>] model` is a *fact we write*. When both are
declared and disagree, the toolset wins — it is the narrower, more deliberate
act — and the reported source is how a user sees that it did. AgentStack does
not reconcile them, because reconciling would mean a toolset selection silently
rewriting a harness's native settings file, which is a much larger act than
choosing which paragraph to compile.

## What `status` says

The honesty matrix is a `status` block, one row per targeted harness, in
adapter order. Three shapes, and every word is checkable:

```text
House rules  Claude Code — CLAUDE.md (project); house → claude-code+opus
                 (model opus, from toolset backend)
                 live channel MCP initialize instructions: confirmed for this
                 tool, not used for house rules — no live channel varies by model
             Codex CLI — AGENTS.md (project); house → codex (model unknown —
                 least specific match used)
                 live channel MCP initialize instructions: unconfirmed —
                 never used as though it worked
             Gemini CLI — no instruction channel; house rules do not reach
                 this tool
```

`status --json` carries the same rows under `instruction_channels`, gated on
the `instruction-channels-v1` ui-contract feature. What that feature promises
and does not promise is written where every other feature's promises are, in
`crates/cli/src/ui_contract.rs`.

## Invariant walk

Against `CLAUDE.md`'s non-negotiable invariants.

- **3 · Untrusted repository content is inert.** A variant is a path in a
  manifest or in a library body, read at compile and pin time only, and every
  one of those reads is behind the trust gate that already fronts instruction
  compilation. A repository cannot add a library source (item 3's rule), so it
  cannot aim variant resolution anywhere the user did not link. No variant
  selector, path, or body starts a process, contacts a network, or resolves a
  secret — an instruction fragment is prose, and the selection is a comparison
  of two strings.
- **4 · Pinned byte changes re-gate.** Every variant body carries its own
  digest in the lock, so any variant's bytes moving means different lock bytes,
  which flips the trust digest and re-gates through the ordinary review card.
  No cache and no partial-trust path is added: there is no "the selected
  variant is unchanged, skip the others" fast path, because the comparison is
  over every pinned body and not over the one in use. Adding or removing a
  variant is likewise a lock change and re-gates.
- **5 · Secrets never serialize.** Nothing here touches secret resolution. A
  variant selector is two optional strings, a variant body is markdown, and
  neither the lock, the library index, nor `status --json` gains any field that
  could carry a resolved value. `${REF}` handling is untouched and still fails
  closed.
- **7 · All repository content is hostile input.** A library instruction body
  is somebody's directory, so it is parsed exactly as every other library body
  is: `instruction.toml` read bounded and parsed defensively, the name put
  through the ordinary name contract, every declared path containment-checked
  against the body root with `..`, absolute paths and symlink escapes refused
  before any read, and every string that reaches a terminal sanitized. A
  selector value is compared, never interpolated — nothing from a variant
  reaches a shell command, and a `cli` selector that names no registered
  adapter simply never matches.
- **8 · Claims match enforcement.** This is the invariant the whole document
  serves. The delivery matrix's disproven justification is replaced with a
  per-harness one; an unconfirmed channel is labelled unconfirmed and is never
  used as though it worked; a harness with no channel is named rather than
  omitted; a global-scope rendered file is never called dynamic; an unknown
  model is reported as unknown instead of defaulted into a claim; and no
  instruction is ever described as going live "via gateway", because none is.

## What this does not do

Named so nobody reads it in.

- **It does not add a per-model channel to any harness.** None exists. What
  ships is AgentStack choosing the bytes; the harness still receives one file.
- **It does not route instructions through the gateway.** See the argument
  above; the lane is unchanged.
- **It does not add instructions to Gemini CLI or Cursor.** Both have
  documented global-file channels the research found, and neither has an
  `instructions:` block today. Wiring them up means AgentStack starting to
  write two files it has never written, which is an intake decision with its
  own consent surface — not a side effect of adding variants.
- **It does not vary anything but instructions by model.** Skills, servers,
  settings and hooks are unchanged. `(CLI, model)` is a property of prose,
  which is the one kind whose right wording genuinely differs per reader.
