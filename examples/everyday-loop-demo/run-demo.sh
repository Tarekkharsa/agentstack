#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — the everyday loop, end to end (the five v2 verbs, v0.18.0+).
#
#   "Drop a file in. Say yes. Take it back. Share it. Someone else runs one
#    command and has the same setup."
#
# first-value-demo proves the IMPORT story (many CLIs → one manifest). This one
# proves the LOOP a user lives in afterwards, over the verbs strategy v2 added:
#
#   yes      review and activate what you dropped into this project
#   undo     take a change back, by point in the timeline
#   share    hand the setup to someone else as a signed bundle
#   receive  review a bundle before anything it carries activates
#   up       set a machine up from a setup that already exists
#
# The journey, exactly as two people run it:
#   1. A project with one manifest and a hand-written .mcp.json of its own.
#   2. A skill folder is dropped into .agentstack/skills/. Activating it pins
#      it in the lock, and `use --write` renders it where the CLI reads skills.
#      That render is the ASK, not the automatic path: delivery is routed, and
#      on an MCP-capable tool skills and servers are served live over a lease
#      (see examples/projects/skills-workout for both lanes side by side). This
#      demo takes the rendered lane deliberately — undo/share/receive/up are all
#      about files on disk, and files are what it has to compare byte-for-byte.
#      Both manifests below therefore carry `[delivery] render_locally = true`,
#      which is how a project asks for files under the default routing.
#   3. `agentstack undo --to 1 --write` puts the native config back BYTE FOR
#      BYTE — including the hand-written server the render had merged with.
#   4. `agentstack share` writes a signed .astack bundle.
#   5. On a second machine (its own HOME, its own AGENTSTACK_HOME), `receive`
#      stages the bundle inert, and `up` brings that machine into line.
#
# Two honest notes about what this script does NOT do — see README.md:
#   · `agentstack yes` is deliberately TTY-only. `--yes` answers its prompt but
#     does not replace the terminal, so a headless caller cannot use it. This
#     script ASSERTS that refusal (it is a security property, not a gap) and
#     then runs the explicit path the error message itself names.
#   · `share` embeds the signature IN the bundle; there is no detached sidecar
#     file. The assertion reads the signature out of the bundle instead.
#
# It exits nonzero and prints FAIL on any mismatch, so it is safe to run
# unattended. Self-contained: isolated temp HOMEs, nothing touches your real
# config. Set DEMO_PAUSE=2.5 for a paced screen recording (asciinema).
#
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=..., or a built
# target/release/agentstack in this repo), git, python3.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd -P)"

# ── binary resolution: AGENTSTACK_BIN, else PATH, else this repo's release build
AS="${AGENTSTACK_BIN:-}"
if [[ -z "$AS" ]]; then
  if command -v agentstack >/dev/null 2>&1; then
    AS="$(command -v agentstack)"
  else
    d="$HERE"
    while [[ "$d" != "/" ]]; do
      if [[ -x "$d/target/release/agentstack" ]]; then AS="$d/target/release/agentstack"; break; fi
      d="$(dirname "$d")"
    done
  fi
fi
if [[ -z "$AS" ]]; then
  echo "could not find agentstack: set AGENTSTACK_BIN, add it to PATH, or run 'cargo build --release'" >&2
  exit 2
