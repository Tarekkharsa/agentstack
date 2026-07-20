# Docs Audit — three-reader walkthrough (v0.14.0)

Audited 2026-07-20 against the release binary and the full doc estate: README,
`docs/*.md`, all 10 GitHub Pages sources, 14 example READMEs, and all 83
`--help` screens. Companion to [DX_AUDIT.md](DX_AUDIT.md), which covers runtime
CLI behavior; this file covers what a reader learns and how fast.

**Honest headline first: the structure most docs audits recommend already
exists here.** There is a landing page with real terminal demos, a two-track
10-minute tutorial with expected output at every step, a task-organized docs
hub with an "I want to…" index, a complete generated command list that CI
keeps honest, an exemplary enforcement matrix, redirect stubs for every moved
page — and the mechanical sweeps came back clean: **zero broken links** (519
checked, checker validated against deliberately-broken controls) and **zero
command/flag mismatches** (all 35 documented `agentstack` invocations verify
against the real binary).

What actually stands between a newcomer and "using it confidently in 10
minutes" is not missing structure. It is four things, in order of impact:
the **register** of the entry-page prose (every sentence carries 2–3 concepts),
the **absence of one concepts page** (terms are defined far from first use, or
only in `--help`, or never), a handful of **terminology splits** (the same
thing has up to four names across docs), and **findability of task answers**
inside the 1,521-line reference (complete, but narrative — answers are
embedded mid-sentence, not led by commands).

Ranking below: highest impact first. §A is the persona baseline to re-test
after any rewrite. §B is the findings. §C lists what verified clean (do not
churn it). §D flags CLI bugs (fix in code, not docs).

---

## A. Persona baseline (re-run after rewrite)

### A1. The skeptic (30 seconds) — **fails the first paragraph, wins by line 30**

