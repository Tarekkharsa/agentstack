#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — the first-value proof, fenced and reproducible (TODO §1.5).
#
#   "You already configured your coding CLIs — separately, in different
#    formats. Import that once, and every CLI gets the whole setup."
#
# The journey, exactly as a new user runs it:
#   1. START from two real native configs: Claude Code (~/.claude.json) knows
#      a `github` server with an inline token; Codex (~/.codex/config.toml)
#      knows a `tldraw` server. Neither CLI knows the other's server.
#   2. `agentstack init --yes --secrets env` — ONE import writes ONE manifest;
#      the inline token is lifted to a `${GITHUB_TOKEN}` reference whose value
#      lands in a gitignored .env, never in the manifest.
#   3. `agentstack apply --scope global --write` — the one manifest renders
#      back into BOTH native formats: now each CLI has BOTH servers.
#   4. `agentstack doctor` — a clean bill of health.
#   5. `agentstack restore --last --write` (twice: render, then import) — the
#      machine returns byte-for-byte to where it started.
#
# It exits nonzero and prints FAIL on any mismatch, so it is safe to run
# unattended. Self-contained: isolated temp HOME, nothing touches your real
# config. Set DEMO_PAUSE=2.5 for a paced screen recording (asciinema).
#
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=..., or a built
# target/release/agentstack in this repo).
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

# ── isolated sandbox (nothing touches your real config) ──────────────────────
SBX="$(mktemp -d)"
FAKEHOME="$SBX/home"
export AGENTSTACK_HOME="$SBX/agentstack-home"
mkdir -p "$AGENTSTACK_HOME" "$FAKEHOME"
trap 'rm -rf "$SBX"' EXIT

# Run agentstack inside the sandbox: fake HOME so the "native configs" are the
# fixtures below, and a controlled PATH holding stub `claude`/`codex` binaries
# so detection sees exactly those two CLIs — reproducible on any machine.
mkdir -p "$SBX/bin"
for cli in claude codex; do
  printf '#!/bin/sh\nexit 0\n' > "$SBX/bin/$cli"
  chmod +x "$SBX/bin/$cli"
done
as() { env HOME="$FAKEHOME" PATH="$SBX/bin:/usr/bin:/bin" "$AS" "$@"; }

# The (fake) token sitting in a live config; it must never enter the manifest.
TOKEN="ghp-demo-FAKE-not-a-real-secret-0000"

# ── the starting point: two CLIs, two formats, two half-setups ───────────────
cat > "$FAKEHOME/.claude.json" <<EOF
{
  "mcpServers": {
    "github": {
      "command": "/usr/bin/env",
      "args": ["npx", "-y", "github-mcp"],
      "env": { "GITHUB_TOKEN": "$TOKEN" }
    }
  }
}
EOF
mkdir -p "$FAKEHOME/.codex"
cat > "$FAKEHOME/.codex/config.toml" <<'EOF'
[mcp_servers.tldraw]
command = "/usr/bin/env"
args = ["npx", "-y", "tldraw-mcp"]
EOF
# Byte-exact copies to prove restoration at the end.
cp "$FAKEHOME/.claude.json" "$SBX/claude.before"
cp "$FAKEHOME/.codex/config.toml" "$SBX/codex.before"

PROJECT="$SBX/project"
mkdir -p "$PROJECT/.git"   # a git project, so the lifted-secret .env is gitignored
cd "$PROJECT"

printf '\033[1;36m  agentstack — import once, use it across every coding CLI\033[0m\n'

say "Today: two CLIs, two formats, two half-setups. Claude Code knows 'github':"
run "cat ~/.claude.json"
sed 's/^/  /' "$FAKEHOME/.claude.json"
say "Codex knows 'tldraw' — in TOML, at a different path:"
run "cat ~/.codex/config.toml"
sed 's/^/  /' "$FAKEHOME/.codex/config.toml"
note "Neither CLI has the other's server, and a live token sits in plain JSON."

