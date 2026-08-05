# AgentStack work queue

> **Purpose:** the only ordered product-wide work queue
>
> **Status:** re-seeded 2026-08-02 from `STRATEGY.md` v3 ("The plan"). The
> queue is the maintainer's to reorder; deviations edit this file, never the
> strategy. The strategy reopens only at its named revisit triggers.

1. [x] W2 — trust checked at dispatch (security first; hardens the shipped lease path)
2. [x] The delivery arc — W1, W3, W5, then W4 last; dynamic becomes the default at arc-end
3. [x] Library inversion — link-your-folders onboarding (multiple sources), library-first authoring, `init` imports into a linked folder
4. [x] Instructions — per-(CLI, model) variants over the injection channels, with the per-harness honesty matrix
5. [x] Surface finish — grouped review card; `run` locked by default; Varlock productization
6. [x] Workflows promotion — per-role model/effort, algorithm helpers, security findings closed, un-hidden
7. [x] Packaging — toolset into a self-run image
8. [x] The panel — the new surfaces over the existing ui-contract
9. [x] Vocabulary, settled 2026-08-02 — three renames before the bar is judged, so the surface being judged is the final one:
      - **`toolsets` becomes the manifest key.** `[toolsets.X]` is real; `[profiles.X]` keeps working as a silent alias so no existing manifest breaks. `--toolset` is primary, `--profile` an accepted older spelling (`run` already does this). Toolset is one of the four ideas; the mechanism noun must not outrank it in the file users read.
      - **"Package" means the library composition only.** The image artifact is only ever called an image (`agentstack image` already). A copy audit removes "package" wherever it means the artifact.
      - **The Mode axis retires.** `status` shows Delivery alone — what happened, per harness, and where. `set-mode` is retired and its ui-contract feature string marked superseded; the contract is versioned for exactly this. Keeping Mode would reintroduce the concept v3 deleted.
10. [x] Surface reduction — adopted 2026-08-04, before the bar is judged, so the surface being judged is the final one. `agentstack --help` lists fifteen verbs, not eighteen: init, status, add, search, apply, doctor, lock, toolset, use, yes, run, trust, undo, adopt, secret. Everything else moves one hop behind `agentstack x <cmd>`, grouped by task.
      - **The rule, not taste.** A command stays visible when a first-run surface, a doctor fix line, or a machine-readable field can name it. That rule promoted `lock`, `secret` and `adopt` and demoted `up`, `share`, `receive`, `workflow` and `restore`. Fifteen is the honest count the rule produces; ten was the wish.
      - **Nothing is removed.** Every hidden command still runs at its own name with its own `--help`; `agentstack x <cmd> …` is the same parse tree, dispatch and exit code. `agentstack --help --all` still shows the whole map, and the panel's fixed action names are untouched.
      - **A surface never names a command a reader cannot find.** Guidance that points at a hidden command now names it through the namespace (`agentstack x guard install`), and plain `--help` still lists, by name, every hidden verb guidance can print.
11. [ ] Review closures 2026-08-05 — what the blueprint review left in the code, ordered by whether it can hurt someone. `plan/work-plan.html` is the corrected board; this queue is the authority where the two disagree.
      - **[x] STOP-SHIP — hooks rendered with no trust gate.** `apply --write` compiled `[hooks.*]` into `.claude/settings.json` and `~/.claude/settings.json` for untrusted and stale-trust projects, exit 0, no warning — the one kind that never gets a compressed consent path. Gated like extensions, witnessed for both states at both scopes with negative controls, and `ENFORCEMENT.md:222`/`:647` now describe what the code does.
      - **[ ] The small fixes.** doctor's Hooks check judges the global file for a repo project · N5 `doctor --json` answers by working directory · F5 the ARCHITECTURE.md crate-edge rule holds by discipline alone · stale `delivery --json` route text · the extensions gate refuses a machine manifest that no `trust` command can reach.
      - **[ ] N7 → P9.** Four stale lines in `docs/design/activation-study.md` are the last block on the study, and the study is the bar.
      - **[ ] The reads, not rewrites.** N2 (which `ENFORCEMENT.md` claims no longer match the code) and P8 (`run` and `workflow` were never exercised; six journey screens are sketches).
      - **Yours to decide, not to schedule:** the verb count (the binary ships seventeen, `TODO.md:21` and `main.rs:29` say fifteen) · R2 the positioning flip · R1 the private APM disclosure · the Mode-axis leftovers still in `doctor --json` and `ui_contract.rs` · P2 `automatic-delivery.md` · O2 · O3 · F6 · F8 · `x diff --profile`.
12. [ ] When the bar is met — re-pin the study kit, run it, fix its three blockers, publish, launch
      The docs site rides with this release, not before it: `.github/workflows/docs.yml` deploys on any push to `main` touching `docs/**`, and `main` documents behaviour no installable version has (decided 2026-08-02 — publishing earlier would describe a product nobody can run).
