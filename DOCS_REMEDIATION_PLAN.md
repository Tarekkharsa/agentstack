# Documentation remediation plan

> **Status:** executed 2026-07-21 (all batches; see git history for the change
> record — secret wording, tutorial commands + mobile shell, cookbook/demos/
> reference/dashboard commands, example READMEs, links/sitemap/responsive,
> and the regression gates in `docs_commands.rs`, `tools/check-docs-site.py`,
> `tools/site-smoke.mjs`, and the docs workflow)  
> **Created:** 2026-07-21  
> **Audit baseline:** AgentStack v0.15.0, local `main` at `84891ca`  
> **Companion evidence:** [`DOCS_AUDIT.md`](DOCS_AUDIT.md) and the 2026-07-21
> live-site-versus-repository review  
> **Project queue:** [`TODO.md`](TODO.md) remains the only product-wide ordered
> work queue. Promote one batch from this plan into `TODO.md` when execution
> begins.

## Outcome

Make the public documentation safe to follow literally and keep it aligned
with the shipped CLI. A reader should be able to copy any advertised command,
understand where secret values are stored, use the tutorial on a phone, and
reach every canonical page without encountering stale simulations or broken
links.

The implementation is not the general problem: the full workspace suite, all
11 committed example/assertion suites, and all 13 Docker-backed sandbox tests
passed during the audit. This plan therefore prioritizes documentation
correctness and regression prevention. The one confirmed functional product
defect in scope is the dashboard's invalid command for adopting discovered
skills.

## Working rules

1. Correct security-relevant claims before improving presentation.
2. Treat the real v0.15.0 CLI parser and executable examples as authority.
3. Edit Markdown sources for generated pages, then regenerate their HTML; do
   not hand-edit generated output.
4. Keep static-render, clean-at-rest, gateway, sandbox, and lockdown claims
   explicitly separated.
5. Every corrected command or claim must gain an automated witness where
   practical.
6. Publish only after the local site, generated pages, tests, and live-site
   artifact are in agreement.

## Priority and batch order

| Batch | Priority | Theme | Exit condition |
|---|---:|---|---|
| 0 | P0 | Establish one deployable baseline | Local and public-page differences are understood; partial fixes are completed |
| 1 | P0 | Correct secret-placement claims | Every surface distinguishes portable refs from plaintext native config |
| 2 | P0 | Repair tutorial correctness and mobile use | All tutorial commands parse; every lesson works at 390 px and desktop |
| 3 | P1 | Repair cookbook, demos, reference, and dashboard actions | All displayed/copied commands match the CLI; dashboard offers a valid skill action |
| 4 | P1 | Reconcile examples and historical findings | READMEs describe current PASS behavior; history is clearly archival |
| 5 | P2 | Fix navigation, indexing, and responsive layout | Zero broken fragments; all canonical pages are indexed; no document-level mobile overflow |
| 6 | P1 | Add regression gates and build hygiene | Dynamic snippets are parser-tested; link/mobile/deployment checks run in CI |

---

## Batch 0 — establish one deployable baseline

The local branch advanced during the audit. The public site still serves the
older cookbook, docs hub, and CI how-to, while local commit `84891ca` contains
a partial recipe-count renumber and a v0.15.0 Action pin.

### Work

- [ ] Review the local cookbook renumber as one coherent change:
  - choose **25 recipes** as the canonical count;
  - change the HTML description, Open Graph description, visible introduction,
    rail labels, card badges, IDs, and links to one contiguous 1–25 sequence;
  - ensure recipe names and numbers agree between the rail and cards.
- [ ] Keep the corrected `Tarekkharsa/agentstack@v0.15.0` pin in
  `docs/howto/ci.md`, regenerate `docs/howto/ci.html`, and update the stale
  example version in `action.yml`.
- [ ] Regenerate any page whose source changed.
- [ ] Compare the complete deployable `docs/` tree with the intended Pages
  artifact before publishing.
- [ ] Deploy, then repeat the live hash/link check so this batch is not closed
  merely because local files are correct.

### Acceptance criteria

