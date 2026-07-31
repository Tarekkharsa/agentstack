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

### v2 observation prompts (Phase 0 — observe, never coach)

Two more things to watch for while running the protocol above. They add
observation only: no new tasks, no changes to the task script, no coaching, and
no new gate. The two gate questions stay exactly two — the follow-up below is
asked after them and feeds no gate metric. Nothing recorded here changes the §7
pass condition.

1. **The drop-a-file reach.** Watch for the moment the tester behaves as if
   placing a file is enough — writing, pasting, or copying a skill,
   instruction, or config file into the project and expecting it to be live —
   and then stalls because it is not. Record the exact moment (timestamp and
   what they had just done), the exact path of the file they created or edited,
   and what they said, verbatim. Do not tell them whether it works. If they
   never reach for it, write "not observed"; never steer them toward it.
2. **What their yes granted.** After the two questions above, ask once: "Earlier
   you approved this project — in your own words, what did that approve?" Write
   the answer down verbatim, word for word, then stop: no correcting, no
   confirming, no filling in gaps, no second attempt. It is scored later against
   what the review actually showed (§9, review comprehension). If no review
   appeared in their session, write "no review shown".

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