say "Import everything once — one command, one manifest:"
run "agentstack init --yes --secrets env"
as init --yes --secrets env 2>&1 | sed 's/^/  /'

say "The manifest is the portable source of truth — and it is commit-safe:"
run "grep -n GITHUB_TOKEN .agentstack/agentstack.toml"
grep -n "GITHUB_TOKEN" .agentstack/agentstack.toml | sed 's/^/  /'
if grep -q '${GITHUB_TOKEN}' .agentstack/agentstack.toml && ! grep -q "$TOKEN" .agentstack/agentstack.toml; then
  ok "the manifest holds \${GITHUB_TOKEN}, never the value (it lives in the gitignored .env)"
else
  bad "the manifest must hold only the placeholder"
fi

say "Render the one manifest back into BOTH native formats:"
run "agentstack apply --scope global --write"
as apply --scope global --write 2>&1 | tail -6 | sed 's/^/  /'

say "Now each CLI has BOTH servers, in its own format:"
run "cat ~/.claude.json ~/.codex/config.toml"
sed 's/^/  /' "$FAKEHOME/.claude.json"
printf '\n'
sed 's/^/  /' "$FAKEHOME/.codex/config.toml"

# ── assertions: the cross-CLI fan-out actually happened ──────────────────────
printf '\n\033[1mAsserting the outcome:\033[0m\n'
if grep -q "tldraw" "$FAKEHOME/.claude.json"; then
  ok "Claude Code gained 'tldraw' (imported from Codex)"
else
  bad "Claude Code is missing 'tldraw'"
fi
if grep -q "github" "$FAKEHOME/.codex/config.toml"; then
  ok "Codex gained 'github' (imported from Claude Code)"
else
  bad "Codex is missing 'github'"
fi
if grep -q "$TOKEN" "$FAKEHOME/.claude.json" && ! grep -q "$TOKEN" .agentstack/agentstack.toml; then
  ok "the token resolved into the native config; the manifest still holds the placeholder"
else
  bad "secret handling: value must reach native configs only, never the manifest"
fi

say "Is everything healthy? One status command:"
run "agentstack doctor"
DOCTOR_OUT="$(as doctor 2>&1)" && DOCTOR_EXIT=0 || DOCTOR_EXIT=$?
printf '%s\n' "$DOCTOR_OUT" | tail -4 | sed 's/^/  /'
if [ "$DOCTOR_EXIT" -eq 0 ] && printf '%s' "$DOCTOR_OUT" | grep -q "0 error(s), 0 warning(s)"; then
  ok "doctor is clean (0 errors, 0 warnings)"
else
  bad "doctor should be clean after the guided journey (exit $DOCTOR_EXIT)"
fi

say "Changed your mind? Every write is recorded — undo the render, then the import:"
run "agentstack restore --last --write   # undoes the apply"
as restore --last --write 2>&1 | tail -2 | sed 's/^/  /'
run "agentstack restore --last --write   # undoes the import"
as restore --last --write 2>&1 | tail -2 | sed 's/^/  /'

if cmp -s "$FAKEHOME/.claude.json" "$SBX/claude.before"; then
  ok "~/.claude.json is byte-identical to where it started"
else
  bad "~/.claude.json was not restored exactly"
fi
if cmp -s "$FAKEHOME/.codex/config.toml" "$SBX/codex.before"; then
  ok "~/.codex/config.toml is byte-identical to where it started"
else
  bad "~/.codex/config.toml was not restored exactly"
fi
if [ ! -f .agentstack/agentstack.toml ] && [ ! -f .agentstack/.env ] && [ ! -f .env ]; then
  ok "the manifest and the secrets .env are gone — the machine is exactly as it was"
else
  bad "restore left onboarding files behind"
fi

say "Import once → both CLIs in sync → clean doctor → fully reversible."
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