- [ ] Cookbook metadata, introduction, navigation, anchors, and cards all say
  and contain 25 recipes.
- [ ] The Action example is v0.15.0 everywhere.
- [ ] Every public HTML page is byte-equivalent to its intended local artifact,
  excluding deployment-specific files if any are documented.

## Batch 1 — make secret placement precise everywhere

### Canonical claim

Use this meaning consistently, adjusting prose for context:

> Secret values never enter the portable manifest or lockfile. Static native
> configurations may contain resolved plaintext when the target format
> requires it. Gateway-backed delivery resolves values host-side without
> placing them in project-native configuration.

Do not use an unqualified statement such as "secrets stay `${REF}`" beside an
`apply --write` example.

### Work

- [ ] Correct the landing-page claim in `docs/index.html`.
- [ ] Correct both claims in `docs/start.html`.
- [ ] Correct the placeholder definition in `docs/concepts.md`, then regenerate
  `docs/concepts.html`.
- [ ] Correct tutorial lesson copy, terminal output, and the secrets quiz in
  `docs/tutorial/index.html`.
- [ ] Correct the native-shape and central-library simulations in
  `docs/examples.html`.
- [ ] Audit `README.md`, `docs/ARCHITECTURE.md`, `docs/ENFORCEMENT.md`,
  `docs/reference.md`, all how-to sources, and example READMEs for the same
  unqualified wording.
- [ ] Correct the `agentstack explain` message in
  `crates/cli/src/commands/explain.rs`; state what remains a ref and where a
  static render may place a value.
- [ ] Keep `examples/one-manifest-demo/` as the executable authority and link to
  it from user-facing explanations where more detail is useful.

### Tests

- [ ] Add or extend a test for `explain` output so it cannot claim that static
  native configurations never receive plaintext.
- [ ] Run `examples/one-manifest-demo/run-demo.sh` and preserve its assertions:
  native configs contain the resolved test value; manifest and lockfile do not.
- [ ] Search the documentation corpus for `never written`, `never values`,
  `secrets stay`, and `resolve in memory`; review every remaining match.

### Acceptance criteria

- [ ] No static-render page implies that `${REF}` is written verbatim to all
  native configs.
- [ ] Delivery-mode-specific claims agree with the enforcement and architecture
  documents.
- [ ] The executable one-manifest witness and all prose describe the same
  behavior.

## Batch 2 — repair the interactive tutorial

### Command corrections

- [ ] Replace the fictional `v1.4.2` installer output with the current release
  or version-neutral output generated from one shared value.
- [ ] Replace `agentstack adopt context7` with the real drift-adoption flow:
  `agentstack adopt --write`, optionally scoped with `--target`.
- [ ] Add `--write` where `agentstack add from` is described as mutating the
  manifest; do not claim that it rendered native configs unless a separate
  apply/use step did so.
- [ ] Replace every `agentstack run claude` example with
  `agentstack run claude-code`.
- [ ] Remove `agentstack mode zero-files --write` and teach the actual gateway
  or clean-at-rest workflow.
- [ ] Replace `agentstack session start --profile <p>` with
  `agentstack session start <PROFILE>`.
- [ ] Replace `verify --pubkey team.pub` with a literal 64-hex public key example
  or explicitly show the shell step that reads a file into the argument.
- [ ] Remove restore output that refers to nonexistent writes.
- [ ] Re-run every tutorial control and confirm exactly one lesson pane is
  visible after each transition.

### Mobile repair

- [ ] Identify the tutorial's inner horizontal flex shell and switch it to a
  vertical layout below 760 px; changing `#app` alone is insufficient.
- [ ] Keep lesson navigation horizontally scrollable without allowing it to set
  the width of the lesson content.
- [ ] Verify content begins inside the viewport at 320, 390, 760, and 1280 px.
- [ ] Test long commands, quiz feedback, wizard output, comparison tables, and
  the completion state at each width.

### Acceptance criteria

- [ ] Every displayed/copied tutorial command parses against the shipped CLI.
- [ ] Every simulated mutation says whether it is a preview or a write.
- [ ] At 390 px the active lesson heading and body are visible without horizontal
  page scrolling.
