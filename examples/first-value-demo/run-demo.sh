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
#   2. `agentstack init --yes --secrets env` — ONE import. The server
#      DEFINITIONS land in the library; the manifest references them by name.
#      The inline token is lifted to a `${GITHUB_TOKEN}` reference in the
#      library definition, and its value lands in a gitignored .env.
#   3. `agentstack more gateway connect --all --write` — register the bridge each
#      MCP-capable tool talks to. This is what makes the live lane real; until
#      it runs, delivery and doctor both say so plainly.
#   4. `agentstack delivery` — how each tool actually gets them: on an
#      MCP-capable tool, servers are served live, so the project stays clean —
#      it holds `.agentstack/` and nothing else.
#   5. `agentstack doctor` — a clean bill of health.
#   6. `agentstack more delivery render-locally --write`, then
#      `agentstack apply --toolset default --scope global --write` — the
#      rendered lane, for when you want the files anyway. Asking for files is
#      now an explicit opt-in; the rendered lane is routed, not removed, and
#      BOTH native formats end up with BOTH servers.
#   7. `agentstack restore --last --write` (four times: render, the
#      render-locally override, the bridge, then the import) — the machine
#      returns byte-for-byte to where it started, library included.
#
# The secret claim is the one this demo exists to make, and after the library
# inversion it lives in three places at once, so it is proven three times: the
# manifest holds NO secret material, the library definition holds the
# `${REF}` and never the value, and the value exists only in a gitignored .env.
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
# An explicit short /tmp template instead of $TMPDIR: macOS's per-user temp dir
# is a ~50-char path that wraps the recorded output's file-path lines.
SBX="$(mktemp -d /tmp/agentstack-demo.XXXXXX)"
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

say "The definitions went to the library; the manifest just names them:"
run "cat .agentstack/agentstack.toml"
sed 's/^/  /' .agentstack/agentstack.toml
LIBDEF="$AGENTSTACK_HOME/lib/servers/github.toml"
run "cat \$AGENTSTACK_HOME/lib/servers/github.toml"
sed 's/^/  /' "$LIBDEF"

# The commit-safety claim, split three ways because the truth now lives in
# three files. Asserting it on the manifest alone would pass for the wrong
# reason: the manifest is secret-free because it defines nothing at all.
printf '\n\033[1mThe token is in exactly one of these three places:\033[0m\n'
if ! grep -q 'GITHUB_TOKEN' .agentstack/agentstack.toml \
   && ! grep -qF "$TOKEN" .agentstack/agentstack.toml \
   && grep -q '"github"' .agentstack/agentstack.toml; then
  ok "the manifest holds no secret material at all — it references 'github' by name"
else
  bad "the manifest must name the server and carry no secret material"
fi
if grep -q '${GITHUB_TOKEN}' "$LIBDEF" && ! grep -qF "$TOKEN" "$LIBDEF"; then
  ok "the library's github definition holds \${GITHUB_TOKEN}, never the value"
else
  bad "the library definition must hold the placeholder, never the value"
fi
# Where the value IS — and, by exhaustion, nowhere else under the project or
# the library. `grep -rl` over both trees, minus the one file allowed to have it.
LEAKS="$(grep -rlF "$TOKEN" . "$AGENTSTACK_HOME/lib" 2>/dev/null | grep -v '^\./\.agentstack/\.env$' || true)"
if grep -qF "$TOKEN" .agentstack/.env \
   && grep -q '/\.agentstack/\.env' .gitignore \
   && [ -z "$LEAKS" ]; then
  ok "the value exists only in .agentstack/.env, which init gitignored"
else
  bad "the value must live only in the gitignored .env (also found in: ${LEAKS:-nowhere})"
fi

# The live lane needs one registration: each MCP-capable tool has to be told
# about the bridge it will talk to. Until this runs, delivery and doctor both
# report "planned live (not connected)" — honest, and the reason doctor is not
# clean yet. The demo resolves the complaint rather than lowering the bar.
say "The live lane needs one bridge registered — one command, both tools:"
run "agentstack more gateway connect --all --write"
as more gateway connect --all --write 2>&1 | tail -3 | sed 's/^/  /'

