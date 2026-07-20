# Docs restructure — Phase 2 proposal

Status: **implemented 2026-07-20** (this commit — the layering landed:
choose/concepts pages, the howto directory, the README rewrite, reference
reshaping, terminology sweep). Follows the findings in
[`DOCS_AUDIT.md`](../../DOCS_AUDIT.md) (Phase 1, 2026-07-20).

## Goal and non-goals

Goal: a developer goes from "never heard of it" to "using it confidently" in
under 10 minutes of reading, without deleting the depth power users need.

Per the audit, the estate already approximates Diátaxis:

| Diátaxis quadrant | Exists today | State |
|---|---|---|
| Tutorial | `start.html` (two tracks, expected output per step) | good — surgical edits only |
| Reference | `docs/reference.md` + generated command list | complete — needs command-first reshaping |
| Explanation | `ARCHITECTURE.md`, `ENFORCEMENT.md`, `HISTORY.md` | excellent — TOCs + one mapping sentence |
| **How-to guides** | — | **missing quadrant** |
| (Entry) | README, `index.html`, `docs.html` hub | strong bones, jargon-dense first screen |

So this is a **layering plan, not a restructure**: two new markdown pages, one
new how-to directory, one ruthless README rewrite, one reference reshaping,
one terminology sweep. Non-goals: no site generator, no page deletions, no
command renames, no rewrite of `start.html`/`index.html` from scratch, no
touching ENFORCEMENT/ARCHITECTURE content beyond §6.

Two standing constraints from the brief apply to every task below:

- **Honest posture language is preserved verbatim in spirit** — advisory vs
  enforced, "unapproved egress is blocked", "cooperative… accidents, not a
  determined attacker". Clarity edits must never soften a limit.
- **Every command and output block is verified against the release binary**
  before it lands (see §9). No fabricated output, ever.

## The target map

```
README.md            entry: what/why/30s demo/install → links out   (rewrite, ~150 lines)
docs/
  concepts.md        NEW — every term, 2–3 sentences each, one diagram
  choose.md          NEW — "which mode do I need?" decision page
  howto/             NEW — 6 one-screen task guides
    add-a-server.md
    team-setup.md
    trust-a-repo.md
    lock-down-a-run.md
    undo.md
    ci.md
  reference.md       reshaped command-first; content unchanged
  ARCHITECTURE.md    + TOC, + audience line, + mode-name mapping (§6)
  ENFORCEMENT.md     + TOC, + audience line — content untouched
  start.html         surgical edits (§5): defer the fork, link concepts/choose
  index.html         hero tagline gloss + MCP expansion only
  docs.html          hub: "I want to…" entries point at howto/ pages
```

New pages are markdown on GitHub, matching the existing pattern where
`docs.html` links "source-of-truth Markdown" entries (↗) — no new HTML.

---

## 1. README — the ruthless rewrite

Target ~150 lines (from 383). Structure, in order:

1. **Logo + one sentence** — the audit's B1 register: *"AgentStack puts
   everything your AI coding tools (Claude Code, Codex, Cursor, …) are
   allowed to run into one reviewed file. A repo you clone can't activate any
   of it until you approve that repo — and what you approve runs behind a
   firewall, with every call logged."* Posture claim moves to one glossed
   sentence at the end of the block, or down to the ladder.
2. **The problem, plain** — compress the current "Why" to ~8 lines. First
   "MCP" gets "(Model Context Protocol — the plugin standard agent CLIs use
   for tools)". Keep the `npm install` framing and the honest "you may not
   need this yet" line — both tested well in the audit.
3. **30-second demo** — ONE fenced block, real output. Recommended: the
   init → apply loop (core value, reproducible non-interactively):
   `init --secrets skip` + `apply --write` condensed real output, captured
   fresh via the fenced sandbox harness (§9). Alternative if we want the
   security aha instead: `guard install` + `guard test rm -rf /` (2 commands,
   real denial, no agent needed). Decision at implementation; only one block
   either way, the other becomes a one-line teaser.