- [ ] All 11 lessons and interactive widgets work without console errors.

## Batch 3 — repair cookbook, demos, reference, and dashboard actions

### Cookbook

- [ ] Replace adapter ID `gemini-cli` with `gemini`.
- [ ] Replace `agentstack lib add server github` with the real reusable-server
  flow, such as `agentstack lib add-server <name> --file <file> --write`.
- [ ] Replace or remove `agentstack add github`; use the correct `add from`,
  `add server`, or manifest library-name flow for the intended result.
- [ ] Replace retired `agentstack add pack ...` syntax with the supported
  provider/git-pack flow.
- [ ] Add the required CLI ID to the generic sandbox invocation.
- [ ] Review every `data-copy` value separately from its visible terminal line.

### Demos page

- [ ] Correct the `lib add-server` and `lib add` simulations, including required
  names, flags, and `--write` semantics.
- [ ] Change the `doctor --ci` explanation: it fails on errors, drift, policy
  violations, and unsafe content—not every warning.
- [ ] Derive simulated output from, or test it against, the corresponding
  executable fixture wherever feasible.

### Reference and dashboard

- [ ] Replace both `agentstack adopt <name>` references. Explain that `adopt`
  imports native **server drift** and takes no positional name.
- [ ] Decide the supported action for an on-disk skill not in the manifest:
  `agentstack add skill`, `agentstack lib add`, or another real flow.
- [ ] Update the dashboard copy/action in
  `crates/cli/src/dashboard/assets/app.js` to offer that supported skill action.
- [ ] Add a UI/unit assertion for the copied command so this cannot regress.

### Acceptance criteria

- [ ] Every visible command and every copy-button value agrees.
- [ ] Commands parse through the real v0.15.0 command tree.
- [ ] The dashboard never recommends a server-only operation for a skill.
- [ ] Reference prose and dashboard behavior describe the same operation.

## Batch 4 — reconcile examples and historical findings

### Work

- [ ] Update `examples/projects/restricted-folders/README.md` and the bundled
  manifest comments: project-layer filesystem denies now pass from both the
  preferred and legacy layouts; update the current Codex decision-envelope
  behavior.
- [ ] Update `examples/projects/per-cli-instructions/README.md`: unsupported
  delivery now warns, and unknown adapter targets are rejected.
- [ ] Update `examples/projects/multi-cli-webapp/README.md`: Cursor's unsupported
  instructions/skills delivery is warned rather than silently dropped.
- [ ] Rework `examples/projects/FINDINGS.md` so the original v0.10.1 evidence is
  unmistakably archival. Either add per-finding resolved banners or move old
  present-tense tables under a clearly historical section.
- [ ] Review every example README for `SKIP`, `known limitation`, `silent`, and
  `defect`; compare each statement with current assertion output.

### Acceptance criteria

- [ ] A README's expected PASS/SKIP/FAIL totals match its current script.
- [ ] Resolved defects are not presented as current behavior.
- [ ] Historical evidence remains available without competing with current
  specification.

## Batch 5 — navigation, indexing, and responsive layout

### Links and sitemap

- [ ] Change `start.html`'s `examples.html#e13` link to the current lease-demo
  anchor, expected to be `examples.html#lease`.
- [ ] Change `start.html`'s `index.html#start` link to the intended existing
  landing-page anchor, expected to be `index.html#install`.
- [ ] Point the historical security-review page at `enforcement.html` and
  `history.html`, not raw Markdown.
- [ ] Expand `docs/sitemap.xml` from 7 entries to every canonical public page:
  reference, concepts, architecture, enforcement, history, choose, all how-to
  pages, and other non-redirect canonical surfaces.
- [ ] Keep redirect stubs out of the sitemap unless there is a documented SEO
  reason to include them.

### Responsive layout

- [ ] Contain or wrap the mobile command table on `docs.html`.
- [ ] Contain the enforcement matrix instead of making the whole page overflow.
- [ ] Allow long inline code in architecture and reference pages to wrap or
  scroll within its own container.
- [ ] On cookbook, start, and examples pages, collapse long navigation rails so
  the main heading is reached quickly on mobile.

