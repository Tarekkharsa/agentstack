# Activation study kit (§1.6 / Stage 1 gate)

> **Status:** ready to run · **Owner:** maintainer · **Written:** 2026-07-29
>
> Everything needed to run the five-participant activation study is in this
> one file: who to recruit, what to say, how to observe, what to record, and
> how the results map onto the Stage 1 gate in `TODO.md`. The study is the
> one planned activity that can falsify the product thesis rather than
> confirm it — run it before adding features.
>
> **P0.1 (2026-07-31):** strategy-v2 instrumentation added — the observation
> prompts at the end of §5 and the metric baselines in §9. Both are additive
> observation; the protocol steps, task script, metrics sheet, results
> template, and the Stage 1 pass condition are unchanged.
>
> **Dry-run readiness pass (2026-07-31):** Appendix A (printable observer
> sheet) and Appendix B (screening checklist + session-day runbook) added, and
> the session length in §1/§2 corrected to 30–40 minutes to match what the
> journey actually takes. All additive: the task script, protocol order,
> metrics, thresholds, and the §7 pass condition are untouched. Findings from
> the isolated pilot run behind this pass are in §8.1.
>
> **Blocker #1 fixed in v0.17.1 (2026-07-31):** the §8.1 pilot's Run B
> dead-end — a project-scope-only setup reported as "no CLIs detected", an
> empty manifest written over it, and a clean `doctor` over that — is fixed and
> published. **Participants install the latest release, so the study now tests
> the fixed journey.** §8.1 keeps Run B's original transcript as the
> before-state, with the after-state recorded beside it. Observer briefing
> changes accordingly: do not expect the Run B stall (Appendix B2's rule is
> unchanged — still never screen on where a candidate's servers live).
> Everything else — task script, protocol, metrics,
> thresholds, §7 pass condition — is unchanged.
>
> **Demand dimension added (2026-08-05):** every gate metric in this kit
> measures usability; none measured whether anyone wants the thing. Three
> additive pieces close that: the §5 exit question (D1), the re-trust friction
> watch (D2), and the §9.1 two-week follow-up. All three are recorded, none is
> scored. The five thresholds, the §6 metrics sheet, the §7 results template,
> and the §7 pass condition are byte-identical — a demand answer informs the
> bar, it does not move the pass/fail line.

## 1. Participant criteria

- Uses **two or more supported agent CLIs** today (any two of: Claude Code,
  Codex CLI, Gemini CLI, Copilot CLI, OpenCode, Cursor, …).
- Has at least one MCP server or skill configured in at least one of them.
- Did **not** build AgentStack and has not used it before.
- Comfortable sharing their screen for 30–40 minutes.

Five participants. Recruit from real installs where possible (v0.16.0+ is
public); colleagues are fine if they meet the criteria and haven't watched
you demo it.

## 2. Recruiting message (copy-paste)

Send the DM variant to people you know; the broadcast variant fits a Slack
channel or public post. Neither names a command, links the repo, or links
docs — participants must arrive cold; the task script hands them the URL.

**DM:**

> Hey — I built a CLI tool that manages MCP servers and skills across coding
> agents (Claude Code, Codex, Gemini, Cursor…), and I'm doing a round of
> usability sessions before launch. You use at least two of those, right?
>
> The ask: 30–40 minutes on a call, screen shared. You install the tool and set
> it up on your own machine with your real configs — I watch and take notes,
> but I don't help; watching where it's confusing *is* the study. Nothing
> leaves your machine, nothing to prepare, and you can rip it out at the end
> (uninstalling cleanly is part of what I'm testing).
>
> Any slot this week or next work for you? I'll owe you a coffee and
> first-name credit in the release notes — or anonymity, your pick.

**Broadcast:**

> Looking for 5 developers who use **2+ AI coding CLIs** (Claude Code, Codex,
> Gemini, Cursor, OpenCode…) for a 30–40 minute usability session of a tool that
> unifies MCP/skill setup across them. You install it on your machine while
> sharing your screen; I watch and take notes but don't coach — the stumbles
> are the data. No prep, nothing leaves your machine, easy to uninstall
> after. DM me if you're in.

## 3. Setup (before each session)

- Participant's own machine, their real CLI configs. No sandbox, no demo repo.
- They pick a real project of theirs (or an empty directory — their choice).
- **The install command participants run** — send them this exact line, not
  the README's. The study tests the completed v2 journey, which the latest
  published release does not carry; this pins the release candidate that does.
  It is checksum-verified by the same installer path as any other install:

  ```sh
  curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh \
    | AGENTSTACK_VERSION=v0.18.0-rc.2 sh
  ```

  > **Recorded change (2026-08-02):** the instrument was re-cut and published
  > as v0.18.0-rc.2 before any participant ran; the pinned install line and the
  > version floor were updated to match. Protocol and pass condition are
  > byte-identical.

- Confirm the binary they end up with carries the current journey:
  `agentstack --version` ≥ 0.18.0-rc.2. A bare `| sh` installs the latest
  *stable* release instead, which is a different and older journey — if their
  version reads 0.17.x, they used the wrong line and the session should
  restart from the install.
- Start a timer at the moment they run the install command; note wall-clock
  timestamps at each milestone below.

Consent and data handling (say this before starting, get a verbal yes):

- What is collected: hand-written notes, milestone timestamps, and verbatim
  confusion quotes. No screen or audio recording unless they explicitly agree;
  if they do, the recording stays on the observer's machine.
- Notes carry a participant number, not a name; nothing else identifying.
- Notes are kept only until the top-three blockers are fixed and the Stage 1
  gate is recorded in `TODO.md`, then deleted. Aggregated results (times,
  counts, anonymized quotes) are what persists.
- They can stop at any point, and can ask for their notes to be deleted
  afterward — both without explanation.

## 4. Task script (read aloud, then stop talking)

> "Install AgentStack from <https://github.com/Tarekkharsa/agentstack>, then
> use it to bring the servers you already have in your CLIs under one setup.
> When you believe everything is working, verify that it is. Then create a
> smaller named toolset for one kind of work, switch to it, and finally undo
> your last change. Think aloud as you go."

That single paragraph contains the whole journey (install → init → apply →
doctor → toolset switch → restore) without naming a single command — whether
they *find* the commands is the study.

## 5. Observation protocol

- **This study observes the CLI only.** The t3code panel is not shown, not
  installed, and not mentioned. Reason, recorded once so it is not
  relitigated: the panel renders `status-v1` and does not yet render the
  contracts this release added, so its "Ready" chip would contradict the CLI on
  exactly the case `readiness` was introduced to fix — a participant would be
  studying a disagreement between two surfaces rather than the journey. The
  fork's migration is post-study work. If a participant raises the panel
  themselves, use the scripted line in Appendix B3 and return to the task.
- **No command coaching.** Never name a command, flag, or file. If asked,
  answer "what would you try?" and record the question verbatim.
- **Intervention = failure.** If they are hard-stuck for 5+ minutes and you
  step in, that participant counts as "did not finish unaided" — keep
  observing the rest of the journey anyway; the blocker list is the point.
- Record think-aloud confusion verbatim, especially any term they misread
  (toolset? manifest? drift? trust?) and every error message they hit.
- Note every `--help`, doc page, or web search they reach for, and whether
  it answered them.
- After the tasks, ask exactly two questions:
  1. "In one sentence, what is this tool?" (gate: "one setup across my
     coding CLIs" or equivalent)
  2. "Was there any point where something was blocked or refused and you
     didn't understand why?" (gate: they understood every block and knew a
     safe next action)

### v2 observation prompts (Phase 0 — observe, never coach)

Four things to watch for while running the protocol above. They add observation
only: no new tasks, no changes to the task script, no coaching, and no new
gate. The two gate questions stay exactly two — the one question below is asked
after them and feeds no gate metric. Nothing recorded here changes the §7 pass
condition.

These were written when the moments they describe were predictions. They now
exist in the binary the participant is running, so each has been rewritten to
ask whether the thing **works and is findable**, rather than whether the user
gropes for something absent. Only prompt 2 has a question you speak aloud; the
rest are silent observation.

1. **The drop-a-file path.** This now exists: a file placed in
   `.agentstack/skills/` or `.agentstack/instructions/` is noticed, and
   `agentstack yes` takes it live. So the question is no longer whether they
   reach for it and stall — it is whether they find it **unaided**. Record: did
   they place a file at all (timestamp, exact path, what they said, verbatim);
   if they did, what they tried next, in their own words, before anything
   worked; and whether they arrived at `yes` themselves, arrived at the longer
   explicit path, or arrived nowhere. Do not tell them the path exists. If they
   never place a file, write "not observed" — that is a finding about
   discoverability, not a failed session.
2. **What their yes granted.** After the two gate questions, ask once: "Earlier
   you approved this project — in your own words, what did that approve?" Write
   the answer down verbatim, word for word, then stop: no correcting, no
   confirming, no filling in gaps, no second attempt. There is now a review card
   that states this in two to five plain lines, so the answer is scorable
   against something specific (§9, review comprehension) rather than against a
   wall of detail. If no review appeared in their session, write "no review
   shown".
3. **The re-gate, if they meet one.** If approved content changes during the
   session, they are shown a diff and three answers: accept, keep the approved
   version, block. Record whether they understood the choice **without asking
   you** — and if they asked, the exact words of the question. Do not explain
   the three answers. If no re-gate occurs, write "not observed"; do not
   engineer one.
4. **Getting back, if they want to.** `agentstack undo` now exists. If they
   express any wish to reverse something — including a muttered one — record
   what they reached for first (undo, restore, `git checkout`, ctrl-Z, asking
   you), whether it worked, and how long it took. Do not suggest undo. If
   nothing goes wrong and they never want to reverse anything, write "not
   observed" and leave §9's recovery-time row blank.

### Demand observation (Phase 0 — one question, one watch; feeds no gate)

Everything above measures whether a stranger can work the tool. Neither of the
two items below does. They measure whether a stranger would keep it — the one
thing this kit could not previously see, and the one thing the product thesis
also rests on. They add no task, no coaching, and no threshold. The two gate
questions stay exactly two; D1 is asked after them and after V2, last of all.
**Nothing recorded here changes the §7 pass condition.** A demand answer informs
the bar; it does not move the pass/fail line.

- **D1 — would they keep it.** After Q1, Q2, and V2, ask once: "Would you keep
  this installed next week — why or why not?" Write the answer down verbatim,
  word for word, including the why. Then stop: no persuading, no correcting, no
  second attempt, no offer to fix the reason they name. A "no" with a clear
  reason is the most useful sentence in the session. If they answer only "yes"
  or only "no", ask "why?" once and record that too — the why is the finding,
  the yes/no is not scored.
- **D2 — the re-trust friction watch (silent).** Watch for every moment the
  participant is told approval is stale or is asked to approve again — after a
  re-lock, after a toolset change, after pinned content moves. Record the time,
  what was on screen, what they had just done, and their reaction verbatim,
  including a muttered or non-verbal one ("again?", a sigh, a scroll back). Do
  not explain the prompt and do not tell them it was expected. Count the
  moments: the hypothesis under watch is that re-trust friction is cheap once
  and expensive in aggregate, so the number of times matters as much as any one
  reaction. If it never happens, write "not observed".

## 6. Metrics sheet (one per participant)

```text
Participant #___   date ______   CLIs: __________________  OS: ______

  install success                     yes / no    t = ____
  time to understand the product      t = ____   (their one-sentence summary, whenever it emerges)
  time to first manifest (init done)  t = ____
  time to first successful apply      t = ____
  time to clean doctor                t = ____   ← gate metric (install → here)
  toolset created + switched          yes / no    t = ____
  undo (restore) succeeded            yes / no    t = ____
  finished without intervention       yes / no
  describes it as "one setup across
  my coding CLIs" (own words ok)      yes / no
  needed Docker / policy / gateway /
  workflow concepts for first value   yes / no   (any = gate fail)
  understood every block hit          yes / no

  Confusing terms (verbatim): _______________________________________
  Abandoned/retried steps:    _______________________________________
  Errors hit + what they did: _______________________________________
```

## 7. Results template → Stage 1 gate

Fill after all five sessions; each line maps 1:1 onto a Stage 1 gate
checkbox in `TODO.md`.

```text
  finished unaided:           _ / 5   (gate: ≥ 4)
  median install→clean doctor: ____   (gate: < 5 min)
  "one setup across CLIs":    _ / 5   (gate: ≥ 4)
  zero advanced concepts:     _ / 5   (gate: 5/5 — no participant needed them)
  understood every block:     _ / 5   (gate: ≥ 4)

  Three most common blockers (by participant count, not severity):
    1. ____________________________________  seen by _ / 5
    2. ____________________________________  seen by _ / 5
    3. ____________________________________  seen by _ / 5
```

**Pass:** tick the Stage 1 gate boxes in `TODO.md`, fix the three blockers
above before any new feature work (§1.6's own closing rule), then launch is
validated. **Fail:** the blocker list *is* the roadmap; fix, re-recruit two,
re-verify the failed metric.

## 8. Maintainer dry-run baseline (2026-07-29)

For calibration only — not one of the five. Sandboxed HOME, release build,
two CLIs (Claude Code + Codex), three servers, one plaintext token. Command
latency across the whole journey: init 1.2s · apply 0.1s · doctor 0.5s ·
toolset create 0.1s · use 0.1s · doctor 0.1s · restore 0.1s — **2.1s total
tool time**, so the 5-minute budget is entirely reading and typing time.
Blockers found and fixed ahead of the study: doctor contradicting a fresh
toolset switch (active-toolset drift awareness), `toolset create` rejecting
the positional name, jargon in the no-such-toolset error and lock summary,
and a stale hand-edit-the-TOML undo hint. Expected remaining friction to
watch for: the trust re-review warning after `toolset create` re-locks (the
intended gate — the cue is now one command, see §8.1), and the Codex
project-trust note. The project-scope shape this section flagged to watch was
found deterministically by the §8.1 pilot and fixed in v0.17.1.

### 8.1 Isolated pilot run (2026-07-31) — kit rehearsal, not a participant

> **HISTORICAL — captured on v0.17.0, replayed on v0.17.1. Participants run
> v0.18.0-rc.1.** Everything in this section is evidence that specific
> blockers were found and fixed. **It is not a description of what a
> participant will see.** Read it to know what was already caught, so a
> familiar-looking stall can be recognized as either "the old one, back" (a
> regression worth reporting loudly) or something new. Do not read it as a
> script for the RC's behaviour.
>
> Where the RC differs by design: the whole v2 journey below the import —
> drop-a-file, one yes with a review card, the re-gate diff, `undo` — did not
> exist in either binary captured here, so no transcript in this section shows
> it and nothing here predicts how it will go.

A second dry run, in a throwaway HOME with the **public v0.17.0 installed by
the published one-line installer**, rehearsing the §4 journey twice to check the
kit is runnable and to see what a stall looks like. Not one of the five, and no
product code was changed in response.

*Run A (captured on v0.17.0) — servers in global CLI config (Claude Code + Codex, 3 servers, 1
plaintext token).* The whole journey completed: import wizard → apply → verify →
toolset create → switch → undo. `doctor` ended 0 errors. The wizard previewed
every file before writing, lifted the token to `${GITHUB_TOKEN}`, and each step
named its own undo. Two things to watch in real sessions:

- After a completed setup, `status` still offers `agentstack doctor` as the next
  step even once doctor has been run clean — it does not advance the participant
  toward the toolset task. A participant following only the on-screen next step
  has nothing telling them the journey continues.
  **Fixed in v0.17.1:** once the wiring is rendered and nothing is grouped yet,
  `status` advances to `agentstack toolset create <name> --server <server>`.
- After `toolset create` re-locks, `status` reports `trust stale (content
  changed)` but its **Next** line points at `doctor`, not at re-review; the
  actual instruction (`review + agentstack trust`) appears one hop later, in
  doctor's output. The re-gate itself is intended — the cue costs two commands,
  not one. This is the friction §8 predicted, one hop longer than predicted.
  **Fixed in v0.17.1:** trust-stale now names `agentstack trust .` on the Next
  line directly, so the cue is one hop. The re-gate itself is unchanged.

*Run B (captured on v0.17.0) — the same servers, but configured only in project-scope files*
(`.mcp.json` and `.codex/config.toml` in the working directory, nothing in the
user's home). This is the shape §8 flagged to watch, and it fails harder than
"import misses some servers":

- `status` reports `CLIs  none detected on this machine` while four servers sit
  in the current directory.
- `init` says `No supported CLIs detected to import`, writes an **empty**
  starter manifest, and directs the user to search a catalog and add servers by
  hand. Nothing on screen says their existing config files were seen and
  skipped.
- `doctor` then reports **`0 error(s), 0 warning(s)`** over that empty manifest.
  A participant who "verifies everything is working" is told it is.
- The toolset task dead-ends: `error: no server 'filesystem' in the manifest or
  central library`.
- `status` contradicts itself on one screen: `CLIs  none detected on this
  machine` above `0 server(s) → 13 detected CLI(s)`.
- `agentstack adopt` *does* find all four servers and lifts the token correctly
  — so the capability exists and only discovery is missing — but `adopt` is
  never named by `status`, `init`, or `doctor`, so an uncoached participant has
  no path to it.

For a participant in this shape the §4 task ("bring the servers you already
have under one setup") could not be completed unaided, and the failure was
silent rather than loud.

**Fixed in v0.17.1 (2026-07-31) — the transcript above is the before-state.**
The root cause was one question asked wrong: every surface except `adopt` was
asking whether a CLI is installed *on this machine* and answering a question
about *this directory* with it. Replaying Run B against the fixed binary in the
same isolated HOME:

- `status` reports `2 of 13 supported detected here: Claude Code · Codex CLI`,
  and a `Found` line naming the three servers and the two files they are in.
- `init` imports all three, lifting the plaintext token to `${GITHUB_TOKEN}`.
- `doctor` warns whenever servers configured here are not in the manifest,
  naming the file, the servers, and `agentstack adopt` as the one next action —
  so `0 errors` over an ignored setup is no longer reachable.
- The one-screen contradiction is gone: detected-here and the adapter catalog
  are stated as two different facts.
- The toolset task proceeds; the dead-end does not occur.

**Observers: do not expect this stall.** Participants install v0.18.0-rc.2,
which carries this fix and everything after it. If a project-scope participant stalls anyway, that is a *new* finding
and belongs on the stall log as one — do not attribute it to this entry. Do not
screen participants out for having a project-scope setup — see Appendix B2.

## 9. North-star metric baselines (strategy v2, Phase 0)

`STRATEGY.md` names four north-star metrics; Phase 0 is where they get their
first numbers. Nothing here adds a tester-visible step. Every value is read off
the §4 task script and the §5 protocol exactly as they already run. A metric a
session does not naturally produce is recorded as **not observed** — never
staged, prompted for, or re-run to manufacture a number. These values are a
baseline, not a gate: the §7 pass condition ignores them.

Keep one addendum sheet per participant alongside the §6 metrics sheet:

```text
Participant #___   (v2 baselines — addendum; does not feed the §7 gate)

  TTLC                    t = ____  / not observed
    content in place at ____ · live in CLI 1 at ____ · live in CLI 2 at ____
  concepts-before-value   count = ____   nouns: ____________________________
  review comprehension    correct / partial / wrong / no review shown
    restatement (verbatim): ___________________________________________
  recovery time           t = ____  / no recovery occurred
    trigger: ____________________________   working again at ____
```

**TTLC — time to live capability.** Wall-clock from the moment the tester
finishes putting a capability's content in place to the moment that capability
is live in two CLIs. Measure it only when a session naturally includes authoring
— they write or paste a skill, instruction, or server of their own. Start the
clock when they save the file (or add the entry); stop when they have seen it
working in the second CLI. Import-and-switch sessions produce no TTLC: record
**not observed**. Never ask a tester to author something to produce this number.

**Concepts-before-value.** Count the distinct mechanism nouns the tester read on
screen, or was shown, before their first successful outcome — first clean
status, the §6 "time to clean doctor" milestone. Count from this closed list:
manifest, lock, trust, digest, gateway, policy. Count each noun once, the first
time it appears in output they actually read or on a doc page they opened; do
not count nouns they typed themselves or found by reading the source. Record
both the count and which nouns.

**Review comprehension.** Take the verbatim restatement from §5's second v2
prompt and score it after the session against what the review actually showed:

- **correct** — they name what was approved and its scope (this project, this
  pinned content), and grant nothing the review did not.
- **partial** — directionally right but missing the scope, or vague ("it let
  the tools run").
- **wrong** — they name something the review did not grant, or cannot say.

Score once, from the written record, never in the room.

**Recovery time.** Wall-clock from the tester's first signal that something is
wrong — an error they react to, a "wait, that's not right", an undo attempt —
to a state they themselves call working again. Take whatever recovery the
session produces naturally, including the §4 undo step when it follows a real
mistake. If nothing went wrong, record **no recovery occurred**; never break
something to create one.

**Privacy.** All four are opt-in observation inside the consented session of §3
— hand-written notes and timestamps, nothing else. No telemetry, no
instrumentation of the participant's machine, no collection outside the session,
and the same deletion terms §3 already promises. The F19 constraint applies
unchanged: these baselines exist only through opt-in studies until F19's
privacy-preserving measurement design is approved.

### 9.1 Two-week follow-up (demand and retention — feeds no gate)

D1 records what a participant says they will do at the end of a session they
just spent 40 minutes in. This section records what they actually did. It is the
only part of the kit that happens outside the room, and like §9 it is a
baseline, not a gate: **the §7 pass condition ignores it, and no threshold on
this page moves because of it.**

Contact each of the five participants **14 days after their own session** —
count from their session date, not from the last session. One message, in
whatever channel you recruited them in, and no reminder if they do not answer:
a chased answer is not a retention signal. Ask for three things and nothing
else:

1. **Is it still installed?** yes / no / unsure.
2. **Have you run it since the session?** yes / no — and if yes, roughly how
   many times, their estimate, unrounded by you.
3. **One sentence on why or why not.** Verbatim, word for word. Take the
   sentence they give; do not ask a follow-up, do not offer to fix what they
   name, and do not tell them what changed in the tool since.

Record per participant, on the same numbered sheet:

```text
Participant #___   session date ______   contacted on ______  (session + 14d)

  still installed          yes / no / unsure / no reply
  ran it since the session yes / no / no reply     roughly ____ times
  why / why not (verbatim, one sentence):
    _______________________________________________________________
    _______________________________________________________________
```

No reply is a recorded outcome, not a missing value: write **no reply** and move
on. Aggregate the five as counts beside the §7 results (_ / 5 still installed,
_ / 5 ran it since) and keep the five sentences together — the sentences carry
the finding, the counts only carry its size. Same consent and deletion terms as
§3: the follow-up is part of the same session's notes, is kept under the same
participant number, and is deleted with them.

## 10. Appendix A — printable observer sheet

One per participant. Print it, fill it in the room, in pen. It is designed to
be usable by an observer who has **not** read `STRATEGY.md`: every box says what
to write and nothing asks for a judgement call during the session. Scoring
happens afterwards, from the sheet.

Two rules that override anything else on this page: **never name a command,
flag, or file**, and **write what was on screen, not what you think they meant**.

```text
ACTIVATION STUDY — OBSERVER SHEET

Participant #___   date __________   observer __________   OS __________
CLIs they actually use: ______________________________________________
Where their servers live today (tick what you SEE, do not ask them to change it):
   [ ] global/user config   [ ] project config in the repo   [ ] both   [ ] unsure

--- TIMELINE (wall clock; start the timer when they run the install command) ---
   install command run                     __:__     <- t0, start of gate metric
   install success                yes/no   __:__
   first manifest exists (init done)       __:__
   first successful apply                  __:__
   clean status/verify reached             __:__     <- GATE METRIC ends here
   toolset created                yes/no   __:__
   toolset switched               yes/no   __:__
   undo / restore succeeded       yes/no   __:__
   session end                             __:__

   install -> clean verify = ______ min  (this participant's gate number)
   finished WITHOUT intervention?   yes / no
     (any time you stepped in after 5+ min hard-stuck = no)

--- §9 BASELINE METRICS (record, do not score in the room) ---
 1. TTLC — time to live capability
      There is now a real funnel to time: file dropped -> live in two CLIs.
      Clock starts when the FILE IS SAVED, not when they start thinking.
      [ ] not observed (they authored nothing of their own)
      file saved / entry added at     __:__   path: ________________
      route they took:  [ ] agentstack yes   [ ] explicit (adopt/lock/trust/use)
                        [ ] never got there
      seen working in CLI #1 at       __:__   (which CLI: __________)
      seen working in CLI #2 at       __:__   (which CLI: __________)
      TTLC = ______        unaided?  [ ] yes  [ ] asked  [ ] told

 2. Concepts-before-value — mechanism nouns they READ on screen or on a doc
      page BEFORE the clean-verify milestone above. Tick only what they were
      actually shown; do not count words they typed or found in source.
      [ ] manifest  [ ] lock  [ ] trust  [ ] digest  [ ] gateway  [ ] policy
      count = ____

 3. Review comprehension — see prompt V2 below; score later, never in the room
      [ ] correct  [ ] partial  [ ] wrong  [ ] no review shown

 4. Recovery time — only if something actually went wrong on its own
      `agentstack undo` now exists, so this measures whether they FIND it,
      not whether a way back exists. Never suggest it.
      [ ] no recovery occurred
      first signal something was wrong  __:__
      what triggered it: ______________________________________________
      what they reached for FIRST: [ ] undo  [ ] restore  [ ] git
                                   [ ] ctrl-Z  [ ] asked you  [ ] other ____
      they call it working again at     __:__
      recovery = ______

 5. The re-gate, if one occurred (prompt V3) — not scored, recorded
      [ ] not observed
      understood accept / keep / block unaided?   [ ] yes  [ ] asked
      if they asked, their exact words: _______________________________

--- THE TWO GATE QUESTIONS (ask verbatim, after the tasks) ---
 Q1 "In one sentence, what is this tool?"
    verbatim: ______________________________________________________
    ______________________________________________________________
    counts as "one setup across my coding CLIs" (own words ok)?  yes / no

 Q2 "Was there any point where something was blocked or refused and you
     didn't understand why?"
    verbatim: ______________________________________________________
    ______________________________________________________________
    understood every block they hit?                            yes / no

--- THE TWO v2 OBSERVATION PROMPTS (observe only; feed no gate) ---
 V1  THE DROP-A-FILE REACH — do not prompt for this, only watch.
     The moment they act as if putting a file somewhere makes it live, then
     stall because it is not.
     [ ] not observed
     time __:__   what they had just done: ___________________________
     exact path of the file they wrote/edited: _______________________
     what they said, verbatim: _______________________________________
     _________________________________________________________________
     (do NOT tell them whether it works)

 V2  WHAT THEIR YES GRANTED — ask ONCE, after Q1 and Q2:
     "Earlier you approved this project — in your own words, what did that
      approve?"
     verbatim, word for word: ________________________________________
     _________________________________________________________________
     _________________________________________________________________
     Then STOP. No correcting, no confirming, no second attempt.
     [ ] no review appeared in their session

--- DEMAND (record only; feeds NO gate — see §5, §9.1) ---
 D1  WOULD THEY KEEP IT — ask ONCE, last of all, after Q1, Q2 and V2:
     "Would you keep this installed next week — why or why not?"
     verbatim, word for word (the WHY is the finding): ________________
     _________________________________________________________________
     _________________________________________________________________
     If they said only yes/no, ask "why?" once and write that here too.
     Then STOP. Do not persuade, do not offer to fix the reason they name.

 D2  RE-TRUST FRICTION — silent watch, every time it happens, no coaching.
     Any moment they are told approval is stale or asked to approve again.
     [ ] not observed          how many times in total: ____
     #  time    what was on SCREEN (quote it)     what they had just done     their reaction, verbatim
     1  __:__   ______________________________    ________________________    ____________________
     2  __:__   ______________________________    ________________________    ____________________
     3  __:__   ______________________________    ________________________    ____________________
     (count sighs, mutters and scroll-backs as reactions; quote them)

--- STALL LOG (one block per stall; keep going, do not coach) ---
 #  time    what was on SCREEN (quote it)      what they EXPECTED (their words)      what ACTUALLY happened
 1  __:__   _______________________________    ___________________________________   ______________________
 2  __:__   _______________________________    ___________________________________   ______________________
 3  __:__   _______________________________    ___________________________________   ______________________
 4  __:__   _______________________________    ___________________________________   ______________________

--- FREE CAPTURE ---
 Terms they misread (toolset? manifest? drift? trust?), verbatim:
   ______________________________________________________________
 --help / doc page / web search they reached for, and did it answer them:
   ______________________________________________________________
 Errors hit + what they did next:
   ______________________________________________________________
 Did they need Docker, policy, gateway, or workflow concepts to get first
 value?   yes / no        (any yes = gate fail for the whole study)

--- 14-DAY FOLLOW-UP (§9.1; fill from OUTSIDE the room; feeds NO gate) ---
 Contact date = session date + 14 days:  __________   contacted on __________
 One message, no reminder if they do not answer.
   still installed             [ ] yes  [ ] no  [ ] unsure  [ ] no reply
   ran it since the session    [ ] yes  [ ] no  [ ] no reply   roughly ____ times
   why / why not (verbatim, one sentence, do not follow up):
     ____________________________________________________________
     ____________________________________________________________
```

### Scoring rule (restated exactly as §7 states it)

Fill this once, after all five sessions. These are §7's words unchanged; do not
adjust a threshold to fit a result.

```text
  finished unaided:           _ / 5   (gate: ≥ 4)
  median install→clean doctor: ____   (gate: < 5 min)
  "one setup across CLIs":    _ / 5   (gate: ≥ 4)
  zero advanced concepts:     _ / 5   (gate: 5/5 — no participant needed them)
  understood every block:     _ / 5   (gate: ≥ 4)
```

In plain terms, the two that decide the gate: **at least 4 of 5 participants
finish unaided**, and the **median install→clean-verify time is under 5
minutes**. The §9 baseline metrics are recorded but do not count toward it.

## 11. Appendix B — recruiting kit

Three artifacts for the maintainer: what to send, who qualifies, and what to do
on the day. The canonical outreach copy stays in §2 — this appendix does not
restate it. Nothing here may pre-teach the product: no command names, no
concept vocabulary (manifest, toolset, trust, drift), no explanation of what
the tool will do beyond the one honest sentence §2 already uses. If a
participant arrives already knowing the words, the study cannot measure whether
the product teaches them.

### B1. Before you send

Use the §2 DM for people you know and the §2 broadcast for a channel. Both are
already written to be honest about the format: 30–40 minutes, screen shared,
you watch and take notes and do not help. Two things to check before hitting
send:

- You have not demoed the tool to this person, and they have not seen it over
  your shoulder.
- You are sending the message as written. Do not add a link to the docs, a
  quickstart, or a command "to save time" — the task script in §4 hands them
  the URL and nothing else, and anything extra invalidates the participant.

### B2. Screening checklist

Confirm all five before booking. Ask these in chat, before the session, so no
session time is spent discovering a participant does not qualify.

```text
Candidate: ______________________   screened on ______

 [ ] 1. Uses TWO OR MORE supported agent CLIs today.
        Which two: ______________________________________
        (Claude Code, Codex CLI, Gemini CLI, Copilot CLI, OpenCode, Cursor,
         VS Code, Windsurf, Kiro, Junie, Antigravity, Pi, Claude Desktop)

 [ ] 2. Has at least one MCP server or skill configured in at least one of
        them, and it is one they actually use.
        Roughly what: ___________________________________

 [ ] 3. Did NOT build AgentStack, has never used it, has not watched a demo.

 [ ] 4. Willing to share their screen for 30–40 minutes, on their own machine,
        with their real configs.

 [ ] 5. Willing to be watched without being helped, and understands the
        stumbles are the point.

  → all five ticked = book them.   any unticked = thank them, do not book.
```

Do **not** screen on where their servers are configured — global config or
project config. Both shapes must reach the study; how the product handles each
is exactly what is being measured. Record the answer on the observer sheet,
never select for it.

### B3. Session-day runbook (observer)

**Before the call (10 minutes, alone).**

- Print one Appendix A sheet. Have a pen and a clock with a seconds hand or a
  phone timer.
- Confirm the version they will get is current: the study needs
  `agentstack --version` ≥ 0.18.0-rc.2. Run §3's pinned install line on your
  own machine, not theirs, and check the version it produces. Do not check
  what the bare public installer serves — it serves the latest stable release
  on purpose, and that is not the build this study is about.
- Have §3's consent points and §4's task script in front of you as text you can
  read off. You will read both aloud.
- Decide nothing else. You are not preparing a demo.

**On the call, in order.**

1. **Consent (say it, get a verbal yes).** Read §3's four consent points:
   what is collected (notes, timestamps, verbatim quotes — no recording unless
   they agree, and it stays on your machine); notes carry a number, not a name;
   notes are deleted once the top-three blockers are fixed and the gate is
   recorded; they can stop at any time and can have their notes deleted, both
   without explanation. Wait for an actual yes.

2. **Set the scene, briefly.** Say verbatim:

   > "I'm going to read you a task, then go quiet. I'm not going to help, and
   > that's not me being difficult — the bits where it's confusing are the
   > whole point. Please think out loud as you go. There are no wrong moves
   > here; anything that goes wrong is the tool's problem, not yours."

3. **Read the §4 task script verbatim, then stop talking.** Start the timer on
   the install command. Write the timeline boxes as they happen.

4. **Observe.** Follow §5: no command coaching, ever. If they ask you what to
   do, the only thing you say is:

   > "What would you try?"

   and you write their question down verbatim. If they are hard-stuck 5+
   minutes and you step in, mark **finished unaided = no** and keep observing
   the rest of the journey — the blocker list is the point, and a participant
   who stalled still produces most of the data.

5. **Watch for V1 (the drop-a-file reach) and D2 (re-trust friction)**
   throughout. Never steer toward either. Log every D2 moment, not just the
   first. If either never happens, write "not observed".

6. **After the tasks, ask Q1 then Q2, verbatim, and write the answers down.**
   Do not react to the answers beyond a neutral "thanks".

7. **Then ask V2 once**, verbatim, write the answer word for word, and stop.
   No correcting, no confirming, no filling in the gaps.

8. **Then ask D1 once**, last of all: "Would you keep this installed next week —
   why or why not?" Write it word for word, the why included. Do not persuade,
   do not defend the tool, and do not offer to fix the reason they give. It
   feeds no gate; it is the only demand signal the session produces.

9. **Close.** Thank them. Tell them how to remove the tool if they want it
   gone, and offer first-name credit or anonymity, their pick. Removing it is
   the one place you may answer a direct how-do-I question — the session is
   over and it is their machine.

**Things you must not say, at any point before step 9.**

- Any command name, flag, subcommand, or file path.
- Any of the concept words, unless the participant says it first: manifest,
  toolset, lock, trust, drift, digest, gateway, policy, adapter, target.
- "You could just…", "try…", "it wants you to…", "scroll up", or pointing at
  the screen. Pointing is coaching without words.
- Any reassurance that implies what should happen next: "that looks right",
  "nearly there", "that's the one". Neutral acknowledgement only.
- Any explanation of an error message. Their reading of it *is* the data.
- Anything about the graphical panel. If they ask — "isn't there a UI?", "can
  I see this in an app?" — say exactly this, once, and move on:

  > "There is a graphical companion, but it's not part of today. Today is the
  > command line."

  Do not say whether it is good, finished, available, or coming. If they press,
  repeat the same sentence rather than improvising a second one, and record
  that they asked.

**Right after the call (5 minutes, alone, before the next thing).**

- Fill any timeline box you left blank while watching, from memory, and mark it
  as reconstructed.
- Compute this participant's install→clean-verify minutes and write it on the
  sheet.
- Score review comprehension (correct / partial / wrong) from the written V2
  answer against what the review actually showed them. Score once.
- Copy the participant's number and their five §7 lines onto the running
  results tally.
- Put one calendar entry at **session date + 14 days** for the §9.1 follow-up,
  against this participant's number. Do it now; it is the one part of the study
  that cannot be recovered later.

**Where each number ends up.** Timeline and yes/no boxes → §6's metrics sheet
shape, already reproduced in Appendix A. The four baselines → the §9 addendum
block. D1, D2 and the 14-day answers → §9.1, recorded beside the results and
scored by nobody. The five gate lines → §7's results template, which maps 1:1
onto the Stage 1 gate checkboxes in `TODO.md`.
