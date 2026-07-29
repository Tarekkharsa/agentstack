# Activation study kit (§1.6 / Stage 1 gate)

> **Status:** ready to run · **Owner:** maintainer · **Written:** 2026-07-29
>
> Everything needed to run the five-participant activation study is in this
> one file: who to recruit, what to say, how to observe, what to record, and
> how the results map onto the Stage 1 gate in `TODO.md`. The study is the
> one planned activity that can falsify the product thesis rather than
> confirm it — run it before adding features.

## 1. Participant criteria

- Uses **two or more supported agent CLIs** today (any two of: Claude Code,
  Codex CLI, Gemini CLI, Copilot CLI, OpenCode, Cursor, …).
- Has at least one MCP server or skill configured in at least one of them.
- Did **not** build AgentStack and has not used it before.
- Comfortable sharing their screen for ~30 minutes.

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
> The ask: 30 minutes on a call, screen shared. You install the tool and set
> it up on your own machine with your real configs — I watch and take notes,
> but I don't help; watching where it's confusing *is* the study. Nothing
> leaves your machine, nothing to prepare, and you can rip it out at the end
> (uninstalling cleanly is part of what I'm testing).
>
> Any slot this week or next work for you? I'll owe you a coffee and
> first-name credit in the release notes — or anonymity, your pick.

**Broadcast:**

> Looking for 5 developers who use **2+ AI coding CLIs** (Claude Code, Codex,
> Gemini, Cursor, OpenCode…) for a 30-minute usability session of a tool that
> unifies MCP/skill setup across them. You install it on your machine while
> sharing your screen; I watch and take notes but don't coach — the stumbles
> are the data. No prep, nothing leaves your machine, easy to uninstall
> after. DM me if you're in.

## 3. Setup (before each session)

- Participant's own machine, their real CLI configs. No sandbox, no demo repo.
- They pick a real project of theirs (or an empty directory — their choice).
- Confirm the released binary they'll install carries the current journey:
  `agentstack --version` ≥ 0.17.0.
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
intended gate — the cue is one command), and the Codex project-trust note.
