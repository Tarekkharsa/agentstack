# Docs architecture — evidence and proposal

**Status:** proposal for the maintainer. Written 2026-08-05/06 from a read-only
survey of `docs/`, the shipped binary's help tree, and `tools/`.

**The complaint being answered:** "The docs site is too much, too complicated,
over-engineered. If you put every scenario and everything this project supports,
it would fit in one page. We have a lot of pages and we need them easier and
categorised correctly for users."

**The measurement:** 28 published pages, ~13,700 lines of authored docs (26,539
including the generated `.html` twins), for a CLI whose default `--help` shows
**fifteen verbs**.

**Recommendation in one line:** cut the site from 28 published pages to **9**,
delete the 2,626-line simulated GUI outright, and move the whole
"hand-maintained simulation of CLI output" category behind a generator or a
test. Three cuts get you 78% of the reduction.

---

## 1. The overlap matrix

Three concepts were the suspects. All three are worse than suspected. Every
claim below is `file:line` on both sides.

### 1.1 Trust — the most duplicated idea in the corpus

**Cluster A — "untrusted means inert; five kinds of delivery refuse."**
Twelve near-verbatim restatements:

| # | location | note |
|---|---|---|
| 1 | `docs/howto/trust-a-repo.md:38-54` | **best version** — the only one that renders the five as a table mapping refusal → triggering command, and the only one that pairs them with the three deliberate exclusions at `:56-71` |
| 2 | `docs/concepts.md:144-153` | best *short* form |
| 3 | `docs/troubleshooting.md:329-340` | |
| 4 | `docs/faq.md:106-111` | |
| 5 | `docs/start.md:126-130` | |
| 6 | `docs/migrations.md:132-136` | |
| 7 | `docs/howto/ci.md:42-46` | |
| 8 | `docs/howto/team-setup.md:27-34` | |
| 9 | `docs/automation.md:72-84` | |
| 10 | `docs/ARCHITECTURE.md:240-242` | |
| 11 | `docs/ENFORCEMENT.md:76-85` | |
| 12 | `docs/choose.md:63` | |

`faq.md:106-111` and `start.md:126-130` are effectively the same sentence twice —
both enumerate "no native MCP server config, no skill files, no compiled
`CLAUDE.md`/`AGENTS.md` region, no hooks, no extensions."

The canonical statement, worth keeping verbatim, is
`docs/howto/trust-a-repo.md:26-36`:

> "a repo you just cloned is **inert**: none of its servers run or are
> contacted, and no secrets resolve, until you run `agentstack trust .`. Trust
> shows exactly what the manifest runs and contacts, then pins the consent
> digest of the manifest, its local overlay, and the lockfile."

and `docs/concepts.md:138-142`:

> "**Trust** — your local approval that a project may auto-load on this machine.
> Until you run `agentstack trust .`, a cloned repo is inert: no server spawns,
> no skill enters context, no secret resolves."

**Cluster B — "lock first, then trust; re-locking is new consent."** Nine
restatements: `concepts.md:74-87` ≈ `howto/trust-a-repo.md:89-109` ≈
`troubleshooting.md:490-515` ≈ `troubleshooting.md:517-554` ≈ `start.md:199-211`
≈ `faq.md:118-123` ≈ `howto/add-a-skill.md:36-39` ≈ `howto/add-a-server.md:66-68`
≈ `howto/name-a-toolset.md:87`.

