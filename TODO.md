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
10. [ ] When the bar is met — re-pin the study kit, run it, fix its three blockers, publish, launch
      The docs site rides with this release, not before it: `.github/workflows/docs.yml` deploys on any push to `main` touching `docs/**`, and `main` documents behaviour no installable version has (decided 2026-08-02 — publishing earlier would describe a product nobody can run).