4. **Install** — current section + the **`--features sandbox` fix (B7)**: one
   sentence stating release binaries carry sandbox support and a bare
   `cargo build` needs `--features sandbox` for step 6.
5. **The ladder table** — kept (it's the best part of the current README),
   with each step's *section* shrunk to 3–6 lines + its links: step
   walkthroughs already exist in `start.html` and `reference.md`; trust
   detail → `howto/trust-a-repo.md`; the two-extension paragraphs of step 4
   (`--locked`, `[extensions.*]`) → one line each linking reference. Heading
   slugs stay identical so existing `#step-N--…` anchors keep working.
6. **Develop + License** — as today.

Everything cut is a *move-or-already-duplicated*, not a deletion: audit
confirmed steps 1–6 prose is substantially duplicated in `start.html` and
`reference.md`; any non-duplicated fragment folds into the how-tos or
reference before the cut.

## 2. `docs/concepts.md` — one page, every term

~2 screens total. Groups, 2–3 plain sentences per term, one relationship
diagram (SVG, matching the site's existing diagram style) showing:
manifest → lockfile → trust → policy (machine ∩ project) → gateway/runs →
audit, with delivery modes as the horizontal axis and the library feeding in.

Terms (from the audit's B2 + jargon inventory):

- **manifest** (& why ARCHITECTURE calls it a bundle) · **lockfile** ·
  **profile** (vs. policy **preset** — one sentence: unrelated)
- **CLI ≡ harness** · **adapter** · **target** (the only place this trio is
  currently defined is `--help`'s Words footer)
- **MCP** (spelled out) · **gateway** (vs. `agentstack proxy`, the unrelated
  observe-only relay — name the confusion explicitly) · **brokered call**
- **trust** & **consent digest** · **drift** · **guard (cooperative)**
- **sandbox vs lockdown vs `--locked`** — three one-liners + link to
  ENFORCEMENT · **posture** (both senses, disambiguated — B3)
- **delivery modes**: static / clean-at-rest / zero-files (canonical names,
  link to `choose.md`)
- **secrets**: `${REF}` / keychain / varlock · **library vs catalog vs MCP
  Registry vs trust store** (four things that skim alike)
- **egress** · **flight recorder / audit log** (which is which)

Then: reference.md's "A few words" and `--help`'s Words footer both point
here ("full glossary: docs/concepts.md"); every Start/Tutorial/how-to page
links terms here on first use instead of re-explaining.

## 3. `docs/howto/` — six one-screen guides

Uniform shape: **commands first** (a fenced block you can run top-to-bottom),
then 5–10 lines of explanation, then "limits" (honest-scope) and links. Each
≤1 screen. Audience/prereq line at top.

1. **add-a-server.md** — leads with the four-verbs table from audit B5
   (have config details → `add server` / `set server`; know only a name →
   `search` + `add from <id>`; already hand-added in one CLI → `adopt`;
   reusable across projects → `lib add-server` + profile ref), then one
   worked example of each, then when to hand-edit the manifest instead.
2. **team-setup.md** — commit `.agentstack/` (manifest + lock); teammate:
   `secret set` → `apply --write` → `doctor`; what never gets committed;
   `sign`/`verify` as the optional provenance step.
3. **trust-a-repo.md** — `gateway connect --all --write` (once), clone,
   observe inert, `trust .`; exactly what trust does/doesn't cover (link the
   ENFORCEMENT section, don't re-argue it); `trust --list` / `--revoke`.
4. **lock-down-a-run.md** — the escalation in one screen: `run --locked`
   (no Docker, pre-launch gates + frozen surface) → `--sandbox` (container,
   proxied egress, direct route open) → `--lockdown` (no route out); the
   posture label each prints; Docker/image prereqs; `--plan` first.
5. **undo.md** — `restore --last --write` and `restore <id>` for writes;
   plus the full undo table: `gateway disconnect`, `guard uninstall`,
   `trust --revoke`, `session end`, `remove`. (Audit: "one undo verb" is
   true for writes but five other verbs undo other things — this page is
   where that's finally in one place.)
6. **ci.md** — `install --locked` + `doctor --ci`, the GitHub Action
   (pinned tag), what `--ci` fails on, and the non-interactive init recipe
   (`init --secrets skip`).

`docs.html`'s "I want to…" index gets an entry per guide (and gains
"add a server to every CLI" and "share one setup with my team", which the
audit found missing).

## 4. `docs/choose.md` — "which mode do I need?"

One page answering the two forks a user actually faces, in newcomer
vocabulary, table-first:

**Fork 1 — protection level** (the brief's flowchart):

| You are… | You need | One command |
|---|---|---|
| just syncing config across CLIs | steps 1–2 | `init` → `apply --write` |
| worried about `rm -rf` / `.env` accidents | guard (cooperative) | `guard install` |
| cloning repos you didn't write | the trust gate | `gateway connect` + `trust .` |
| launching with a frozen, verified surface, no Docker | protected run | `run <cli> --locked` |
| running sensitive work that must not leak | lockdown (Docker) | `run <cli> --sandbox --lockdown` |

with one honest-scope line per row (cooperative vs enforced, from
ENFORCEMENT's legend — linked, not restated).

**Fork 2 — delivery mode**: static / clean-at-rest / zero-files as a 3-row
table (want zero setup and any CLI → static; repo must stay pristine →
clean-at-rest; many repos, MCP-capable CLI → zero-files), plus "the wizard
defaults to static and you can switch later".

ARCHITECTURE's operating-model table stays as the architect's version; both
pages cross-link, and §6 gives ARCHITECTURE the vocabulary mapping.

## 5. Tutorial (`start.html`) — surgical edits only

The two-track walkthrough already is the golden path; changes are scoped to
the audit findings:

- **Defer the fork**: in A1, the delivery-mode panel gets one line above it —
  "Press Enter for **static** (the default) and keep going; you can switch
  any time. Choosing deliberately? See [which mode do I need?](choose.md)" —
  so the newcomer path introduces ≤3 concepts (manifest, `${REF}`, doctor)
  before first success and treats the fork as a later decision.
- First-use links to `concepts.md` for every remaining term (jargon ban on
  tutorial pages: every term is either glossed inline or linked at first use).
- Verify every "You should see" block against a fresh capture (§9) — the
  audit spot-verified fragments only.
- No structural changes to Tracks A/B.

`index.html`: hero tagline takes the B1 gloss (drop bare "posture" from the
first screen, expand MCP once); everything else stands.

## 6. Terminology unification (audit B3/B4/B8/B10)

One sweep commit, mechanical, across README/docs/site/examples:

| Split | Decision |
|---|---|
| delivery mode / rendered-file mode / three modes / artifact mode | **"delivery mode"** everywhere (the wizard's word — what users see first). Retitle reference section "Delivery modes — where rendered files live" (old anchor kept, see §8). |
| manifest vs bundle | "manifest" in all user-facing prose. ARCHITECTURE keeps "bundle" as its strategic term but states the mapping in its intro paragraph (not a parenthetical), and gains the delivery-mode mapping: "static render ≡ static, native session ≡ clean-at-rest, MCP/profile lease ≡ zero-files". |
| "bundle" (export artifact) | rename in prose to "encrypted archive" (README:310, reference `export`/`import` section). Command help text change rides with §7. |
| posture (two senses) | "posture" reserved for the per-run label. Docs always say "machine-policy summary" for doctor's open/restrictive/mixed line. (CLI wording change optional, §7.) |
| render vs compile | "render" for manifest→native-config; "compile" reserved for policy→`CompiledRuleset` and `[instructions.*]`→managed region. |
| CLI ≡ harness | declared once in concepts.md; README/reference no longer alternate unglossed. |

Explicitly **not** touched (audit §C: already disciplined): trust/consent/
pin/gate vocabulary, drift, secrets naming, profile, library/catalog/
registry, sandbox/lockdown/guard/gateway distinctions.

## 7. Companion CLI fixes (small code PR, separate approval)

From audit §D — help text only, no behavior changes: remove the "P27" leak;
fill the empty `add skill --write` string; describe `<NAME>` on
`add server`/`add skill`/`set server`/`secret set|get|rm`; give
`--target`/`--scope` their full description on `instructions`/`adopt`/`use`/
`diff`/`restore`; one canonical `--write` phrase ("Write the change (else
preview)"); replace "quirks" in doctor's one-liner; add an Examples line to
the six shapes that need one (`add server`, `set server`, `settings set`,
`lib add-extension`, `explain`, `restore`). Then `self docs --write` to
regenerate reference's command block. Optional: reword doctor's
machine-policy line per §6. Per house rules this is a plan-first code change
— I'll treat it as its own approval gate.

## 8. URL stability and redirects

- Site URLs: unchanged (`start.html`, `index.html`, `docs.html`,
  `examples.html` keep their names; the 5 existing redirect stubs stay).
- README: step heading slugs preserved exactly (`#step-1--…` etc.).
- reference.md: retitled sections keep their old anchors via an explicit
  `<a id="where-rendered-files-live-three-modes"></a>` above the new
  heading — same for any heading whose slug changes. No section moves out of
  reference.md in this phase.
- New pages are additive; nothing existing is deleted, so no new stubs
  needed.

## 9. Verification protocol (before any page lands)

- All output blocks captured from the release binary via the fenced
  patterns that already exist: `examples/sandbox/` isolated-HOME harness for
  init/apply/guard flows; `docs/trust-gate-demo.sh` for the trust block.
  Interactive-wizard screens captured once from a real run in a scratch
  project and pasted verbatim (trimmed with `[…]` markers, never invented).
- Command/flag cross-check re-run (the Phase 1 script) after §7 lands, since
  help text regeneration touches reference.md.
- `agentstack doctor`-style claim check: any sentence stating what a mode
  enforces must cite or link ENFORCEMENT rather than restate it.
- After all tasks: re-run the three-persona test from Phase 1 and append
  before/after results to `DOCS_AUDIT.md` (its final section is reserved for
  this).

## 10. Execution order and size

Ordered so each lands independently and the biggest reader-impact ships
first; one session each unless noted:

| # | Task | Touches | Size |
|---|---|---|---|
| 1 | Micro-fixes: README `--features sandbox`, hub "~47 subcommands" count, sandbox-example Docker prereq line | README, docs.html, examples/sandbox/README | XS |
| 2 | `concepts.md` + diagram; point reference "few words" + hub at it | new file, reference.md, docs.html | M |
| 3 | `choose.md`; ARCHITECTURE mapping sentence + TOC + audience line; ENFORCEMENT TOC + audience line | new file, ARCHITECTURE.md, ENFORCEMENT.md | S |
| 4 | README rewrite (§1) with fresh captured demo | README | M |
| 5 | `howto/` six guides + hub "I want to…" rewiring | new dir, docs.html | M |
| 6 | Terminology sweep (§6) | prose-wide | S (mechanical) |
| 7 | reference.md command-first reshape + anchor links in "All commands" | reference.md | M–L |
| 8 | start.html/index.html surgical edits (§5) + full output re-verification | 2 HTML files | S–M |
| 9 | CLI help fixes (§7) + `self docs` regen | crates/cli, reference.md | S (code, own gate) |
| 10 | Persona re-test → append to DOCS_AUDIT.md | DOCS_AUDIT.md | S |

Rough total: ~6–8 working sessions. Tasks 1–6 deliver the audit's tier-1/2
impact; 7–10 are the polish half. Any task can be dropped or reordered
without stranding the others.
