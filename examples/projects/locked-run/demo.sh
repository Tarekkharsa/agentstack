#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — `run <harness> --locked`: the Protected host tier, as a paced
# narrative for screen recording. THREE scenes, no Docker:
#
#   safe    — a cloned repo: pin it, review it, trust it, launch it locked.
#   policy  — the repo asks for egress your MACHINE policy forbids → refused
#             at admission, before launch, with the exact rule named.
#   drift   — a trusted, locked repo whose pinned server script is edited →
#             refused before launch until you re-lock and re-trust.
#
# Everything runs inside a throwaway HOME — your real ~/.agentstack is never
# touched. This is the demo cousin of the asserting `assert.sh` in this dir;
# assert.sh proves the invariants for CI, demo.sh tells the story for humans.
#
# Usage:
#   ./demo.sh [safe|policy|drift|all]     # default: all
#
# Record one scene into a GIF (asciinema + agg):
#   DEMO_PAUSE=2.5 asciinema rec runtime/locked-safe.cast   --window-size 100x30 -c './demo.sh safe'
#   DEMO_PAUSE=2.5 asciinema rec runtime/locked-policy.cast --window-size 100x30 -c './demo.sh policy'
#   DEMO_PAUSE=2.5 asciinema rec runtime/locked-drift.cast  --window-size 100x30 -c './demo.sh drift'
#   agg --font-size 14 runtime/locked-safe.cast   ../../../docs/demos/locked-safe.gif
#   agg --font-size 14 runtime/locked-policy.cast ../../../docs/demos/locked-policy.gif
#   agg --font-size 14 runtime/locked-drift.cast  ../../../docs/demos/locked-drift.gif
#
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=…, or a built
# target/release/agentstack in this repo).
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd -P)"
SCENE="${1:-all}"

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
[[ "$AS" == */* ]] && AS="$(cd "$(dirname "$AS")" && pwd -P)/$(basename "$AS")" || AS="$(command -v "$AS")"

# ── pacing + narration ───────────────────────────────────────────────────────
PAUSE="${DEMO_PAUSE:-0}"
C_H='\033[1;36m'; C_SAY='\033[1;35m'; C_CMD='\033[1;32m'; C_DIM='\033[2m'; C_0='\033[0m'
say()  { printf "\n${C_SAY}▎ %s${C_0}\n" "$*"; [ "$PAUSE" = "0" ] || sleep "$PAUSE"; }
banner() { printf "\n${C_H}  %s${C_0}\n" "$*"; }
# run "<display>" "<real>": show the clean command a user would type, then run
# the actual (often plumbed) command. If <real> is omitted, display == real.
# The displayed line still collapses the resolved binary path to `agentstack`.
run() {
  local disp="$1" real="${2:-$1}"
  printf "${C_CMD}\$ %s${C_0}\n" "${disp//$AS/agentstack}"
  [ "$PAUSE" = "0" ] || sleep 0.6
  eval "$real" || true
  [ "$PAUSE" = "0" ] || sleep "$PAUSE"
}

# ── a fresh fenced machine + cloned repo for each scene ──────────────────────
# machine_deny: optional `[policy.egress]` line for the machine manifest.
setup() {
  local machine_deny="${1:-}"
  # A short, fixed path (not mktemp) keeps recorded output lines from wrapping.
  SBX="/tmp/agentstack-locked-demo"
  rm -rf "$SBX"
  mkdir -p "$SBX"
  export AGENTSTACK_HOME="$SBX/home"
  export HOME="$SBX/fakehome"
  mkdir -p "$AGENTSTACK_HOME" "$HOME"
  # a fake `claude` on PATH so a clean locked run has something to launch
  mkdir -p "$SBX/bin"
  printf '#!/bin/sh\necho "  [claude launched — project is trusted, locked, and admitted]"\nexit 0\n' > "$SBX/bin/claude"
  chmod 755 "$SBX/bin/claude"
  export PATH="$SBX/bin:$PATH"
  # the machine policy the repo can never loosen
  {
    echo "version = 1"
    if [[ -n "$machine_deny" ]]; then
      echo "[policy.egress]"
      echo "$machine_deny"
    fi
  } > "$AGENTSTACK_HOME/agentstack.toml"
  # the cloned repo
  PROJECT="$SBX/agent-repo"
  mkdir -p "$PROJECT"
  cp -R "$HERE/bundle/." "$PROJECT/"
  cd "$PROJECT"
}
teardown() { cd /; rm -rf "$SBX"; }

# ─────────────────────────────────────────────────────────────────────────────
scene_safe() {
  setup
  banner "run --locked · SAFE REPO — pin it, review it, trust it, launch it"
  say "You cloned a repo that declares two MCP servers. It is inert until you consent."
  run "$AS lock && $AS trust ." "$AS lock >/dev/null && $AS trust . --yes 2>&1 | sed -n '1,3p'"
  say "The surface is pinned and trusted. Preview the locked launch — it runs nothing."
  run "$AS run claude-code --locked --plan" "$AS run claude-code --locked --plan 2>&1 | tail -n 12"
  say "Every gate is green. Launch for real — the grant freezes, then the harness runs."
  run "$AS run claude-code --locked" "$AS run claude-code --locked 2>&1 | grep -avE '^\$' | tail -n 8"
  say "Nothing ran until it was trusted; the run's decisions are recorded."
  teardown
}

scene_policy() {
  # the repo declares an HTTP server; the MACHINE denies that host
  setup '"*" = ["!api.partner.example"]'
  # swap in a manifest that requests the forbidden egress
  cat > "$PROJECT/.agentstack/agentstack.toml" <<'EOF'
version = 1

[servers.partner-api]
type = "http"
url = "https://api.partner.example/mcp"
EOF
  banner "run --locked · POLICY VIOLATION — the machine ceiling no repo can loosen"
  say "Your machine policy forbids egress to api.partner.example — on every server."
  run "cat \$AGENTSTACK_HOME/agentstack.toml"
  say "This cloned repo declares an HTTP server that wants exactly that host."
  run "grep -A2 'servers.partner-api' .agentstack/agentstack.toml"
  run "$AS lock && $AS trust ." "$AS lock >/dev/null && $AS trust . --yes >/dev/null 2>&1; echo 'pinned + trusted'"
  say "You can trust the bytes — but a locked run still refuses what policy forbids."
  run "$AS run claude-code --locked" "$AS run claude-code --locked 2>&1 | grep -iaE 'refused|policy.egress|admission' | head -n 4"
  say "Refused at admission, before launch — the exact rule and its source, named."
  teardown
}

scene_drift() {
  setup
  banner "run --locked · DRIFT — a pinned byte changes, the run re-gates"
  say "The repo is pinned and trusted, and a locked run launches cleanly."
  run "$AS run claude-code --locked" "$AS lock >/dev/null && $AS trust . --yes >/dev/null 2>&1; $AS run claude-code --locked 2>&1 | tail -n 2"
  say "Now someone edits a server executable that was pinned by \`lock\`."
  run "echo '# tampered' >> opsbox.sh" "printf '# a new line, after lock\n' >> opsbox.sh; echo 'opsbox.sh edited'"
  say "The next locked run refuses BEFORE launch — the pinned byte no longer matches."
  run "$AS run claude-code --locked" "$AS run claude-code --locked 2>&1 | grep -iaE 'refused|opsbox|drift|lock' | head -n 4"
  say "Re-lock and re-trust to readmit — a consent re-gate, never a silent lockout."
  run "$AS lock && $AS trust . && $AS run claude-code --locked" "$AS lock >/dev/null && $AS trust . --yes >/dev/null 2>&1; $AS run claude-code --locked 2>&1 | tail -n 2"
  teardown
}

case "$SCENE" in
  safe)   scene_safe ;;
  policy) scene_policy ;;
  drift)  scene_drift ;;
  all)    scene_safe; scene_policy; scene_drift ;;
  *) echo "usage: ./demo.sh [safe|policy|drift|all]" >&2; exit 2 ;;
esac
