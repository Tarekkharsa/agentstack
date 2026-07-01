#!/usr/bin/env bash
# First-run demo — the clean adoption story on a *fresh machine*, fully fenced.
# Simulates a dev who already uses Claude Code with one MCP server and adopts
# agentstack to spread it across every other CLI. Proves the core loop
#   init → bootstrap → doctor --ci → apply → apply --write
# without ever touching your real ~/.claude.json etc. Idempotent: wipes its own
# throwaway HOME each run, so it is always a genuine first run.
#
# Record it into a GIF/video (optional):
#   vhs demo-firstrun.tape                    # writes ../docs/firstrun.gif
#   mkdir -p runtime
#   asciinema rec runtime/firstrun.cast -c ./demo-firstrun.sh
#   agg runtime/firstrun.cast ../docs/firstrun.gif    # asciinema → GIF (github.com/asciinema/agg)
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

# Build the debug binary.
( cd .. && source "$HOME/.cargo/env" 2>/dev/null || true; cargo build --quiet )

bin="$here/../target/debug/agentstack"
sandbox="$here/runtime/firstrun"          # throwaway simulated machine (gitignored)
home="$sandbox/home"
proj="$sandbox/proj"

# Always a genuine first run: wipe the fenced HOME + project each time.
rm -rf "$sandbox"
mkdir -p "$home" "$proj"

# agentstack against the fenced machine — never your real HOME.
as() { env HOME="$home" AGENTSTACK_HOME="$sandbox/ashome" "$bin" "$@"; }
# DEMO_PAUSE=<seconds> holds each section on screen — set it when recording so
# the GIF is readable; defaults to 0 so tests and CI stay fast.
line() { sleep "${DEMO_PAUSE:-0}"; printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

# Starting point: this dev already runs Claude Code with ONE MCP server, in the
# canonical shape Claude itself writes (explicit "type"). Keeping it canonical
# means apply leaves Claude untouched and the story stays clean: Claude already
# has it, agentstack spreads it to the *other* CLIs.
cat > "$home/.claude.json" <<'JSON'
{
  "mcpServers": {
    "filesystem": {
      "type": "stdio",
      "command": "npx",
      "args": [
        "-y",
        "@modelcontextprotocol/server-filesystem",
        "~/code"
      ]
    }
  }
}
JSON

cd "$proj"

line "0. Starting point — Claude Code already has one MCP server, nothing else"
cat "$home/.claude.json"

line "1. init — detect the CLIs on this machine, import what already exists"
as init

line "2. The generated manifest — one server, portable across every CLI"
cat "$proj/.agentstack/agentstack.toml"

line "3. bootstrap — preflight: validate, list adapters, preview the plan"
as bootstrap

line "4. doctor --ci — the trust gate (must exit 0 on a clean manifest)"
if as doctor --ci; then
  printf '\033[32m→ doctor --ci exited 0 — gate passed\033[0m\n'
else
  printf '\033[31m→ doctor --ci exited %d — gate FAILED\033[0m\n' "$?"
fi

line "5. apply — dry-run diff: the one server spreads to every other CLI"
as apply

line "6. apply --write — do it for real (into the fenced HOME only)"
as apply --write

line "7. Proof: one server, now synced across every CLI config"
for cfg in \
  "$home/.claude.json" \
  "$home/.codex/config.toml" \
  "$home/.copilot/mcp-config.json" \
  "$home/.gemini/settings.json" \
  "$home/.config/opencode/opencode.json"; do
  if [ -f "$cfg" ] && grep -q filesystem "$cfg"; then
    printf '  \033[32m✓\033[0m %s\n' "${cfg#$home/}"
  else
    printf '  \033[33m—\033[0m %s (no filesystem entry)\n' "${cfg#$home/}"
  fi
done

line "8. Boring and safe: re-running apply is a clean no-op (nothing to write)"
as apply

line "9. …and the gate still passes — every target in sync"
if as doctor --ci; then
  printf '\033[32m→ doctor --ci exited 0 — reproducible\033[0m\n'
fi

printf '\n\033[1mDone.\033[0m Fenced HOME lived under %s — your real configs were never touched.\n' "${sandbox#$here/}"
sleep "${DEMO_PAUSE:-0}"
