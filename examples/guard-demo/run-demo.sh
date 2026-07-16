#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — the guard demo, as a CI-grade proof.
#
# `agentstack guard` wires a COOPERATIVE pre-tool-use hook into agent CLIs
# (Claude Code, Codex, Gemini, Cursor, …). Before the harness runs a tool it
# hands the pending call to `agentstack guard check`, which decides allow/deny
# from the machine's own config — and records every denial to the audit log.
#
# This script feeds realistic Claude-Code-format pre-tool-use payloads into
# `agentstack guard check` and ASSERTS each outcome:
#
#   1. rm -rf outside the workspace   → BLOCKED (write outside the workspace)
#   2. git reset --hard               → BLOCKED (discards uncommitted work)
#   3. cat .env                       → BLOCKED ([policy.filesystem] deny glob)
#   4. an ordinary safe command       → ALLOWED (the guard stays out of the way)
#
# …then greps the audit log to prove the three denials were recorded as
# `host-guard` entries.
#
# It exits nonzero and prints FAIL on any mismatch, so it is safe to run
# unattended in CI: the demo either provably works or fails loudly.
#
# What this demo does NOT claim — see README.md. In one line: this is
# COOPERATIVE protection. It catches an agent's *accidents* because the harness
# chooses to consult the hook. A harness that ignores its own hook protocol, or
# hostile code, bypasses it entirely; kernel-enforced confinement is
# `agentstack run --sandbox` / `--lockdown`.
#
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=...) and python3. If no
# release binary is found it is built with `cargo build --release` (minutes).
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd -P)"
REPO_ROOT="$(cd "$HERE/../.." && pwd -P)"

# ── locate (or build) the binary ─────────────────────────────────────────────
AS="${AGENTSTACK_BIN:-}"
if [ -z "$AS" ]; then
  if [ -x "$REPO_ROOT/target/release/agentstack" ]; then
    AS="$REPO_ROOT/target/release/agentstack"
  elif command -v agentstack >/dev/null 2>&1; then
    AS="agentstack"
  else
    printf 'No agentstack binary found — building release (this can take minutes)…\n'
    # shellcheck disable=SC1090
    [ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
    (cd "$REPO_ROOT" && cargo build --release)
    AS="$REPO_ROOT/target/release/agentstack"
  fi
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
# Optional pacing for screen recordings (DEMO_PAUSE=2.5). Off by default so CI
# stays fast; a no-op when unset.
PAUSE="${DEMO_PAUSE:-0}"
pause() { [ "$PAUSE" = "0" ] || sleep "$PAUSE"; }

# ── isolated sandbox (nothing touches your real config) ──────────────────────
SBX="$(mktemp -d)"
export AGENTSTACK_HOME="$SBX/home"
export HOME="$SBX/fakehome"
mkdir -p "$AGENTSTACK_HOME" "$HOME"
trap 'rm -rf "$SBX"' EXIT

AUDIT="$AGENTSTACK_HOME/audit/calls.jsonl"

# ── the user's OWN machine config — which no repo can loosen ─────────────────
# `[guard] enabled` turns the hook on; `[policy.filesystem] deny` names the
# paths that may never be read or written (the same list `guard install` seeds).
cat > "$AGENTSTACK_HOME/agentstack.toml" <<'EOF'
version = 1

[guard]
enabled = true

[policy.filesystem]
deny = [".env", ".env.local", "id_rsa", "*.pem"]
EOF

# ── a realistic workspace the agent is "working in" ──────────────────────────
# The `.git` marker anchors the guard's workspace here, so a write higher up the
# tree reads as "outside the workspace". A planted (fake) secret makes the
# deny-glob block tangible.
WS="$SBX/project"
mkdir -p "$WS/.git"
SECRET_NAME=".env"
printf 'API_KEY=sk-demo-FAKE-not-a-real-secret-0000\n' > "$WS/$SECRET_NAME"

# Build a Claude-Code pre-tool-use payload for a Bash tool call. $1 = command.
payload() {
  python3 - "$WS" "$1" <<'PY'
import json, sys
ws, command = sys.argv[1], sys.argv[2]
print(json.dumps({
    "session_id": "guard-demo",
    "cwd": ws,
    "hook_event_name": "PreToolUse",
    "tool_name": "Bash",
    "tool_input": {"command": command},
}))
PY
}

# Run one payload through `guard check` and classify the decision.
# Claude protocol: a DENY emits a JSON envelope on stdout; an ALLOW is silent.
# Both exit 0 (the block signal is the JSON body, not the exit code), so we
# read stdout, not $?.
decide() {
  local out
  out="$(payload "$1" | "$AS" guard check --protocol claude 2>/dev/null || true)"
  if grep -q '"permissionDecision":"deny"' <<< "$out"; then
    echo "BLOCKED"
  else
    echo "ALLOWED"
  fi
}

# Assert a command is blocked / allowed.
assert_blocked() {
  local label="$1" cmd="$2"
  printf '\033[2m$ %s\033[0m\n' "$cmd"
  pause
  if [ "$(decide "$cmd")" = "BLOCKED" ]; then
    ok "$label — blocked"
  else
    bad "$label — expected BLOCKED, guard allowed it"
  fi
}
assert_allowed() {
  local label="$1" cmd="$2"
  printf '\033[2m$ %s\033[0m\n' "$cmd"
  pause
  if [ "$(decide "$cmd")" = "ALLOWED" ]; then
    ok "$label — allowed"
  else
    bad "$label — expected ALLOWED, guard blocked it"
  fi
}

printf '\n\033[1mAgentStack — guard demo (asserting)\033[0m\n'
printf '\033[2mThe agent is working in %s; every tool call is checked first.\033[0m\n' "the-project"
pause

printf '\n\033[1m1) A destructive shell command outside the workspace\033[0m\n'
pause
assert_blocked "rm -rf outside the workspace" "rm -rf /opt/acme/data"

printf '\n\033[1m2) A history-destroying git command\033[0m\n'
pause
assert_blocked "git reset --hard" "git reset --hard HEAD~3"

printf '\n\033[1m3) Reading a secret the machine policy denies\033[0m\n'
pause
assert_blocked "cat a [policy.filesystem] deny path" "cat $SECRET_NAME"

printf '\n\033[1m4) An ordinary, safe command\033[0m\n'
pause
assert_allowed "an everyday command" "ls -la"

# ── the audit trail: every denial is recorded as a host-guard entry ──────────
printf '\n\033[1mThe audit log recorded each denial (server=host-guard):\033[0m\n'
pause
DENIALS="$(grep -c 'host-guard' "$AUDIT" 2>/dev/null || true)"
DENIALS="${DENIALS:-0}"
if [ "$DENIALS" -eq 3 ]; then
  ok "3 host-guard denials written to the audit log"
  grep 'host-guard' "$AUDIT" | python3 -c '
import json, sys
for line in sys.stdin:
    d = json.loads(line)
    print("  audit:", {k: d.get(k) for k in ("server", "tool", "outcome")})'
else
  bad "expected 3 host-guard audit records, found ${DENIALS}"
fi

pause
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