fi
if [[ "$AS" == */* ]]; then
  AS="$(cd "$(dirname "$AS")" && pwd -P)/$(basename "$AS")"
else
  AS="$(command -v "$AS")"
fi

PASS=0
FAIL=0
ok()  { printf '  \033[32mPASS\033[0m %s\n' "$*"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=$((FAIL + 1)); }

PAUSE="${DEMO_PAUSE:-0.6}"
say()  { printf '\n\033[1;35m▎ %s\033[0m\n' "$*"; sleep "$PAUSE"; }
run()  { printf '\033[2m$ %s\033[0m\n' "$*"; sleep "$PAUSE"; }
note() { printf '  \033[2m%s\033[0m\n' "$*"; }

# ── two isolated machines (nothing touches your real config) ─────────────────
# A short /tmp template rather than $TMPDIR, for the same reason
# first-value-demo uses one: macOS's per-user temp dir wraps recorded output.
SBX="$(mktemp -d /tmp/agentstack-everyday.XXXXXX)"
trap 'rm -rf "$SBX"' EXIT

HOME_A="$SBX/machine-a"; ASHOME_A="$SBX/ashome-a"; PROJECT_A="$SBX/project"
HOME_B="$SBX/machine-b"; ASHOME_B="$SBX/ashome-b"; PROJECT_B="$SBX/teammate"
mkdir -p "$HOME_A" "$ASHOME_A" "$PROJECT_A" "$HOME_B" "$ASHOME_B" "$PROJECT_B" "$SBX/bin"

# A stub `claude` on a controlled PATH, so detection sees exactly one CLI and
# the demo is reproducible on any machine (the same trick device-onboarding
# uses). It is never executed for its behaviour — only its presence matters.
printf '#!/bin/sh\nexit 0\n' > "$SBX/bin/claude"
chmod +x "$SBX/bin/claude"

# Machine A is where the setup is authored; machine B is a colleague's laptop
# that has never seen this project. Separate HOME *and* separate
# AGENTSTACK_HOME: trust and publisher keys are per-machine by design, and
# sharing one would quietly fake the most interesting half of the story.
asa() { env HOME="$HOME_A" AGENTSTACK_HOME="$ASHOME_A" PATH="$SBX/bin:/usr/bin:/bin" "$AS" "$@"; }
asb() { env HOME="$HOME_B" AGENTSTACK_HOME="$ASHOME_B" PATH="$SBX/bin:/usr/bin:/bin" "$AS" "$@"; }

# The explicit, headless activation path — the one `agentstack yes` names in
# its own refusal message. Declare → pin → consent (bound to the previewed
# bytes) → render. `yes` collapses these four into one reviewed step for a
# human at a terminal; it does not do anything different.
activate() { # $1: the `as*` function to run it through
  local as="$1" digest
  "$as" adopt --write >/dev/null
  "$as" lock --write >/dev/null
  digest="$("$as" trust . --preview | sed -n 's/.*"surface_digest": "\([^"]*\)".*/\1/p')"
  "$as" trust . --yes --consented-digest "$digest" >/dev/null
  "$as" use --write >/dev/null
}

printf '\033[1;36m  agentstack — the everyday loop: yes · undo · share · receive · up\033[0m\n'

# ═══ 1. a project with a manifest, and a config it already hand-wrote ════════
cd "$PROJECT_A"
git init -q .
mkdir -p .agentstack
# `[delivery] render_locally = true` is a deliberate opt-in, not a workaround.
# Under the default routing an MCP-capable tool is served live and no native
# server config is written at all — which is right for the first-value journey,
# and useless here: undo/share/receive/up are verbs ABOUT files on disk, and
# this demo's whole point is comparing those files byte-for-byte (step 3 reverts
# a render that had merged with a hand-written server). The rendered lane is
# routed, not removed; this is how a project asks for it.
cat > .agentstack/agentstack.toml <<'EOF'
version = 1

[delivery]
render_locally = true

[servers.docs]
type = "http"
url = "https://docs.example/mcp"

[targets]
default = ["claude-code"]
EOF
# A server this project's author wrote by hand, months ago, outside agentstack.
# It is here to make step 3 mean something: "reverted" has to include content
# agentstack never owned.
cat > .mcp.json <<'EOF'
{
  "mcpServers": {
    "my-hand-server": {
      "type": "http",
      "url": "https://hand.example/mcp"
    }
  }
}
EOF
cp .mcp.json "$SBX/mcp.before"

say "Where we start: one manifest, and a .mcp.json this project wrote by hand."
run "cat .agentstack/agentstack.toml"
sed 's/^/  /' .agentstack/agentstack.toml

# ═══ 2. drop a skill in, and say yes ═════════════════════════════════════════
say "Someone drops a skill folder into the project — just files, nothing wired:"
mkdir -p .agentstack/skills/sql-review
cat > .agentstack/skills/sql-review/SKILL.md <<'EOF'
---
name: sql-review
description: Review SQL migrations before they ship.
---
Check every migration for missing indexes and unbounded scans.
EOF
cp .agentstack/skills/sql-review/SKILL.md "$SBX/skill.before"
run "find .agentstack/skills -type f"
find .agentstack/skills -type f | sed 's/^/  /'

# `yes` is the interactive verb. Prove the gate holds headlessly BEFORE using
# the explicit path — a demo that only ever took the path that works would be
# the one place this refusal could regress unnoticed.
say "\`agentstack yes\` is a review a human reads. Headlessly it refuses — on purpose:"
run "agentstack yes --yes    # no terminal"
YES_OUT="$(asa yes --yes 2>&1)" && YES_EXIT=0 || YES_EXIT=$?
printf '%s\n' "$YES_OUT" | fold -s -w 76 | sed 's/^/  /'
if [ "$YES_EXIT" -ne 0 ]; then
  ok "\`yes --yes\` refuses without a terminal (--yes answers a prompt, it is not a substitute for one)"
else
  bad "\`yes --yes\` accepted an asserted consent nobody was shown"
fi
if grep -q "agentstack trust --yes --consented-digest" <<< "$YES_OUT"; then
  ok "the refusal names the explicit headless path instead of dead-ending"
else
  bad "the refusal should name the headless alternative"
fi
if ! grep -q "sql-review" .agentstack/agentstack.toml && [ ! -f .agentstack/agentstack.lock ]; then
  ok "the refusal left nothing behind — nothing declared, nothing pinned"
else
  bad "a refused activation still wrote something"
fi

say "So we take the path it names: declare → pin → consent → render."
run "agentstack adopt --write && agentstack lock --write && agentstack trust --yes ... && agentstack use --write"
activate asa

# ── assertions: the drop actually went live ──────────────────────────────────
printf '\n\033[1mAsserting the activation:\033[0m\n'
if grep -q "sql-review" .agentstack/agentstack.toml; then
  ok "the dropped skill is declared in the manifest"
else
  bad "the manifest never learned about the dropped skill"
fi
if [ -f .agentstack/agentstack.lock ] && grep -q "sql-review" .agentstack/agentstack.lock; then
  ok "the skill is pinned in agentstack.lock (this is the content that was consented to)"
else
  bad "the skill is not in the lock"
fi
# Where Claude Code actually looks for skills. Reading the file THROUGH that
# path is the assertion — the rendered entry is a symlink, and checking only
# that the link exists would pass over a link pointing at nothing.
if [ -f .claude/skills/sql-review/SKILL.md ] && cmp -s .claude/skills/sql-review/SKILL.md "$SBX/skill.before"; then
  ok "the skill is readable at .claude/skills/ — where the CLI looks for it"
else
  bad "the skill did not reach the CLI's skills directory"
fi
if grep -q "docs.example" .mcp.json && grep -q "my-hand-server" .mcp.json; then
  ok "the render merged the manifest's server in beside the hand-written one"
else
  bad "the render lost either the managed server or the hand-written one"
fi

# ═══ 3. take it back ═════════════════════════════════════════════════════════
say "Changed your mind? The timeline is a list, and you pick a point on it:"
run "agentstack undo"
asa undo | sed 's/^/  /'
run "agentstack undo --to 1 --write"
asa undo --to 1 --write | sed 's/^/  /'

if cmp -s .mcp.json "$SBX/mcp.before"; then
  ok ".mcp.json is byte-identical to before the activation (hand-written server intact)"
else
  bad ".mcp.json was not reverted exactly"
fi
if [ -f .agentstack/skills/sql-review/SKILL.md ]; then
  ok "the dropped file itself is untouched — undo reverts writes, it does not delete your work"
else
  bad "undo deleted the source file it should never have owned"
fi

say "And back again — the undo is itself just another recorded change:"
run "agentstack use --write"
asa use --write >/dev/null
if grep -q "docs.example" .mcp.json && grep -q "my-hand-server" .mcp.json; then
  ok "re-activated: both servers are rendered again"
else
  bad "re-activation did not restore the render"
fi

# ═══ 4. share it ═════════════════════════════════════════════════════════════
say "Hand the whole reviewed setup to someone else — signing is not optional:"
run "agentstack share team-sql-review"
asa share team-sql-review | sed 's/^/  /'

BUNDLE="$PROJECT_A/team-sql-review.astack"
if [ -f "$BUNDLE" ]; then
  ok "the .astack bundle exists"
else
  bad "share wrote no bundle"
fi
# The signature travels INSIDE the bundle rather than as a detached sidecar
# (see the README): assert on the bytes that are actually there.
if python3 - "$BUNDLE" <<'PY'
import json, sys
b = json.load(open(sys.argv[1]))
sys.exit(0 if b.get("signature") and b.get("publisher") else 1)
PY
then
  ok "the bundle carries a publisher key and a signature over its contents"
else
  bad "the bundle is unsigned"
fi
if python3 - "$BUNDLE" <<'PY'
import json, sys
b = json.load(open(sys.argv[1]))
sys.exit(0 if b.get("lock") and b.get("manifest") else 1)
PY
then
  ok "the bundle carries the manifest and the lock the receiver will review against"
else
  bad "the bundle is missing the manifest or the lock"
fi

# ═══ 5. a second machine receives it, then comes up ══════════════════════════
say "A colleague's laptop — its own HOME, its own trust store, its own project:"
cd "$PROJECT_B"
git init -q .
mkdir -p .agentstack
# Machine B asks for the rendered lane too — same reason as machine A, and the
# `up` step below is asserted on the native config it writes.
cat > .agentstack/agentstack.toml <<'EOF'
version = 1

[delivery]
render_locally = true

[servers.docs]
type = "http"
url = "https://docs.example/mcp"

[targets]
default = ["claude-code"]
EOF

run "agentstack receive team-sql-review.astack --yes"
asb receive "$BUNDLE" --yes | sed 's/^/  /'

if [ -f .agentstack/skills/sql-review/SKILL.md ] && cmp -s .agentstack/skills/sql-review/SKILL.md "$SBX/skill.before"; then
  ok "the shared skill landed on the second machine, byte-identical"
else
  bad "the received skill did not land intact"
fi
# The whole point of `receive`: files arrive, capability does not.
if [ ! -e .claude/skills/sql-review ]; then
  ok "nothing is active yet — receive stages, it does not activate"
else
  bad "receive activated content before anyone reviewed it"
fi
if ! grep -q "sql-review" .agentstack/agentstack.toml; then
  ok "the bundle's manifest was NOT merged — the receiver decides what to adopt"
else
  bad "the bundle's manifest was merged without a decision"
fi

say "The colleague reviews it, then brings the whole machine up in one command:"
asb adopt --write >/dev/null
asb lock --write >/dev/null
DIGEST_B="$(asb trust . --preview | sed -n 's/.*"surface_digest": "\([^"]*\)".*/\1/p')"
asb trust . --yes --consented-digest "$DIGEST_B" >/dev/null
# `--write` is load-bearing: bare `up` is a dry run that only prints the plan
# ("Nothing written. Re-run with --write to apply this plan."), and the three
# assertions below are about what the write actually did on this machine.
run "agentstack up --write"
UP_OUT="$(asb up --write 2>&1)" || {
  printf '%s\n' "$UP_OUT"
  bad "agentstack up --write exited nonzero (its output is above)"
  printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
  exit 1
}
printf '%s\n' "$UP_OUT" | sed 's/^/  /'

if grep -q "Claude Code" <<< "$UP_OUT"; then
  ok "up found this machine's CLIs and said so"
else
  bad "up did not report the harnesses it found"
fi
if grep -q "verified against lock" <<< "$UP_OUT"; then
  ok "up verified the received skill source against the lock before rendering"
else
  bad "up rendered without verifying against the lock"
fi
if [ -f .mcp.json ] && grep -q "docs.example" .mcp.json; then
  ok "up rendered this machine's native config"
else
  bad "up rendered no native config"
fi

# `up` renders configs; materializing skill folders is `use --write`'s job (see
# the README). One more step, and the received setup is live on machine B.
run "agentstack use --write"
asb use --write >/dev/null
if [ -f .claude/skills/sql-review/SKILL.md ] && cmp -s .claude/skills/sql-review/SKILL.md "$SBX/skill.before"; then
  ok "the received skill is live on the second machine, byte-identical to the original"
else
  bad "the received skill never went live on the second machine"
fi

cd /
say "Drop it in → say yes → take it back → share it → someone else runs one command."
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