### Acceptance criteria

- [ ] Zero broken internal links or fragments across all 26 published pages.
- [ ] Every canonical page appears in the sitemap exactly once.
- [ ] No document-level horizontal overflow at 390 px.
- [ ] Redirect pages still reach their canonical destinations.
- [ ] All external links still return successfully.

## Batch 6 — regression gates and build hygiene

### Command validation

- [ ] Extend `crates/cli/tests/docs_commands.rs` or add a dedicated extractor
  that reads commands from:
  - Markdown fenced blocks;
  - HTML `<pre>` and `<code>` blocks;
  - `data-copy` attributes;
  - tutorial JavaScript command objects;
  - terminal simulation line arrays.
- [ ] Normalize prompts, comments, placeholders, and intentional pseudocode,
  then validate all real commands using Clap's `try_get_matches_from`.
- [ ] Maintain a small, explicit allowlist for examples that intentionally stop
  before execution; do not silently skip unrecognized shapes.

### Site validation

- [ ] Add an internal link-and-fragment crawler to CI.
- [ ] Add a sitemap-to-canonical-page consistency check.
- [ ] Add browser smoke tests at 390 and 1280 px for the landing page, tutorial,
  docs hub, cookbook, examples, start guide, reference, and enforcement matrix.
- [ ] Assert tutorial pane visibility and horizontal overflow, not screenshots
  alone.
- [ ] Add a generated-page cleanliness check that fails when Markdown sources
  and committed HTML differ.
- [ ] Add a post-deploy smoke job or documented release checklist that verifies
  the public Pages artifact rather than only the repository tree.

### Build hygiene

- [ ] Add `.claude/` to `.dockerignore`; review other generated native CLI
  directories for exclusion without excluding required source fixtures.
- [ ] Assert an upper bound on the Docker build-context input where practical.
- [ ] Confirm repeated sandbox tests reuse the egress-sidecar image layer rather
  than retransmitting gigabytes of unrelated generated state.

### Acceptance criteria

- [ ] A deliberately invalid dynamic tutorial or copy-button command fails CI.
- [ ] A deliberately broken fragment fails CI.
- [ ] A deliberately overflowing tutorial shell fails the browser smoke test.
- [ ] The egress-sidecar Docker context contains workspace source, not local CLI
  state.

---

## Verification matrix for every batch

Run the narrow checks during development and the complete gate before deploy.

```sh
# Generated documentation and command inventory
cargo test -p agentstack --test docs_commands

# Full default workspace gate
cargo test --workspace --all-targets

# Executable documentation/examples
DEMO_PAUSE=0 examples/one-manifest-demo/run-demo.sh
# Run the remaining committed run-demo.sh/assert.sh suites through the same
# repository helper or documented audit loop.

# Docker-backed enforcement gate
cargo test -p agentstack-egress --test sidecar_image -- --include-ignored --nocapture
cargo test -p agentstack --features sandbox \
  --test sandbox_egress \
  --test sandbox_cli_e2e \
  --test sandbox_fs \
  --test sandbox_lockdown \
  --test sandbox_gateway_e2e \
  -- --nocapture

# Final repository check
git status --short
```

Also run the site crawler, viewport smoke suite, and live deployment comparison
introduced in Batch 6.

## Final release gate

The remediation is complete only when all of the following are true:

- [ ] Security and secret-placement wording is delivery-mode-specific.
- [ ] Every documented or copied CLI command is parser-valid.
- [ ] The tutorial works completely at 390 and 1280 px.
- [ ] Cookbook count, numbering, metadata, navigation, and content agree.
- [ ] Dashboard skill actions use a supported command.
- [ ] Example READMEs match current executable assertions.
- [ ] Internal links, fragments, sitemap entries, and external links pass.
- [ ] Generated Markdown/HTML pairs are synchronized.
- [ ] Default workspace tests, all example assertions, and Docker enforcement
  tests pass.
- [ ] The public site matches the reviewed local artifact after deployment.
- [ ] The worktree is clean and the completed changes are recorded in
  `docs/HISTORY.md` if they alter a previously published security claim.

