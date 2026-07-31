# Panel wireframe v2 — the popover's daily shape

**Status:** Active contract for the t3code popover. Implemented 2026-07-31
(t3code `panel-ux-redesign`; CLI contracts `set-mode-v1` +
`doctor-cli-coverage-v1` in this repo). The §1.6 activation study has not run:
everything here is maintainer design judgment plus review, not user evidence.

This is the redraw after the v1 review. Mode lives in the footer, mode changes
show their real plan, and the not-ready state is drawn. The rules that
constrain future panel work:

1. **The popover does the daily work. Manage is for the rare stuff.**
2. **The resting state is ONE card.** A previous pass cut that surface from
   nine regions to one; nothing may re-clutter it. Mode is a footer word, not
   a second card.
3. **A mode switch shows what it would add, remove, and break — then
   confirms.** Three clicks is the honest floor for the panel's most
   consequential write.
4. **Delivery mode is never forced.** Zero-files is not the default (see
   "Zero-files is not the default", below).

## Click budget

| Task | Before | v2 | Note |
|---|---|---|---|
| See toolset + mode + readiness | 1 | **1** | all three read from the resting state |
| Switch toolset | 4 | **2** | inline list, no dialog |
| Act on a problem | 2+ | **1** | one concern, one button |
| Change delivery mode | not possible | **3** | the third click is the confirm, and it stays |
| First run | 6+ | **3** | set up → review → apply |

## Daily path

### Resting state — 1 click

One card and a footer. The card is `WORKING UNDER <toolset>` with the one verb
(`Switch`). The footer reads:

```
● Ready · on disk · 13 CLIs                Manage ›
```

- The readiness word moved OUT of the header into the footer; the header is
  the mark and the name, nothing else. (The collapsed trigger chip keeps its
  own label — that is the affordance that must be noticed.)
- The mode is a clickable word (dashed underline), not a card. Mode changes
  almost never; a card advertises the control instead of serving the daily
  read.
- The CLI count comes from `doctor.clis` (`doctor-cli-coverage-v1`), and it is
  scoped to the mode: on disk shows `13 CLIs`; served live shows
  `11 of 13 CLIs` when two cannot host the bridge — a link into Manage, where
  the two that fall out are named. **A number that shrinks silently is worse
  than no number.**

### Not ready — 1 click

One concern, said as its consequence, one button, and what the button
promises. The footer swaps to `⚠ Needs review · <mode>` and drops the CLI
count — never two problems. Everything else is counted in Manage; a panel
that lists five worries has told you to go read them elsewhere. The mode word
stays reachable.

### Switch toolset — 2 clicks

`Switch` opens the list **in the popover**; picking a ready row applies it
(the same temporary-session activation the Manage rail uses — project-scope,
reversible, no re-gate) and closes. The Manage dialog never opens. Rows the
trust gate blocks say why and route to the review. `+ New toolset…` opens
Manage — creation needs the library beside it.

## Changing delivery mode — 3 clicks

The control v1 got wrong, twice: it committed on the radio click (the panel's
first confirm-less write, and its most consequential — machine scope, every
CLI config), and it had no un-render leg, so the switch would have left
rendered files in place and the derived mode would keep displaying a selection
the system refuses.

- **Click 2 — options, from the footer word.** Three options; the current one
  is tagged. Nothing commits from this list.
- **Click 3 — expanded, the real plan, then confirm.** Expanding an option
  fetches the CLI's `set-mode --preview`: every file the un-render removes,
  the managed `.gitignore` block, bridge registration with honest coverage
  ("11 of 13 CLIs can host it") and the NAMED CLIs that fall out, the
  compiled-instructions warning, the undo command, and the machine-scope
  warning ("registering the bridge changes every CLI's config on this
  machine; switching this project back later does not unregister it").
  Confirm applies under the previewed `consent_digest`.
- When the CLI would refuse, the confirm is replaced by the honest next step:
  an untrusted project gets **Review this project first** (trust is granted in
  the review, never here); an active session gets "stop using it first"; a
  switch the derivation cannot honor gets the CLI's own sentence.

**The chooser must NOT look or act like the toolset list.** A toolset switch
is project-scope and reversible; a mode switch is machine-scope and
asymmetric. Equal-looking controls teach equal safety — the toolset list
applies on click and carries pick-dots; the mode list is disclosure rows with
plans and an explicit confirm.

### The CLI contracts underneath

- `set-mode-v1` — `agentstack set-mode <static|clean-at-rest|zero-files>`,
  digest-bound preview/apply, with the un-render leg (shared with
  `uninstall`), the render leg (the one activation path), state-ledger
  clearing so the derived mode actually flips, and fail-closed edges
  (untrusted zero-files, active session, bridge-serves-this-project). The
  panel's picker exists only behind this name.
- `doctor-cli-coverage-v1` — `doctor.clis = {detected, bridge_capable,
  bridge_incapable[]}`, from the same eligibility `gateway connect` uses, so
  the footer count and the plan's coverage can never disagree.

## First run — 3 clicks

Set up → review (every write named, **including the `.gitignore` block**, with
the opt-out as a real button) → done lands on the resting state. No mode menu
on first run: the recommendation is applied and stays changeable from the
footer forever after. No completion screen. Trust review appears only in
modes that need it — it is not a tax the import journey pays by default.

## What changed from v1, and why

- **Mode moved to the footer.** v1 gave it a permanent card, re-cluttering
  the surface a prior pass cut from nine regions to one.
- **Mode switching gained a plan and a confirm.** v1 was a confirm-less
  machine-scope write.
- **The un-render leg exists.** v1's switch would have left the derived mode
  reading "on disk" over a selection the system refuses.
- **The not-ready state is drawn.** v1 only designed the happy path, which is
  the easy half of a status surface.

## Zero-files is not the default — deliberately

Two reasons. First, it contradicts both strategy documents verbatim: the
import journey "should not require Docker, policy authoring, a gateway, a
library, or an understanding of delivery modes", and the first promise is
*imported once and rendered everywhere* — a zero-files first run never
renders. Second, the complaint that motivated it is fixed: what made on-disk
mode feel invasive was AgentStack editing `.gitignore` without ever showing
you. It shows you now (`gitignore-opt-out-v1`), and you can decline. Changing
the universal default was a large lever for a problem that had a small one.
The default-mode question belongs to the activation study; nothing here blocks
it — the restructure works identically in all three modes.

## Two things this wireframe does not decide

- **The edit-flow collapse.** Reducing seven states to three is fine as
  presentation, but "confirm" must keep the previewed-digest handshake — the
  step where what you approved is bound to exact bytes. Drop that and the
  state count improves while a consent guarantee quietly disappears.
- **What Manage becomes.** It keeps Status / Toolsets / Activity, but nothing
  on the daily path routes through it anymore. Whether its own contents need
  cutting is a separate question from this one.