The README's opening blockquote is the highest-stakes text in the project and
it front-loads seven unglossed terms: *control plane, manifest, trust-gates,
firewalls, audits, governed gateway, contained runs* — and closes with
"enforce and record exactly the controls their printed **posture** names,"
which is opaque until `docs/reference.md:937`, ~1,900 lines of reading later.
The site landing tagline has the same shape ("…exactly what their printed
posture names"). A skeptic who parses sentence-by-sentence bounces here.

The recovery is fast — "Try it in 60 seconds" and the "Why" section (README
lines 17–53) are genuinely strong: concrete commands, the `npm install with an
agent attached` framing, and the honest "you may not need this yet" line. The
problem is purely the 3 sentences before them.

**Worst single term: MCP.** "Model Context Protocol" is never spelled out
once — not in README, not on any site page, not in any `docs/*.md`
(full-corpus grep). It is the most repeated undefined term in the project and
the entry docs assume it on line 34.

### A2. The newcomer (10 minutes) — **passes, with one cognitive spike**

There IS one obvious linear path, twice over: README steps 1–6 (with a
stop-anywhere ladder table) and `start.html` Track A/B with a "You should see"
block at every step. Install → first success is real: `curl | sh` →
`agentstack init` → done, and I verified the orientation screen, `status`,
`--help`, and `--help --all` all match what the docs claim.

Concepts required before the first win: manifest, `${REF}`, target/adapter,
delivery mode, profile, doctor — about six. The spike is the **delivery-mode
fork** (static / clean-at-rest / zero-files) at roughly minute 3, inside the
wizard: the newcomer is asked to choose between three architectures they
cannot yet evaluate. Mitigations already exist (static is default, "you can
switch later", one-line help per option, fork panels in start.html), but the
docs-side answer — a short "which mode do I need?" decision page — doesn't
exist. The closest thing is the decision table in
`ARCHITECTURE.md#operating-model`, written for architects in different
vocabulary (see B4).

### A3. The returning user (searching) — **mixed**

| Question | Verdict |
|---|---|
| "what does trust actually do?" | **Excellent.** `ENFORCEMENT.md` "What trusted does and does not mean" is a model answer, findable from hub → Protect. |
| "what's enforced in `--sandbox`?" | **Excellent.** The matrix row + per-cell notes answer it precisely. |
| "how do I undo apply?" | **Good.** `restore` has a section and a hub "I want to…" entry. |
| "how do I add a server?" | **Poor.** Four verbs apply (`add server`, `add from <id>`, `adopt`, `lib add-server`) plus hand-editing the manifest; no page puts them side by side; the hub's task list doesn't include this task. See B5. |
| "how do I share config with a team?" | **Weak.** One README paragraph (step 5); no dedicated page; not in the hub task list. |

The general returning-user problem is reference.md's register: complete and
accurate, but organized as narrative prose. The answer to "how do I X" is
typically the middle clause of a 40-word sentence. Section-level findability
is good (real TOC); answer-level findability is not.

---

## B. Findings, ranked by impact

### B1. Entry-page register: unglossed jargon in the first three sentences

The single highest-impact edit in the project is the README blockquote +
landing tagline. Same facts, glossed inline, shorter sentences:

**Before** (README lines 3–8):

> Cloning a repo shouldn't hand your agent to a stranger.
> AgentStack is a local control plane for AI coding agents: one reviewed
> manifest defines what every agent CLI may run — then trust-gates,
> firewalls, and audits it. An untrusted repo's declarations are never
> auto-activated, and governed gateway and contained runs enforce and
> record exactly the controls their printed posture names.

**After** (illustrative — preserves every claim, defines at first use):

> Cloning a repo shouldn't hand your agent to a stranger.
> AgentStack puts everything your AI coding tools (Claude Code, Codex,
> Cursor, …) are allowed to run into one reviewed file. A repo you clone
> can't activate any of it until you approve that repo — and what you do
> approve runs behind a firewall, with every call logged. Each run is
> labelled with how strongly that is actually enforced, and the label
> never overstates.

The same treatment applies to the landing tagline and to the "Why" bullets'
lead-ins (`README.md:40-50` uses *brokered*, *machine policy*, *lockfile*
before defining any of them — the jargon inventory in the sweep found 13
terms used before definition on the two entry surfaces, vs. 3 defined at
first use).

**Fix shape:** rewrite only the first ~50 lines of README + the landing hero;
add "(Model Context Protocol — the plugin standard agent CLIs use for tools)"
at the first "MCP" on each entry surface. Everything from "Try it in 60
seconds" down mostly survives.

### B2. No concepts page — definitions live in `--help`, in footnotes, or nowhere

Today the vocabulary is scattered across four partial glossaries: the
`--help` "Words:" footer (3 terms — the *only* place CLI ≡ harness is ever
declared), reference.md's "A few words used throughout" (the same 3),
ARCHITECTURE's primitives table (8 rows, different vocabulary), and inline
apposition when the author remembered. Terms with **no** definition anywhere
a newcomer will look: *MCP, posture, brokered, lockfile-vs-manifest
relationship, harness, lease, egress, consent digest*.

**Fix shape:** one `docs/concepts.md` (~1 screen per group): manifest &
lockfile · profile · adapter/target/CLI(harness) · trust & consent digest ·
policy (machine ∩ project) · gateway · guard · sandbox vs lockdown vs
`--locked` · posture · the three delivery modes · library vs catalog vs
registry vs trust store · drift. 2–3 plain sentences each + one relationship
diagram. Every other page links here instead of re-explaining — which also
shrinks README and reference.md.

### B3. "Posture" names two different things

The per-run **execution posture** (`HOST / ADVISORY` … `LOCKDOWN / ENFORCED`)
and doctor's one-word **machine-policy posture** (`open` / `restrictive` /
`mixed`) share the word. reference.md disambiguates with adjectives;
README line 8 and the landing page use bare "posture" for the first sense
only. A reader who meets doctor's `restrictive` will reasonably expect it to
be one of the four run labels.

**Fix shape:** keep "posture" for the per-run label (it's the brand); rename
doctor's line to "machine policy: open/restrictive/mixed" without the word
"posture", or gloss it where printed. Definitions land in the concepts page.

### B4. The three delivery modes have four naming schemes

The canonical trio **static / clean-at-rest / zero-files** is consistent
across README, reference.md, and the whole site — but:

- `ARCHITECTURE.md` never uses two of the three names. It calls the same
  mechanisms "static render / native session / MCP lease (profile lease)"
  with zero cross-reference. A reader arriving from the tutorial cannot map
  the decision table onto the modes they just chose between.
- The concept itself is named four ways: "delivery mode" (wizard,
  start.html), "Where rendered files live (three modes)" (reference.md
  section), "Rendered-file modes" (docs hub), "artifact modes"
  (reference.md:670, house-rules fragment).

**Fix shape:** pick "delivery mode" (the wizard's word — it's what users see
first) as the concept name everywhere; add one sentence to ARCHITECTURE
mapping its mechanism names onto the three user-facing mode names.

### B5. Returning-user task answers: no how-to layer

The hub's "I want to…" list is the right idea and already covers 12 tasks —
but each entry links into a long page, and several real questions have no
entry at all. The worst: **adding a server** spans four verbs whose
differences are documented in four places (README table, reference
`#adopt-and-add`, `#adding-capabilities`, `#search-across-providers`) and
compared nowhere:

| You have | Use |
|---|---|
| A server's config details (URL/command) | `agentstack add server` (or `set server` to upsert) |
| Just a name — find it in catalog/registry | `agentstack search` → `agentstack add from <id>` |
| Already hand-added it to one CLI's config | `agentstack adopt --write` |
| Want it reusable across projects by name | `agentstack lib add-server` + reference it in a profile |

That table doesn't exist in the docs; every cell of it does, scattered.

**Fix shape:** a small `docs/howto/` layer (or sections on one page), one
screen each, commands first, prose after: add a server (the table above) ·
share a setup with a team/CI · trust a cloned repo · lock down a run · undo
anything · use in CI. Most content is lift-and-reshape from reference.md, not
new writing.

### B6. reference.md answers are complete but not scannable

reference.md is the map, and its TOC + generated "All commands" section are
genuinely good. But entries are narrative: e.g. the answer to "what does
`lib add` do with my source dir?" is the middle of a 60-word sentence
(`reference.md:530-536`). The register that makes ENFORCEMENT.md excellent
(it argues one claim per cell) makes a *reference* slow.

**Fix shape:** don't rewrite content — restructure entries to lead with the
command form and a 1-line answer, prose after ("synopsis → common examples →
edge cases"). Also: link each "All commands" line to its section anchor
(today the list names commands but doesn't link them).

### B7. README omits `--features sandbox` for source builds — real trap

`start.html` correctly says release binaries have sandbox compiled in and a
bare `cargo build` omits it. README's Install section shows only
`cargo build --release` + `self link` — a source-builder following README
gets a binary where `run --sandbox` silently doesn't exist, and nothing in
README warns them. One-line fix, but it's a correctness gap between the two
surfaces, and step 6 depends on it.

### B8. "Bundle" is triple-overloaded

Three meanings in the corpus: (1) ARCHITECTURE's word for the manifest unit
(the split is acknowledged at `ARCHITECTURE.md:12` — but only there, so a
reader who *starts* in ARCHITECTURE never learns the public name is
"manifest"); (2) the age-encrypted `export`/`import` artifact (README:310,
reference:859); (3) a literal `bundle/` directory in 7 example projects —
sense (1) leaking into user-facing walkthroughs the README never prepares.

**Fix shape:** "manifest" is the user word everywhere user-facing;
ARCHITECTURE may keep "bundle" as the internal/strategic term but should
state the mapping in its intro, not a parenthetical; call the export artifact
an "encrypted archive"; leave example dirs (cosmetic).

### B9. Long deep docs lack TOCs; audience lines missing on the deep pair

`ARCHITECTURE.md` (493 lines) and `ENFORCEMENT.md` (432) are the only
>300-line pages without an in-document TOC (reference.md has one). Neither
states its audience/prerequisites up front (ENFORCEMENT states scope well,
not audience). Cheap fixes with real jump-in value, since both are pages
people enter mid-document from links.

### B10. Minor, listed for completeness

- **render vs compile** used interchangeably for the same `apply` action,
  including within README itself (line 46 "renders" vs 152 "compiles").
  Pick "render" (dominant); reserve "compile" for policy → `CompiledRuleset`.
- **"preset" vs "profile"** — genuinely different things (policy starter
  files vs activation sets), never confused in the docs, but nothing warns a
  newcomer they're unrelated. One sentence in the concepts page.
- The hub's Reference blurb says "~47 subcommands"; the generated list has
  36 top-level entries. Verify the count or drop the number.
- Docker prerequisite for `examples/sandbox/` is stated mid-page, not up
  front (most example READMEs otherwise state prereqs well).
- The wizard's fork output in `start.html` was spot-verified only at the
  fragments I could safely reproduce (orientation screen, help screens, mode
  descriptions); a full interactive-wizard transcript re-check should ride
  along with any rewrite that touches those blocks.

---

## C. Verified clean — do not churn

- **Links:** 0 broken of 519 (md + html, anchors included).
- **Commands:** 0 mismatches across all 35 documented invocations; every
  flag exists on its command.
- **Terminology that is already disciplined** (the sweep's explicit
  verdicts): *trust / consent digest / pin / gate* (layered deliberately),
  *drift* (single noun everywhere), *secrets* (`${REF}` / keychain /
  varlock stable), *profile*, *library vs catalog vs registry* (kept apart
  even in the same sentence), and the *sandbox / lockdown / guard / gateway*
  quartet — precisely distinguished everywhere, including their honest
  limits.
- **Honest-posture language** is consistent across every surface
  ("unapproved egress is blocked", never "exfiltration is impossible";
  guard is "cooperative… accidents, not a determined attacker" in README,
  ENFORCEMENT, guard-demo, and `guard --help` alike). Any rewrite must
  preserve this register verbatim in spirit.
- **Redirect stubs** for the 5 moved pages are intentional (noindex,
  0s refresh) — orphaned by design, keep them.
- The `--help` top screen (grouped map + Words footer + `--help --all`) and
  the bare-`agentstack` orientation screen are better than most CLIs ship.

## D. CLI bugs surfaced by the audit (fix in code, not docs)

From the full 83-screen help capture:

1. `setup`'s line in `--help --all` leaks an internal design reference:
   "…(P27: one front-door verb)".
2. `add skill --write` has an **empty** help string (unlike every sibling).
3. The `<NAME>` positional is undescribed on `add server`, `add skill`,
   `set server`, `secret set/get/rm` (while `lib add`, `remove`,
   `adapters show` describe theirs).
4. `--target` and `--scope` are fully described on `apply`/`setup` but blank
   or bare on `instructions`, `adopt`, `use`, `diff`, `restore`.
5. Six different phrasings of the `--write` boolean across commands.
6. `doctor`'s one-liner promises "quirks" — a word defined nowhere in the
   help tree.
7. Only 3 of ~80 help screens contain a usage example; the shapes that most
   need one (`add server`, `settings set` key syntax, `lib add-extension`,
   `explain`) have none.

(Runtime-behavior issues — banner-before-validation, missing-manifest error
text, etc. — are already tracked in DX_AUDIT.md and not repeated here.)

---

## E. What "10x" means here, concretely

Given C, the leverage is not a restructure. Ranked by
reader-minutes saved per line changed:

1. B1 + B2 (entry register + one concepts page) — fixes the skeptic and
   halves the newcomer's concept load.
2. B5 (how-to layer / task table) + B6 (reference entry shape) — fixes the
   returning user.
3. B3 + B4 + B8 (the three real terminology splits) — removes the silent
   comprehension taxes.
4. B7 + B9 + B10 + D — correctness and polish.

A Phase 2 proposal should map these onto the Diátaxis frame the existing
site already approximates (landing=pitch, start.html=tutorial,
hub=index, reference.md=reference, ARCHITECTURE/ENFORCEMENT=explanation) —
the missing quadrant is **how-to guides**, plus the concepts page that
Diátaxis puts inside explanation.

---

# Phase 2 re-test — before/after (2026-07-20, post-rewrite)

The restructure landed per [docs/design/docs-restructure.md](docs/design/docs-restructure.md):
new `docs/concepts.md`, `docs/choose.md`, six `docs/howto/` guides, README
rewritten to 200 lines with a captured demo, reference.md reshaped
command-first (64 headings preserved, old anchors kept), ARCHITECTURE/
ENFORCEMENT gained TOCs + audience lines + the mode-name mapping
(ENFORCEMENT's diff verified pure-insertion), terminology unified, site nav
rewired (sidebar generator updated — it had drifted from a past hand-fix),
and the CLI help fixes shipped (736 tests green, `self docs` regenerated).
The three personas were re-run as fresh-eyes agents barred from reading this
audit.

## Persona results

**Skeptic (30s): fail → PASS.** Before: seven unglossed terms in the first
three sentences; MCP never expanded anywhere in the corpus. After: both front
doors land what/problem/why inside 30 seconds; MCP is expanded at first use;
the reader called the `npm install` line "the clearest sentence in either
document." Residual (fixed post-test): the hedge-forward clause ("the label
never overstates") was dropped from the README header and the site lede
rewritten assertively. Open brand question for the maintainer: the site h1
("Build, govern, and run your agent stack") still reads config-manager, not
security — the README hook is sharper; aligning them is a brand call, not a
docs edit.

**Newcomer (10 min): pass-with-friction → PASS (conditional).** The happy
path is 4 commands, ~5 minutes, all defaulted. The re-test's residual
frictions were each fixed after the run: the three competing entry blocks
merged into one Start flow; the "forks the rest of the run" framing softened
to "press Enter for static, switch later"; the dangling "six capability
kinds" now names all six. The remaining spike is CLI UX, not docs: the wizard
still asks two 3-way questions (secrets home, delivery mode) mid-`init` —
tracked as a DX follow-up (defaults-first prompt design).

**Returning user: mixed → PASS.** All five audit questions now resolve in
one hop from the hub, commands-first ("add a server" via the four-verbs
table; trust's do/don't; undo; sandbox-vs-lockdown; team setup). The re-test
found the how-tos were reachable only from the hub's task grid, not the
persistent sidebar — fixed in `tools/make-docs-sidebar.py` (new How-to group
+ Concepts + Which-mode + trust-limits entries on all three sidebar pages
and the index docs-map).

## Verification outcomes (what the adversarial passes caught, all fixed)

1. **Three honesty regressions in the rewritten README front matter** — the
   flagship header and Why-bullet dropped the "brokered"/"auto-" qualifiers
   ("logs every call" where host mode logs nothing). Restored; the claims are
   again scoped to the gateway path, matching ENFORCEMENT. Everything else —
   concepts, choose, all six how-tos — passed the adversarial honesty pass
   outright.
2. **Phantom command in reference.md** — `agentstack upgrade <pack>` (which
   predates the rewrite as prose and was promoted to a fenced synopsis);
   the real verb is `lock --upgrade`. Fixed in all four spots.
3. **B3 closed in code** — doctor's section label renamed
   `Machine policy posture` → `Machine policy` (doctor.rs + progressive
   test + policy-intersection assert + both policies examples);
   `examples/projects/policy-intersection/assert.sh` passes 15/15 against
   the rebuilt binary. "Posture" now means only the per-run label,
   everywhere.
4. **Stale sidebar generator** — `tools/make-docs-sidebar.py` still pointed
   Dashboard at nonexistent `docs/dashboard.md` (the live HTML had been
   hand-fixed without updating the generator; regenerating would have
   regressed it). Generator reconciled, then regenerated.
5. Duplicated drift sentence in reference.md removed; hub's subcommand count
   corrected to the verifiable 37.

## Known follow-ups (out of docs scope, flagged)

- `agentstack init` ancestor-dir search escapes `AGENTSTACK_HOME` isolation,
  breaking `examples/sandbox/demo-firstrun.sh` on machines where the repo
  lives under a `$HOME` that has a machine manifest.
- Bundled catalog assets (`using-agentstack` skill, house-rules fragment)
  still say "artifact mode" — needs a code-side pass to say "delivery mode".
- Per-command section links inside reference.md's generated block need a
  `self docs` generator change.
- Wizard defaults-first prompt design (the two mid-init forks).