say "How does each tool actually get them? Delivery is routed — ask it:"
run "agentstack delivery"
DELIVERY_OUT="$(as delivery 2>&1)"
printf '%s\n' "$DELIVERY_OUT" | sed 's/^/  /'
# Dynamic is the default: on an MCP-capable tool the servers are served live
# over the gateway (an open lease naming the toolset), so nothing is written.
if grep -q "Claude Code.*MCP servers served live" <<< "$DELIVERY_OUT" \
   && grep -q "Codex CLI.*MCP servers served live" <<< "$DELIVERY_OUT"; then
  ok "both tools are routed to receive the servers live — no files needed for them"
else
  bad "delivery should route MCP servers live on both MCP-capable tools"
fi
# The claim "nothing on disk" is now checked where it is actually true: in the
# project. Under the default routing the import writes no native server config
# into the repo at all — only AgentStack's own `.agentstack/` directory and the
# .gitignore that hides the lifted secret. Asserting on the delivery TEXT would
# be weaker; this asserts on the tree itself.
STRAY="$(find . -mindepth 1 -maxdepth 1 \
          ! -name .agentstack ! -name .git ! -name .gitignore | sort || true)"
if [ -z "$STRAY" ] && [ ! -e .mcp.json ] && [ ! -e .claude ] && [ ! -e .codex ]; then
  ok "the project holds only .agentstack/ — the live lane wrote no config into the repo"
else
  bad "the live lane left files in the project: ${STRAY:-none at top level}"
fi

say "Is everything healthy? One status command:"
run "agentstack doctor"
DOCTOR_OUT="$(as doctor 2>&1)" && DOCTOR_EXIT=0 || DOCTOR_EXIT=$?
printf '%s\n' "$DOCTOR_OUT" | tail -4 | sed 's/^/  /'
# Match the summary line doctor actually prints: "0 errors, 0 warnings" with
# an optional ", N notes" tail. The old literal "0 error(s), 0 warning(s)"
# predates the conjugation sweep and had stopped matching anything.
if [ "$DOCTOR_EXIT" -eq 0 ] && grep -qE "0 errors, 0 warnings" <<< "$DOCTOR_OUT"; then
  ok "doctor is clean (0 errors, 0 warnings)"
else
  bad "doctor should be clean after the import (exit $DOCTOR_EXIT)"
fi

# ── the rendered lane, on request ────────────────────────────────────────────
# The live lane above is what happens automatically. Asking for files is now an
# explicit opt-in: `delivery render-locally` records it for this project, and
# `apply` then writes them. The rendered lane is routed, not removed. It is the
# same one import fanning out, so the toolset has to be named: the definitions
# are in the library now, not in the manifest's own [servers].
say "Want the files anyway? Say so once — the rendered lane is routed, not gone:"
run "agentstack more delivery render-locally --write"
as more delivery render-locally --write 2>&1 | sed 's/^/  /'

say "Now ask for them — one import, both native formats:"
run "agentstack apply --toolset default --scope global --write"
as apply --toolset default --scope global --write 2>&1 | tail -6 | sed 's/^/  /'

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
if grep -qF "$TOKEN" "$FAKEHOME/.claude.json" && ! grep -qF "$TOKEN" "$LIBDEF"; then
  ok "the token resolved into the native config; the library entry still holds the placeholder"
else
  bad "secret handling: value must reach native configs only, never the library"
fi

say "Changed your mind? Every write is recorded — walk the timeline back:"
run "agentstack restore --last --write   # undoes the apply"
as restore --last --write 2>&1 | tail -2 | sed 's/^/  /'
run "agentstack restore --last --write   # undoes the render-locally override"
as restore --last --write 2>&1 | tail -2 | sed 's/^/  /'
run "agentstack restore --last --write   # undoes the bridge registration"
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
# The import reached OUTSIDE this project, into the shared library. An undo that
# stopped at the project boundary would leave the definitions (and the ${REF})
# behind on the machine.
if [ ! -f "$LIBDEF" ] && [ -z "$(find "$AGENTSTACK_HOME/lib" -type f 2>/dev/null)" ]; then
  ok "the library entries the import created are gone too — the undo followed it out of the project"
else
  bad "restore left the imported library definitions behind"
fi

say "Import once → served live to both CLIs → clean doctor → fully reversible."
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
