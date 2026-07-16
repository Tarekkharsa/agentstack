#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — one manifest, every CLI. The portability wedge, as a CI proof.
#
#   "Author your agent setup ONCE. `agentstack apply` renders it into each
#    CLI's own native config format — Claude Code, Codex, and Cursor here —
#    and your secret stays a `${REF}` in the manifest, resolved per-machine."
#
# The demo:
#   1. Shows the single committed manifest and proves it holds NO secret value,
#      only a `${GITHUB_TOKEN}` placeholder.
#   2. Runs `agentstack apply` as a read-only PREVIEW — the per-CLI plan, three
#      different native files at three different paths, from one source.
#   3. Runs `agentstack apply --write` with the token resolved from the env,
#      then shows the rendered native files and ASSERTS the outcome:
#        - each CLI got the server, in its own native shape;
#        - the instruction fragment landed in CLAUDE.md and AGENTS.md;
#        - the resolved token is in the rendered NATIVE configs (their formats
#          hold plaintext) while the manifest still holds only the placeholder.
#
# It exits nonzero and prints FAIL on any mismatch, so it is safe to run
# unattended. Self-contained: isolated temp HOME, nothing touches your real
# config. Set DEMO_PAUSE=2.5 for a paced screen recording (asciinema).
#
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=..., or a built
# target/release/agentstack in this repo) and python3.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd -P)"

# ── binary resolution: AGENTSTACK_BIN, else PATH, else this repo's release build
AS="${AGENTSTACK_BIN:-}"
if [[ -z "$AS" ]]; then
  if command -v agentstack >/dev/null 2>&1; then
    AS="$(command -v agentstack)"
  else
    # walk up from this script looking for target/release/agentstack
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
export AGENTSTACK_HOME="$SBX/home"
export HOME="$SBX/fakehome"
mkdir -p "$AGENTSTACK_HOME" "$HOME"
trap 'rm -rf "$SBX"' EXIT

# clone the committed bundle into the sandbox and work there
PROJECT="$SBX/project"
mkdir -p "$PROJECT"
cp -R "$HERE/bundle/." "$PROJECT/"
cd "$PROJECT"

# the (fake) token this machine resolves; real value never touches the manifest
export GITHUB_TOKEN="ghp-demo-FAKE-not-a-real-secret-0000"

printf '\033[1;36m  agentstack — one manifest, every CLI\033[0m\n'

say "One file, committed to the repo, is the whole team's agent setup:"
run "cat .agentstack/agentstack.toml"
sed 's/^/  /' .agentstack/agentstack.toml
note "It targets three CLIs, declares one MCP server, one instruction fragment,"
note "and references the token only as \${GITHUB_TOKEN} — no secret value here."

say "Prove the portable artifact is secret-free before we render anything:"
run "grep GITHUB_TOKEN .agentstack/agentstack.toml"
grep -n "GITHUB_TOKEN" .agentstack/agentstack.toml | sed 's/^/  /'
if grep -q '${GITHUB_TOKEN}' .agentstack/agentstack.toml \
   && ! grep -q "$GITHUB_TOKEN" .agentstack/agentstack.toml; then
  ok "the manifest holds the \${GITHUB_TOKEN} placeholder, not the value"
else
  bad "the manifest should hold only the placeholder, never the resolved token"
fi

"$AS" lock --manifest-dir "$PROJECT" >/dev/null

say "Preview the render — one manifest, three CLIs' native configs, three paths:"
run "agentstack apply --scope project        # read-only; writes nothing"
"$AS" apply --manifest-dir "$PROJECT" --scope project 2>&1 | sed 's/^/  /'
note "The preview MASKS the secret as \${GITHUB_TOKEN} in every diff."

say "Now apply for real (token resolved from this machine's environment):"
run "GITHUB_TOKEN=… agentstack apply --scope project --write"
"$AS" apply --manifest-dir "$PROJECT" --scope project --write >/dev/null 2>&1
note "done — here is what landed on disk:"
run "tree of rendered native files"
find . -type f -not -path './.agentstack/*' | sort | sed 's#^\./#  #'

say "Same server, three native shapes — Claude tags it, Codex sub-tables it, Cursor infers it:"
run "cat .mcp.json .codex/config.toml .cursor/mcp.json"
for f in .mcp.json .codex/config.toml .cursor/mcp.json; do
  printf '  \033[1m%s\033[0m\n' "$f"
  sed 's/^/    /' "$f"
done

say "The story on secrets, honestly:"
note "the MANIFEST never holds the value; the rendered NATIVE configs DO — those"
note "formats store plaintext, so the resolved token is now on disk in each one."

# ── assertions ───────────────────────────────────────────────────────────────
printf '\n\033[1mAsserting the outcome:\033[0m\n'

# each CLI's native config exists at its own path
for f in .mcp.json .codex/config.toml .cursor/mcp.json; do
  if [ -f "$f" ]; then ok "rendered $f"; else bad "missing $f"; fi
done

# one manifest fanned out: every native config names the same server
for f in .mcp.json .codex/config.toml .cursor/mcp.json; do
  if grep -q "github" "$f"; then ok "$f declares the github server"; else bad "$f is missing the server"; fi
done

# the instruction fragment compiled into both native instruction files
if grep -q "formatter and linter" CLAUDE.md; then
  ok "the instruction fragment compiled into CLAUDE.md (Claude Code)"
else
  bad "the instruction fragment did not reach CLAUDE.md"
fi
if grep -q "formatter and linter" AGENTS.md; then
  ok "the instruction fragment compiled into AGENTS.md (Codex + Cursor)"
else
  bad "the instruction fragment did not reach AGENTS.md"
fi

# honest secret story: value in native configs, never in the manifest
for f in .mcp.json .codex/config.toml .cursor/mcp.json; do
  if grep -q "$GITHUB_TOKEN" "$f"; then
    ok "$f holds the resolved token (native formats store plaintext)"
  else
    bad "$f did not get the resolved token"
  fi
done
if ! grep -q "$GITHUB_TOKEN" .agentstack/agentstack.toml \
   && ! grep -rq "$GITHUB_TOKEN" agentstack.lock 2>/dev/null; then
  ok "the manifest and lockfile never hold the resolved token"
else
  bad "the manifest/lockfile leaked the resolved token"
fi

say "One manifest in. Three CLIs configured, in sync, in their own formats. Secret resolved per-machine."
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