Note that `troubleshooting.md:490-515` and `troubleshooting.md:517-554` are **the
same rule twice inside one file** — the file admits it at `:520` ("This is the
same rule seen from the other end"). Best version, and the tightest sentence in
the whole corpus, `docs/concepts.md:83-87`:

> "`lock --write` *accepts* the new bytes, which is a machine's job; only `trust`
> *reviews* them, which is yours, and only the review re-opens delivery."

**Cluster C — "trusted ≠ safe."** Eight restatements: `ENFORCEMENT.md:51-74` ≈
`faq.md:213-218` ≈ `howto/trust-a-repo.md:116-126` ≈ `concepts.md:140-142` ≈
`reference.md:841-844` ≈ `tutorial/index.html:304` ≈ `docs.html:228` ≈
`panel/index.html:1855-1858`. `tutorial/index.html:304` and `docs.html:228` are
the same callout with cosmetic edits — both say "pins a SHA-256 consent digest of
manifest + overlay + lockfile … does not vouch the code is safe … vet with
`agentstack explain <name>`."

Best version `docs/ENFORCEMENT.md:45-49` — the only one that states the positive
claim before the negatives:

> "Trusting a project asserts exactly one thing: **the current manifest, local
> overlay, and lockfile consent digest was approved for automatic loading on this
> machine.**"

**Cluster D — "a command may not refuse its own write."** `concepts.md:161-172` ≈
`ARCHITECTURE.md:244-268` ≈ `troubleshooting.md:556-601` ≈
`howto/trust-a-repo.md:67-71` ≈ `howto/add-a-skill.md:83-102`. Best normative
version `ARCHITECTURE.md:244-268` (the only place `PriorTrust` and `TrustCarry`
are named); best user-facing version `troubleshooting.md:556-601`.

**Cluster E — "consent does not travel."** `howto/trust-a-repo.md:140-144` ≈
`migrations.md:126-131` ≈ `automation.md:84-86` ≈ `howto/ci.md:42-43` ≈
`integrations.md:183-191` ≈ `howto/team-setup.md:69-72`. Best
`migrations.md:126-131` — the only one enumerating all three cases (clone, second
checkout, worktree).

**Cluster F — the digest-bound headless grant.** `howto/ci.md:54-75` ≈
`troubleshooting.md:616-629` ≈ `reference.md:819-827` ≈ `automation.md:88-91` ≈
`migrations.md:139-143` ≈ `integrations.md:264-272`. Best `howto/ci.md:54-75` —
the only one that says *why* the digest must come from the surface the job just
printed.

> **Six clusters, ~50 passages, for one idea.** Trust is taught somewhere in 15
> of the 28 pages.

### 1.2 Delivery lanes — duplicated *and* internally contradictory

**The lane table exists four times near-verbatim**, plus two prose restatements:

- `docs/concepts.md:284-289` — **canonical**, and the anchor
  (`#delivery-modes`) that `reference.md:618`, `choose.md:49`, `start.md:271`,
  `troubleshooting.md:96`, `howto/add-a-skill.md:82`, `howto/team-setup.md:61`
  and `howto/name-a-toolset.md:134` all point at
- `docs/choose.md:21-26` — the only copy that adds a *why* per row
- `docs/reference.md:620-625` — pure redundancy above its own `--json` detail
- `docs/tutorial/index.html:384-388`
- prose forms: `docs/start.md:83-86`, `docs/ENFORCEMENT.md:256-261`

All four table copies carry the same four rows and the same "no live channel a
CLI is *known* to consume varies by model" clause.

**`render-locally` is explained seven times.** The six-item rationale list
("offline work, deterministic native files, … compatibility testing against a
CLI's own behaviour") is reproduced **word-for-word** in `reference.md:640-643`,
`concepts.md:298-301` and `choose.md:33-36`. Other copies:
`start.md:98-103`, `ENFORCEMENT.md:258-261`, `tutorial/index.html:387` and
`:653-657`, `troubleshooting.md:596`. Best `reference.md:636-644` — the only copy
with `--off`, both TOML spellings, and "automatic is the *absence* of an
override, not a second stored value."

**"0 project artifacts, never '0 files'" appears four times** —
`concepts.md:305-307` ≈ `reference.md:646-648` ≈ `ENFORCEMENT.md:156` ≈
`tutorial/index.html:648-649`.

**The three older modes appear three times** — `reference.md:666-682` ≈
`concepts.md:309-329` ≈ `choose.md:46-51`.

#### The contradiction — this is the real damage

`docs/ARCHITECTURE.md:91-128` still teaches delivery as a **mode you choose**,
after the 2026-08-03 flip to routing:

- `:93-94` — "The **delivery mode** answers *how does it reach the agent?*"
- `:106` — "| Delivery | How does selection reach the harness? Static render,
  native session, or MCP lease. |"
- `:110-123` — a "Situation | **Use** | Why" table that literally instructs the
  reader to pick a mode
- `:124-128` — "Those three delivery mechanisms are what the user-facing docs
  call **delivery modes**", present tense, no mention of the flip

Directly contradicted by `concepts.md:279` ("a routing decision AgentStack
makes"), `choose.md:16` ("You do not have to choose"), `reference.md:614-616`,
`ENFORCEMENT.md:254` ("routed, not chosen"), `tutorial/index.html:383` ("not a
mode you pick"), and the binary itself — `agentstack --help --all` prints
`set-mode  Retired: delivery is routed, not a mode you pick`.

Worse: `choose.md:50-51` sends the reader **to that stale section** as the
authority, and `ARCHITECTURE.md:127` sends them back to `concepts.md` — a
circular link between two mutually inconsistent versions of the same idea.

**And `choose.md` contradicts itself ten lines apart.** Its H1 is "# Which mode
do I need?" and `:5-11` says "AgentStack has two decisions to make…This page
picks both from what you are trying to do" — then `:16` says the first decision
is not the reader's to make. The title, the lede and the "First: how do
capabilities reach your CLIs?" heading all survive from the pre-flip version.

Stale "mode" vocabulary elsewhere: `integrations.md:69` ("typed delivery
**mode**") sits two rows above `:71` ("retired; do not build a mode picker");
`automation.md:119`; `concepts.md:355-357` (secrets still explained per-mode);
`troubleshooting.md:92`; and `reference.md:611`, whose own anchor is still
`where-rendered-files-live-three-modes` while the heading says "routing".

### 1.3 Toolsets — duplicated, and the vocabulary is not settled

Taught in at least 20 places across 12 files. Canonical definition
`docs/concepts.md:89-99`; canonical teaching artifact
`docs/howto/name-a-toolset.md` (the only page with the definition, the negative
space, the command path, the hand-written TOML path, a two-toolset worked
example, and an activation decision).

Duplications:

- `concepts.md:91-93` ↔ `tutorial/index.html:602` ↔ `tutorial/index.html:315` —
  near-verbatim triplicate of "A named subset of the manifest ("backend",
  "design") you activate together". The two tutorial copies duplicate **each
  other inside one file**.
- `start.md:159-180` ↔ `howto/name-a-toolset.md:5-33` — same command, same TOML,
  same trust caveat. `start.md:273` already links out to the how-to.
- `reference.md:455-457` ↔ `reference.md:1468-1470` — the same membership rule
  twice inside one file, ~1,000 lines apart. Only the second carries the
  load-bearing clause: "Naming a nonexistent toolset is an error, never a silent
  create."
- Four accounts of `use` vs `session start`: `faq.md:166-176` ↔
  `howto/name-a-toolset.md:119-132` ↔ `concepts.md:337-345` ↔
  `tutorial/index.html:312`.
- Fence semantics four times: `ENFORCEMENT.md:132-142` ↔ `ENFORCEMENT.md:542-547`
  ↔ `workflows.md:61-66` ↔ `reference.md:501-511`.
- Three first-contact one-liners: `cookbook.html:337` ↔ `index.html:138` ↔
  `panel/index.html:643`.

**The naming promise is false.** `docs/howto/name-a-toolset.md:11` states:
"everything you read, type, and write says *toolset*." Counterexamples:

- *read*: `troubleshooting.md:833` quotes the shipped error string
  `error: no profile 'dev' in manifest`.
- *type*: `reference.md:867` (`agentstack_lease_open({ "profile": "backend" })`),
  `reference.md:1932` (`diff --profile`), `reference.md:1944-1954` (eight panel
  commands: `add-skill-to-profile`, `create-profile`, `use-profile`,
  `edit-profile`, `rename-profile`, `delete-profile`).
- *contracts*: `automation.md:122,147,159-163` (`profiles-v1`,
  `profiles-edit-v1`, `profiles-edit-batch-v1`), `automation.md:195,314`
  (`"profile": "dev"`), `integrations.md:48-49,86`.
- *demo*: `panel/index.html:1190-1214` shows AgentStack **writing** a
  `[profiles.rust]` table — directly contradicting `concepts.md:95-96` ("it is
  read, never written back").

**"Bundle" carries four meanings** across the corpus: the manifest primitive
(`ARCHITECTURE.md:103`), a toolset (`cookbook.html:337`, `panel/index.html:643`),
the trusted unit (`ENFORCEMENT.md:1045`, `workflows.md:45`), a shareable archive
(`start.md:264`), and a skill (`cookbook.html:322`). "Stack" is used informally
for the whole capability set at `concepts.md:326` and `reference.md:675`,
colliding with the product name.

Stale API name: `reference.md:753` documents `agentstack_create_profile`; the
live MCP surface exposes `agentstack_create_toolset`.

Gap: `docs/migrations.md` — the page whose job is migrations — **never mentions
the profile→toolset rename**. The one rename users carry in their manifests is
documented only in `concepts.md:94-96` and `howto/name-a-toolset.md:9-11`.

---

## 2. The three-way split: what the corpus actually *is*

23,022 lines ship under `docs/` (excluding `archive/`, `design/`, `theme/`).
They fall into three categories, and the ratio is the whole argument.

| category | lines | share | maintained by |
|---|---:|---:|---|
| **(a) generated / generatable** | 8,832 + 75 + ~64 = **8,971** | **39%** | `make-docs-pages.py`, `make-docs-sidebar.py`, `self docs --write` |
| **(b) hand-written teaching** | ~**8,150** | **35%** | humans, reviewed |
| **(c) hand-maintained simulation** | ~**5,900** | **26%** | humans, **unreviewed, unverified, undetected when wrong** |

### (a) Generated — 8,971 lines, 39%

- **8,832 lines** of `.html` twins: all 22 generated pages (`start.html` …
  `howto/*.html`). Nobody writes these; `tools/make-docs-pages.py` compiles them
  from the 22 `.md` sources. They are 100% of the `.html` weight for those pages.
- **75 lines** of redirect stubs (`how-it-works.html`, `primitives.html`,
  `library.html`, `strategy.html`, `mcp-capability-layer.html` — 15 lines each).
- **~64 lines** inside `reference.md:1893-1958`, between
  `<!-- agentstack:generated commands -->` and `<!-- agentstack:end -->`, produced
  by `agentstack x self docs --write` and CI-checked for staleness.

**Finding:** the generated share is already large and already correct. It is not
the problem. It also means the "28 pages, 13,700 lines" framing understates the
win — deleting one `.md` deletes its `.html` twin too, so every line cut is
roughly **two** lines removed from the shipped tree.

### (b) Hand-written teaching — ~8,150 lines, 35%

The 22 `.md` sources total **8,293** lines; subtract the 64 generated lines and
the ~256 lines of simulated terminal output measured inside them (below), and
about **7,970** lines are genuine prose teaching. Add ~180 lines of real prose in
`index.html`/`docs.html`.

This is the category that should exist. It is also the category the overlap
matrix shows is roughly **2–3× larger than it needs to be**: six clusters for
trust, four table copies plus seven prose copies for delivery, twenty places for
toolsets.

### (c) Hand-maintained simulation of CLI output — ~5,900 lines, 26%

**This is where every drift this session came from.** Nothing generates it,
nothing checks it, and it goes stale silently because no test ever compares it to
the binary.

| artifact | lines | what it simulates |
|---|---:|---|
| `docs/panel/index.html` | **2,626** (2,293 of them JavaScript) | an entire fake t3code GUI — a running single-page app with hardcoded fake CLI responses |
| `docs/cookbook.html` | **901** (851 lines of hand-written markup) | ~55 hand-typed terminal transcripts |
| `docs/tutorial/index.html` | **901** (487 JS) | a fake interactive terminal, 99 simulated-output glyph hits |
| `docs/examples.html` | **434** (202 JS) | 16 hand-typed transcripts |
| `docs/security-review-2026-07-11.html` | **425** | a dated point-in-time report, hand-maintained |
| simulated output inside the 22 `.md` files | **~256** | `✓ ⚠ ↳ │ Next:` blocks typed by hand — 152 of them in `troubleshooting.md` alone |
| `docs/start.html` body transcripts (via `start.md`) | ~15 | |

**The three worst offenders, and why:**

1. **`panel/index.html` (2,626 lines).** 87% JavaScript. It is a second UI. The
   project's own `CLAUDE.md` says: *"t3code is an optional companion calling
   stable read APIs and fixed actions, never an enforcement boundary. **Never
   recreate a second UI.**"* This page is a hand-written recreation of that UI,
   in the docs tree, that no test runs and no adapter compiles. It is already
   provably wrong: `panel/index.html:1190-1214` shows AgentStack **writing** a
   `[profiles.rust]` table, contradicting `concepts.md:95-96` ("it is read, never
   written back") and the shipped `profiles` → `toolsets` rename.
2. **`cookbook.html` (901 lines).** 851 lines of hand-written markup containing
   ~55 transcripts. Every flag change in the CLI can invalidate any of them, and
   nothing tells you.
3. **`troubleshooting.md` (877 lines, 152 of simulated output).** The page whose
   entire value is *"this is the exact string you saw"* — and the exact strings
   are typed by hand. `troubleshooting.md:833` still quotes
   `error: no profile 'dev' in manifest`.

**The structural conclusion:** 26% of the shipped docs tree is an unverified
re-implementation of the binary's output. The right size for that category is
**near zero** — either the CLI prints it (link to `--help`), or a golden-file
test asserts it, or it should not be in the docs.

---

## 3. Proposed structure — nine pages

The smallest set that covers the product. Everything else is generated, merged,
or deleted.

| # | page | one-line purpose | the reader who arrives |
|---|---|---|---|
| 1 | `index.html` | The pitch and the install line. | Someone who has heard the name and has 60 seconds. |
| 2 | `one-page.md` → `one-page.html` | **The whole product**: model, gate, delivery, the nine journeys, every verb named. Absorbs `start`, `concepts`, `choose`, `faq`, all nine how-tos. | Everyone who decided to try it. This is the site. |
| 3 | `tutorial/` | The only interactive artifact — do it, don't read it. Rebuilt against the binary. | The reader who learns by typing. |
| 4 | `reference.md` | Every verb, flag and subcommand. Grepped, never read. | Someone who knows what they want and needs the spelling. |
| 5 | `ENFORCEMENT.md` | What is *enforced* vs merely cooperative. The product's claims document. | A security reviewer, and CI. |
| 6 | `adapters.md` | Which of 13 CLIs supports what, at which scope. | Someone checking whether their CLI is covered. |
| 7 | `workflows.md` | Governed multi-agent runs — separate threat model, separate evidence contract. | The small set of users running workflows. |
| 8 | `integrations.md` | The t3code fixed-action contract + the automation/JSON contract. | An integrator building against stable APIs. |
| 9 | `troubleshooting.md` | The residue of failures the binary cannot explain inline. | Someone who is already stuck. |

Plus `migrations.md` as a **10th, conditional** page (see the table below — I
would keep it only if the profile→toolset rename is added to it).

Nine pages, ~6,000 authored lines against today's 14,115. With generated twins,
the shipped tree drops from 23,022 lines to roughly **9,500**.

---

## 4. Disposition table — every current page

**Legend:** KEEP = unchanged · GENERATE = keep, but a tool produces it ·
MERGE INTO x · DELETE.

### The 22 Markdown sources

| page | lines | disposition | reason | what is lost |
|---|---:|---|---|---|
| `reference.md` | 1,986 | **KEEP** (+ grow the generated region) | The grep surface; the one page whose length is a feature. | — |
| `ENFORCEMENT.md` | 1,383 | **KEEP, untouched** | See §5 — it is hard-gated by `tools/check-enforcement-pairing.py`. | — |
| `troubleshooting.md` | 877 | **KEEP, cut to ~350** | 152 lines are hand-typed CLI output; the binary now prints `↳ fix:` inline. | The two duplicate lock/trust explanations (`:490-515` and `:517-554`) go — no loss, `concepts` said it better. **Real loss:** the headless-grant recipe at `:616-629` must move, not vanish. |
| `ARCHITECTURE.md` | 600 | **KEEP, but fix `:91-128` first** | It is a CLAUDE.md-named authority — but its delivery section still teaches "modes you pick", the single worst contradiction in the corpus. | Nothing, if fixed. If deleted, `PriorTrust`/`TrustCarry` are named nowhere else (`:244-268`). |
| `automation.md` | 436 | **MERGE INTO `integrations.md`** | Same audience (integrators), same contracts; `automation.md` has **zero body inbound links** from anywhere on the site. | Nothing — it is a sibling of a page nobody bounces to it from. |
| `concepts.md` | 419 | **MERGE INTO the one page — anchors preserved** | Its content is absorbed, but it is the **canonical delivery-lane table** (`:284-289`) and 7 pages deep-link `#delivery-modes`. | **Losable if done carelessly:** `:83-87` is the best sentence in the corpus and must survive verbatim. Deleting the file without redirect stubs breaks fragment checks in `check-docs-site.py`. |
| `workflows.md` | 326 | **KEEP** | Separate threat model, separate evidence contract, own audience. | — |
| `integrations.md` | 298 | **KEEP + absorb `automation.md`** | **I disagree with the one-page draft here** — see §6. This is the only home for the t3code fixed-action contract. | — |
| `adapters.md` | 286 | **KEEP** | A truth table; shrinking it makes it a lie. | — |
| `start.md` | 278 | **MERGE INTO the one page** | It is the one page's §5 "First run" with three restatements of the four ideas around it. | Nothing new; but `start.html` is the #1 funnel URL and **must redirect**, not 404. |
| `faq.md` | 232 | **MERGE INTO the one page** | Every entry is answered elsewhere or is a troubleshooting entry with a question mark. Agrees with the draft. | `:213-218` ("trusted ≠ safe") is a good phrasing but the better one is `ENFORCEMENT.md:45-49`. |
| `migrations.md` | 204 | **KEEP — conditionally** | Separate audience (people leaving another tool), and **zero body inbound links**. | **This is the one real gap in the corpus:** it never mentions the profile→toolset rename — the single migration every existing user actually carries in their manifest. Keep the page *and add that*, or the rename stays documented only in two pages we are merging away. |
| `choose.md` | 76 | **DELETE** | Self-contradictory: the H1 asks "Which mode do I need?" while `:16` says "You do not have to choose", and `:50-51` routes the reader to the stale `ARCHITECTURE.md` section as the authority. | Nothing. `:21-26` (the per-row "why") should be lifted into the one page's delivery table before deleting. |
| `howto/trust-a-repo.md` | 148 | **MERGE INTO the one page — verbatim** | The best-written page in the corpus and the most inbound-linked how-to (11 body links). | **Real loss unless lifted verbatim:** `:38-54` is the only place the five refusals are mapped to their triggering commands, and `:56-71` the only place the three deliberate exclusions are stated. |
| `howto/name-a-toolset.md` | 137 | **MERGE INTO the one page** | The canonical toolset teaching artifact — definition, negative space, TOML path, worked example. | The two-toolset worked example has no equivalent; lift it. |
| `howto/add-a-skill.md` | 115 | **MERGE INTO the one page** | One journey with prose around it. | — |
| `howto/undo.md` | 96 | **MERGE INTO the one page** | Ditto. | — |
| `howto/ci.md` | 96 | **MERGE INTO the one page** | Ditto. | **Real loss:** `:54-75` is the only place that explains *why* the consent digest must come from the surface the job just printed. Lift that paragraph. |
| `howto/team-setup.md` | 79 | **MERGE INTO the one page** | 7 inbound links, but all to content the one page covers. | — |
| `howto/add-a-server.md` | 77 | **MERGE INTO the one page** | 77 lines for four commands. | — |
| `howto/lock-down-a-run.md` | 72 | **MERGE INTO the one page** | The run-posture table covers it. | — |
| `howto/see-what-happened.md` | 72 | **MERGE INTO the one page** | Absorbed by the `report` journey. | — |

### The hand-authored HTML — the draft does not cover this, and it is where the weight is

| page | lines | disposition | reason | what is lost |
|---|---:|---|---|---|
| `panel/index.html` | **2,626** | **DELETE** | 2,293 lines of JavaScript simulating a second UI, which `CLAUDE.md` forbids building. Already wrong: `:1190-1214` shows AgentStack *writing* `[profiles.*]`, contradicting `concepts.md:95-96`. | The visual sense of what the panel looks like. Replace with 3 screenshots in `integrations.md` — a screenshot cannot silently drift into a false claim. |
| `cookbook.html` | 901 | **DELETE** | 851 lines of hand-typed markup holding ~55 unverified transcripts. Every one is a drift candidate. | Genuinely: a browsable "here's a recipe" surface. The one page's §5 journeys are that surface, verified against the binary. |
| `tutorial/index.html` | 901 | **KEEP, rebuild** | The only artifact that teaches by doing rather than telling — worth its cost, unlike the other simulations. | Nothing, if rebuilt. Today it duplicates its own toolset definition (`:315` and `:602`) and re-teaches trust at `:304`. |
| `examples.html` | 434 | **MERGE INTO `tutorial/`** | 16 more hand-typed transcripts; overlapping purpose with `cookbook`. | The `#e20` anchor is deep-linked from the sidebar ("Reports & call audit"); re-point it. |
| `security-review-2026-07-11.html` | 425 | **MOVE to `docs/archive/`** | A dated point-in-time report with **zero body inbound links**, sitting in the live site. It is history, and `CLAUDE.md` says history lives in `archive/`. | Nothing — it stays readable, it just stops looking like current documentation. |
| `docs.html` | 249 | **KEEP, rewrite as a 9-item index** | With nine pages, the hub is a list, not a landing page. | — |
| `index.html` | 286 | **KEEP** | The pitch. | — |

### Generated and stubs

| page | lines | disposition | reason |
|---|---:|---|---|
| 22 `.html` twins | 8,832 | **GENERATE** (unchanged mechanism) | Already correct; shrinks automatically as sources shrink. |
| `reference.md:1893-1958` | 64 | **GENERATE — and grow it** | `agentstack x self docs --write` already owns this region and CI checks it. Every hand-typed flag list elsewhere should move inside it. |
| 5 redirect stubs | 75 | **KEEP** | 15 lines each to not break old URLs. Add 3 more (`choose.html`, `automation.html`, `start.html`). |

---

## 5. What I would NOT touch, and why

**`ENFORCEMENT.md` — keep it, untouched, at 1,383 lines.** The maintainer asked
for the argument either way. Here it is.

*The case for cutting it:* it is the second-largest file in the corpus and I
found it teaching trust in three separate clusters (`:45-49`, `:51-74`,
`:76-85`, `:667-671`, `:687`, `:735`, `:750`, `:790`, `:905-909`, `:945`). By the
overlap logic applied to every other page, that is duplication.

*The case for keeping it, which wins:* it is **hard-gated by CI**.
`tools/check-enforcement-pairing.py` enforces invariant 8 — any PR touching
`crates/trust/`, `crates/policy/` or `crates/egress/` must change
`ENFORCEMENT.md` in the same PR or carry a written `ENFORCEMENT-WAIVER:`
trailer. That gate is what stops the code and its claims drifting apart, and it
is anchored to *this file path*. Splitting, merging or renaming it would either
break the gate or silently weaken it, and the gate is worth more than 1,383
lines of tidiness.

Its repetition is also different in kind from the rest of the corpus: it repeats
"untrusted renders zero" once **per surface** (servers, skills, hooks,
instructions, extensions) because each is a separately-audited claim. That is a
matrix, not a duplicate. `reference.md` and `adapters.md` earn the same
exemption for the same reason — they are truth tables, and a shorter truth table
is a false one.

**Also untouched:** the generation mechanism. `make-docs-pages.py`,
`make-docs-sidebar.py` and `check-docs-site.py` work. This proposal deletes
pages, not tooling — the sidebar `TREE` shrinks, it does not change shape.

---

## 6. Where I agree with `plan/one-page-draft.md`, and where I do not

Two independent readings. The draft is right about most of the Markdown and
silent about where the real weight is.

**Agree, and my evidence strengthens it:**

- *All nine how-tos absorbed.* Agreed on the principle. My link graph confirms
  they are a closed cluster: they mostly link to each other, and
  `howto/add-a-server.md` has exactly **one** inbound link on the whole site.
- *`choose.md` deleted.* Agreed, and my reason is harder than the draft's: it is
  not merely redundant, it is **self-contradictory** and routes readers to a
  stale authority.
- *`faq.md` and `start.md` absorbed.* Agreed.
- *`troubleshooting.md` shrinks rather than disappears.* Agreed, and I can size
  it: 152 of its 877 lines are hand-typed simulated CLI output.
- *`ENFORCEMENT.md`, `reference.md`, `adapters.md`, `workflows.md` survive.*
  Agreed — and §5 above gives the CI reason the draft did not have.

**Disagree:**

1. **`integrations.md` is not redundant.** The draft says "§3 (adapter list)
   plus §6 (`gateway`, `mcp`, `shim`) is the whole of it." It is not. The page
   carries the **t3code fixed-action contract table** — `trust-preview`,
   `trust-server-blockers-v1`, `trust-review-card-v1`, `trust-card-diff-v1`
   (`:51-53`), `profiles-edit-batch-v1` (`:86`) — the named, versioned APIs an
   external panel is built against. A recent commit (`c9534e7`) added a witness
   keeping that table true against `FEATURES`. Deleting it would drop the only
   documentation of a contract the project has just built a test for. **KEEP,
   and absorb `automation.md` into it.**
2. **`concepts.md` cannot simply be deleted with "keep nothing".** Content-wise
   the draft is right. Mechanically it is a trap: seven pages deep-link
   `concepts.html#delivery-modes`, and `check-docs-site.py` validates fragments
   in both HTML and Markdown. This is a MERGE with anchor redirects, not a
   DELETE, and `:83-87` must survive verbatim.
3. **The draft accounts for ~5,400 lines of Markdown and stops.** The other
   **5,822 lines are hand-authored HTML** — `panel/index.html` alone is 2,626,
   larger than any two Markdown pages combined, and it is the single most
   drift-prone artifact in the repository. A docs-architecture proposal that
   does not name it has missed the largest object in the room.
4. **`migrations.md`: keep, but only if fixed.** The draft keeps it as a
   "genuinely separate audience". True — but the page is currently missing the
   one migration that matters most.
5. **`ARCHITECTURE.md`: the draft never mentions it.** It is a `CLAUDE.md`-named
   authority whose `:91-128` is the source of the delivery contradiction. It
   must be fixed **before** anything merges into the one page, or the one page
   will inherit the wrong model.

---

## 7. The sequenced plan — the site never breaks

Each step ends green: `tools/check-docs-site.py` passes, `sitemap.xml` matches
the tree both ways, and no internal link or fragment dangles.

**Step 0 — truth before tidiness (no page count changes).**
Fix `ARCHITECTURE.md:91-128` to teach routing, not chosen modes. Fix the stale
`profile` vocabulary at `troubleshooting.md:833`, `reference.md:753`
(`agentstack_create_profile` → `agentstack_create_toolset`), and
`reference.md:611`'s anchor. Add the profile→toolset rename to `migrations.md`.
*Nothing moves; the corpus stops lying.* Green by construction.

**Step 1 — retire the dead weight (−3,952 lines, 3 pages).**
Delete `panel/index.html` and `cookbook.html`; move
`security-review-2026-07-11.html` to `docs/archive/`. All three have zero or
near-zero body inbound links, so only the **sidebar `TREE`, `docs.html` and
`sitemap.xml`** need editing. Add redirect stubs for the two deleted URLs. This
is the largest single cut and it is also the safest — do it first for the
morale.

**Step 2 — build the one page beside the old site (+1 page, nothing removed).**
Land `docs/one-page.md`, generated into `one-page.html` by adding one row to
`PAGES` in `make-docs-pages.py`. Lift verbatim, before anything is deleted:
`howto/trust-a-repo.md:26-36` and `:38-71`; `concepts.md:83-87` and `:284-289`;
`howto/ci.md:54-75`; `howto/name-a-toolset.md`'s worked example;
`choose.md:21-26`. Add it to the sitemap and the `TREE`. *The site now has one
extra page and zero broken links.*

**Step 3 — merge the how-tos (−9 pages, −892 lines).**
For each of the nine, in inbound-link order (fewest first —
`add-a-server` has 1, `trust-a-repo` has 11, so it goes last): confirm its
content is in `one-page.md`, delete the `.md` and `.html`, repoint every inbound
link, drop it from `TREE` and `sitemap.xml`, add a redirect stub. Nine small
commits, each independently green.

**Step 4 — merge the four absorbed pages (−4 pages, −1,013 lines).**
`start.md`, `faq.md`, `choose.md`, `concepts.md` — in that order.
`concepts.md` is **last** because it owns the `#delivery-modes` anchor that
seven pages use: repoint those seven to `one-page.html#delivery` in the same
commit, then delete. `start.html` and `choose.html` get redirect stubs.

**Step 5 — fold `automation.md` into `integrations.md` (−1 page, −436 lines).**
Zero body inbound links, so this is a copy, a delete, a stub and a `TREE` edit.

**Step 6 — shrink `troubleshooting.md` (−~500 lines, 0 pages).**
Delete the duplicate lock/trust pair at `:490-554`. Replace hand-typed
transcripts with the binary's own `↳ fix:` output where it exists. Keep the
headless-grant recipe.

**Step 7 — rebuild `tutorial/`, absorb `examples.html` (−1 page).**
Last, because it is the only step that is real authoring rather than deletion.
Re-point the sidebar's `examples.html#e20` entry.

**Net: 28 published pages → 9 (+1 conditional). ~23,000 shipped lines → ~9,500.**

**The one guard rail:** after Step 1, add a CI check that fails if a fenced block
in `docs/**` contains `✓`/`⚠`/`↳` glyphs outside a file the generator owns. That
is what stops category (c) growing back, and without it this whole exercise
repeats in six months.
