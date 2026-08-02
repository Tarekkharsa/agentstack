# AgentStack panel — scenario tutorials

The harness that records nine short screen tutorials of the AgentStack panel
inside T3 Code (web), one per scenario. **The rendered videos are deliberately
not tracked** — they are ~20 MB per generation and git would keep every one
forever, so `rec/` is committed instead and reproduces them on demand.

Recorded output lands in `videos/` (gitignored). Captured 2026-08-02 at 1456x1008 against **agentstack v0.18.0-rc.2** and
the t3code fork at `a8e56403c`, driving the real app — not mockups.

Each video opens with a title card and narrates itself with on-screen captions.
A blue dot marks where the click lands.

| # | File | What it shows |
|---|---|---|
| 1 | `01-where-things-stand.mp4` | The chip in the thread header, and the popover: what changed and the one next step. |
| 2 | `02-the-yes.mp4` | The consent card — what a yes allows: the command it runs, what it reaches, which secrets it may read, and every skill's pin state. Nothing is approved on close. |
| 3 | `03-status.mp4` | Status: is it ready, the single next action, which CLIs are wired, what drifted, how the setup is delivered. |
| 4 | `04-drift.mp4` | Review drift: your on-disk edits vs the project's version — Keep edits or Re-render, with machine-wide files called out separately. |
| 5 | `05-toolsets.mp4` | Toolsets: named bundles per task, the rail, and switching. Creating never activates. |
| 6 | `06-library.mp4` | The library: filter, add a capability to a toolset, and the confirm that explains the re-lock before it writes. |
| 7 | `07-activity.mp4` | Activity: brokered calls, guard denials, skill loads — arguments recorded as digests, never values. |
| 8 | `08-protection.mp4` | More protection: guard, machine policy, live serving, locked run, sandbox — and sharing as signing. |
| 9 | `09-undo.mp4` | Undo: the recorded-changes ledger, what each entry touched, and which of them this panel can revert. |

## Honest notes

- The project used for recording sits in a **re-gate state** ("reviewed content
  changed on disk"), which is why the consent card has real content to show.
  Nothing was approved during recording — the card is opened and closed.
- **09-undo** opens the ledger in full. The entries are real: the top one is the
  `set-mode zero-files` switch that re-rendered 13 files across every CLI. Note
  the honest labels — most entries read *"elsewhere on this machine"* (recorded,
  but not revertible from this panel) and one reads *"already undone"*, because a
  revert is itself a recorded change. The panel says so in its own footer:
  only this project's changes can be reverted from here.
- Delivery mode shown is **files on disk (static)** — the shipped default.
  Zero-files/dynamic is available, not the default.

## Re-recording

The harness is committed under `rec/`. It pairs once against a running t3code
dev server, saves the session, then records one Playwright context per
scenario — captions and the cursor dot are injected into the page, so narration
is burned into the footage.

```bash
# 1. one dev server only — orphaned ones serve mixed builds and corrupt captures
cd ~/Documents/GitHub/t3code && pnpm dev          # note the printed pairing URL

# 2. pair once (tokens are single-use)
cd .tutorials/rec
TUT_OUT=.. TUT_PAIR="http://localhost:PORT/pair#token=XXXX" node auth.cjs

# 3. record — all, or selected
TUT_OUT=.. TUT_APP="http://localhost:PORT" node scenarios.cjs
TUT_OUT=.. TUT_APP="http://localhost:PORT" node scenarios.cjs 02 06

# 4. convert (raw/ holds .webm; crop removes Playwright's padding)
ffmpeg -i raw/NAME.webm -vf "crop=1040:720:0:0,scale=1456:1008:flags=lanczos,fps=24" \
  -c:v libx264 -preset slow -crf 20 -pix_fmt yuv420p -movflags +faststart videos/NAME.mp4
```

`raw/` and `auth.json` are gitignored — the first is intermediate, the second is
a live pairing credential.

Two traps worth remembering: Playwright **pads** rather than upscales when
`recordVideo.size` exceeds the viewport (hence the crop), and panel content
routinely renders below the dialog fold, so scenarios scroll with `reveal()`
between captions rather than narrating over a half-visible view.
