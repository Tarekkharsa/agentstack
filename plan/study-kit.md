# Activation study — run sheet

> One page for running the five sessions. It is a pointer to
> [`docs/design/activation-study.md`](../docs/design/activation-study.md), not a
> fork of it: every threshold, script line, and consent term lives there and
> wins over anything here. Section numbers below are that document's.
>
> **STOP — re-pin pass, 2026-08-08. Do not book anyone yet.** The kit was
> re-checked against a binary built from `main` at `ecf5c4d` and two things
> block a run, both maintainer decisions, both written up in **§0.1**:
>
> 1. **The pinned RC is 125 commits stale.** `v0.18.0-rc.2` predates routed
>    delivery, the consent gate over all five delivered kinds, `init
>    --connect`, `NO_COLOR`, and the whole family of next-step fixes. There is
>    no newer tag, so re-pinning needs an RC cut first.
> 2. **`agentstack --version` cannot tell the two builds apart** — the crate
>    version was never bumped after the tag, so `main` also prints
>    `0.18.0-rc.2`. Every version check on this page is inert until that
>    changes.
>
> What the journey now actually does, and what it broke, is in §0.2. What is
> recommended but deliberately not applied is in §0.3. **Nothing below moves a
> threshold** — the §7 pass condition is untouched.

## 1. Who qualifies, and where to find five

Criteria are §1 — all four, no exceptions: uses **two or more** supported agent
CLIs today, has at least one MCP server or skill configured in one of them, did
not build AgentStack and has never used it or watched a demo, and will share a
screen for 30–40 minutes. Screen with the §B2 checklist in chat before booking.

Where to look, in rough order of speed:

- Colleagues and ex-colleagues who run **2+ coding CLIs** daily. Fastest yes,
  and §B1 is the whole risk: if you have demoed it to them, they are
  disqualified, not merely awkward.
- One or two from a Rust or MCP community you already participate in — post the
  §2 broadcast as written, where it is on-topic. Add no link, quickstart, or
  command "to save time".
- Existing users of the public release who have not talked to you about it.

**Do not screen on where their servers are configured** (§B2) — global-scope and
project-scope shapes must both reach the study.

## 2. Pre-flight (per session, 10 minutes, alone)

- [ ] Send **§3's pinned install line**, exactly as written there — the
      `curl … | AGENTSTACK_VERSION=<the RC> sh` form. Not the README's line.
      **The version in §3 is stale (§0.1 B-A); it must be replaced with the RC
      cut for this run before anything is sent.**
- [ ] **Do not send the Homebrew path.** The tap serves 0.17.1 and has no
      prerelease channel, so it never carries an RC. A participant on 0.17.x is
      running a different and older journey and will hit missing verbs that no
      amount of not-coaching can rescue.
- [ ] Check the version yourself first: run §3's line on **your** machine and
      confirm what it produces. Never check what the bare installer serves — it
      serves latest stable on purpose. **The version string alone is not
      sufficient evidence until §0.1 B-B is fixed** — until then, also confirm
      the binary is the one you cut, not merely one that prints the number.
- [ ] In the room, confirm their version. Reads 0.17.x → wrong line, restart
      from the install (§3).
- [ ] Print one Appendix A sheet. Pen. Timer with seconds. §3's consent points
      and §4's task script in front of you, to read aloud.

## 3. The session, in six lines

1. Read §3's consent points, wait for an actual verbal yes.
2. Say the §B3 scene-setting line: you will go quiet, the confusing bits are the
   point, nothing that goes wrong is their fault.
3. Read the §4 task script verbatim, start the timer on the install command,
   then **stop talking**.
4. **No coaching, ever** — no command, flag, file path, or concept word. The
   only answer to "what do I do?" is "what would you try?", written down
   verbatim.
5. **Intervention = failure**: hard-stuck 5+ minutes and you step in → mark
   *finished unaided = no*, then keep observing; the blocker list is the point.
6. Capture verbatim throughout — errors, misread terms, stalls, V1, and every
   D2 re-trust moment. Write what was on screen, not what you think they meant.

Then, in order and verbatim: **Q1, Q2, V2, D1.** Close with §B3 step 9 — removal
instructions are the one direct question you may answer.

**What changed under that script (measured 2026-08-08 — §0.2 has the detail).**
The script is unchanged and is still read verbatim; the product beneath it is
not what the kit described.

- **Delivery is routed.** Servers and skills are served live to a connected
  CLI; only house rules, settings and hooks become files. A participant can
  reach a clean verify with **no `apply` at all** — that is not a missed step.
- **A bridge registration sits inside the gate-metric window.** A clean verify
  is unreachable until it happens: one command if they take it, or a second
  one hop behind `agentstack x`. Record which. Never name either.
- **"Switch to it" looks like nothing happened** on a routed project. Record
  what they believe happened.
- **"Undo your last change" may only be offerable as "undo everything"** — the
  ledger does not carry toolset creation. Read the line anyway; log the
  outcome as a product finding, never against the participant.
- **`status` and `doctor` can disagree after a clean verify** (§8.1's re-pin
  note). If they loop between the two, that is known — log it and keep going.

## 4. Demand: the new question and the calendar line

- **D1, asked last:** *"Would you keep this installed next week — why or why
  not?"* Verbatim, the why included. Do not persuade, do not defend, do not
  offer to fix the reason. It feeds **no gate** (§5, §9.1).
- **Calendar, booked the same day as the session:** *"Study follow-up —
  participant #N"* at **session date + 14 days**. Ask three things (§9.1): still
  installed, run since, one verbatim sentence on why. One message, no reminder,
  no reply is a recorded outcome.

## 5. What to bring back

Per participant: the filled Appendix A sheet (timeline, stall log, free
capture), the §9 baseline addendum, the Q1/Q2/V2 verbatims, the D1 answer, and
the D2 re-trust log with its count. Fourteen days later, the three follow-up
answers on the same numbered sheet.

Where it goes: five gate lines → **§7's results template**, which maps 1:1 onto
the Stage 1 gate checkboxes in `TODO.md`. Four baselines → **§9**. D1, D2 and
the follow-up → **§9.1**, recorded beside the results and scored by nobody.

**The two lines that decide the gate** (§7, unchanged, and never adjusted to fit
a result): at least **4 of 5 finish unaided**, and the **median
install→clean-verify time under 5 minutes**. Everything else on this page
informs the bar; none of it moves the bar.
