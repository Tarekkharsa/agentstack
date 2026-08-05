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
      - **[x] The small fixes.** doctor's Hooks check judged the global file for a repo project · F5 the ARCHITECTURE.md crate-edge rule is now parsed and checked, not remembered · stale `delivery --json` route text · the extensions gate refused a machine manifest that no `trust` command can reach. **N5 did not reproduce** — twelve runs across three `--manifest-dir` spellings and four working directories gave byte-identical JSON, so the two observed runs saw different state or an older binary; `doctor_cwd_independence.rs` guards it anyway. The two real cwd seams it did surface are fixed: `trust` never read `--manifest-dir`, and the process cwd outranked `AGENTSTACK_MANIFEST_DIR`.
      - **[x] N7 → P9.** The study kit is self-consistent again, so **nothing now blocks P9 but running it**.
      - **[x] N2, read then applied.** The audit is `plan/enforcement-claim-audit-2026-08-05.md`; twenty claims are narrowed to what the code does. **[ ] P8** stays open — `run` and `workflow` were never exercised, and six journey screens are sketches.
      - **[ ] The gaps narrowing leaves standing.** Each is a real absence the document no longer hides. Ordered by what a hostile repository could use.
        - **G1** `use --write` materializes skill files with no trust check — add the `render::hooks::trust_refusal` shape to `commands/use_profile.rs`.
        - **G2** `apply --write` writes native server config for an untrusted project — give the server arm of `render/apply.rs` the refusal hooks and extensions already have.
        - **G3** an `owner`-tagged server's command line changes with no human re-review, because apply auto-repins trust. Show the refreshed definition in a card, or drop the auto-repin.
        - **G4** only `Gateway::from_frozen` carries the hard trust gate; extend `require_trust` to the host and lease constructors.
        - **G6** file-tool write confinement covers only the fixed `WRITERS` names, and Cursor file writes never reach the guard — build a payload-shaped classifier; wire Cursor when it ships a pre-write hook.
        - **G7** instruction compilation never calls `trust::check`.
        - **G8** `--allow-unresolved` writes a literal `${NAME}` into live config — omit the key instead, or remove the flag.
        - **G9** the write-time egress refusal in `render::apply` records nothing.
        - **G10** the execution relay binds `0.0.0.0` when the gateway is undetermined and on unrecognized platforms — fail closed, or require an opt-in.
        - **G11** the snapshot store and standing decisions are delivery inputs, not display state; a missing snapshot should re-gate, not drop.
        - **G13** the three fail-closed guard system refusals write no audit line.
        - **G14** the executor result-file bind has no kernel write cap; 1 MiB is a host read refusal.
        - **G15** `max_wall_seconds` is inert in the engine.
        - **G16** `set_decision` mutates the trust store and appends nothing.
        - **G17** `Family::Fence` has no `RunEvent`, so fence refusals never reach a run report.
        - **G18** settings are not pinned at all — `LockedSetting`, a re-gate on change, a doctor probe, a witness. On no queue item until now.
        - **G19** Kiro gets no host guard: its hooks nest inside per-agent config files.
        - **G21** extension, workflow and blueprint checksums never deposit into the content store, so a re-gate can show a pin with no approved copy to diff.
        - **G5** there is no headless path for unsigned content, since `receive --yes` refuses it — add a `--consented-digest` acceptance bound to previewed bytes, if that path is wanted at all.
      - **Yours to decide, not to schedule:** the verb count (the binary ships seventeen, `TODO.md:21` and `main.rs:29` say fifteen) · R2 the positioning flip · R1 the private APM disclosure · the Mode-axis leftovers still in `doctor --json` and `ui_contract.rs` · P2 `automatic-delivery.md` · O2 · O3 · F6 · F8 · `x diff --profile`.
12. [ ] When the bar is met — re-pin the study kit, run it, fix its three blockers, publish, launch
      The docs site rides with this release, not before it: `.github/workflows/docs.yml` deploys on any push to `main` touching `docs/**`, and `main` documents behaviour no installable version has (decided 2026-08-02 — publishing earlier would describe a product nobody can run).
