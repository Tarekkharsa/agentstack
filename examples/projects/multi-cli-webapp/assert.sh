#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — one setup, three CLIs (Claude Code + Codex + Cursor).
#
# A small storefront web app whose team ships ONE agent setup across three CLIs.
# The committed manifest declares one HTTP MCP server (secret as a ${REF}), one
# house-rules instruction fragment, and a profile that pulls one skill BY NAME
# from the central library. This script seeds an isolated library, activates the
# profile, and ASSERTS the honest outcome on disk:
#
#   1. the server lands in .mcp.json / .codex/config.toml / .cursor/mcp.json,
#      each in its native shape, with the resolved token;
#   2. the house-rules marker is inside the managed region of CLAUDE.md AND
#      AGENTS.md;
#   3. the library skill materialized as a symlink into .claude/skills AND
#      .agents/skills with the right SKILL.md;
#   4. the manifest and lockfile never hold the resolved token.
#
# It also probes the Cursor gap: Cursor's adapter supports MCP but NOT
# instructions and NOT skills. Cursor is a declared target, so the probe
# asserts AgentStack warns that the instruction/skill can't reach Cursor
# (before v0.15.0 it was silently dropped). The script captures the warning
# verbatim.
#
# Isolated: its own AGENTSTACK_HOME and HOME under a temp dir; nothing touches
# your real config. Exits nonzero on any FAIL.
#
# Requires: agentstack on PATH (or AGENTSTACK_BIN=..., or a built
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
SKIP=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$*"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=$((FAIL + 1)); }
skip() { printf '  \033[33mSKIP\033[0m %s\n' "$*"; SKIP=$((SKIP + 1)); }
say()  { printf '\n\033[1;35m▎ %s\033[0m\n' "$*"; }
nocolor() { sed 's/\x1b\[[0-9;]*m//g'; }

# ── isolated sandbox (nothing touches your real config) ──────────────────────
SBX="$(mktemp -d)"
export AGENTSTACK_HOME="$SBX/home"
export HOME="$SBX/fakehome"
mkdir -p "$AGENTSTACK_HOME" "$HOME"
trap 'rm -rf "$SBX"' EXIT

PROJECT="$SBX/project"
mkdir -p "$PROJECT"
cp -R "$HERE/bundle/." "$PROJECT/"
cd "$PROJECT"

# The (fake) token this machine resolves. The real value never touches the
# manifest — it lives only in the environment here.
export WEBAPP_API_TOKEN="wapi-demo-FAKE-not-a-real-secret-0000"

printf '\033[1;36m  agentstack — one setup, three CLIs (multi-cli-webapp)\033[0m\n'

# ── 0. the portable artifact is secret-free ──────────────────────────────────
say "The committed manifest holds the placeholder, never the token value:"
if grep -q '${WEBAPP_API_TOKEN}' .agentstack/agentstack.toml \
   && ! grep -q "$WEBAPP_API_TOKEN" .agentstack/agentstack.toml; then
  ok "manifest holds \${WEBAPP_API_TOKEN}, not the resolved value"
else
  bad "manifest should hold only the placeholder"
fi

# ── 1. seed the central library, then lock ───────────────────────────────────
say "Seed the isolated central library with the team's skill, then lock:"
"$AS" lib add ./team-library/api-conventions --name api-conventions --write >/dev/null 2>&1
if "$AS" lib list 2>&1 | nocolor | grep -q "api-conventions"; then
  ok "api-conventions is in the central library (referenced by name from the profile)"
else
  bad "lib add did not register api-conventions"
fi
"$AS" lock >/dev/null 2>&1
ok "locked the manifest (skill + server + instruction pinned)"

# ── 2. activate the profile + render instructions ────────────────────────────
say "Activate the profile (servers + skills), then render instructions:"
"$AS" use team --scope project --write >/dev/null 2>&1
"$AS" apply --scope project --write >/dev/null 2>&1
ok "use team + apply completed"

# ── 3. one server, three native shapes, each with the resolved token ─────────
say "The same server, compiled into three native formats:"

# Claude Code: JSON with an explicit transport "type": "http"
if [ -f .mcp.json ] \
   && grep -q '"type": *"http"' .mcp.json \
   && grep -q "$WEBAPP_API_TOKEN" .mcp.json; then
  ok ".mcp.json carries the server tagged \"type\":\"http\" with the resolved token"
else
  bad ".mcp.json missing, untagged, or missing the token"
fi

# Codex: TOML with a nested [mcp_servers.<name>.http_headers] sub-table
if [ -f .codex/config.toml ] \
   && grep -q '\[mcp_servers.storefront-api.http_headers\]' .codex/config.toml \
   && grep -q "$WEBAPP_API_TOKEN" .codex/config.toml; then
  ok ".codex/config.toml carries the server with an http_headers sub-table + token"
else
  bad ".codex/config.toml missing, wrong shape, or missing the token"
fi

# Cursor: JSON, transport inferred (NO "type" key), token present
if [ -f .cursor/mcp.json ] \
   && ! grep -q '"type"' .cursor/mcp.json \
   && grep -q "$WEBAPP_API_TOKEN" .cursor/mcp.json; then
  ok ".cursor/mcp.json carries the server with an inferred transport (no type tag) + token"
else
  bad ".cursor/mcp.json missing, unexpectedly tagged, or missing the token"
fi

# ── 4. the house-rules marker landed in both managed instruction files ───────
say "The house-rules fragment compiled into the managed region of both files:"
marker="STOREFRONT-HOUSE-RULE-A7"
for f in CLAUDE.md AGENTS.md; do
  if [ -f "$f" ] && grep -q '<!-- agentstack:start -->' "$f" && grep -q "$marker" "$f"; then
    ok "$f has the house-rules marker inside the managed region"
  else
    bad "$f is missing the managed region or the house-rules marker"
  fi
done

# ── 5. the library skill materialized as symlinks with the right content ─────
say "The library skill materialized (symlinked) into each skills-supporting CLI:"
skill_marker="STOREFRONT-SKILL-CONV-Q3"
for d in .claude/skills .agents/skills; do
  link="$d/api-conventions"
  if [ -L "$link" ] && grep -q "$skill_marker" "$link/SKILL.md" 2>/dev/null; then
    ok "$link is a symlink and its SKILL.md carries the right content"
  else
    bad "$link is missing, not a symlink, or has the wrong SKILL.md"
  fi
done

# ── 6. the secret never entered the portable artifact ────────────────────────
say "The resolved token is in the native configs (their formats store plaintext), never in the manifest/lock:"
if ! grep -q "$WEBAPP_API_TOKEN" .agentstack/agentstack.toml \
   && ! grep -rq "$WEBAPP_API_TOKEN" .agentstack/agentstack.lock 2>/dev/null; then
  ok "manifest and lockfile never hold the resolved token"
else
  bad "manifest/lockfile leaked the resolved token"
fi

# ── 7. THE CURSOR GAP ────────────────────────────────────────────────────────
# Cursor supports MCP but NOT instructions and NOT skills. It is a declared
# target, and the instruction fragment's targets default to "*" (all three
# CLIs). What does the user experience? First the disk reality:
say "The Cursor gap — Cursor is a declared target with no instructions/skills support:"
if [ ! -e .cursor/skills ] && [ ! -f .cursor/CLAUDE.md ] && [ ! -f .cursor/AGENTS.md ]; then
  ok "Cursor got the MCP server but NO instruction file and NO skills dir (structural)"
else
  bad "unexpected Cursor instruction/skills artifacts appeared"
fi

# Now: does AgentStack TELL the user the house-rules / api-conventions can't
# reach Cursor, or silently drop them? Collect the surfaces a user would look
# at and search for any warning that names Cursor alongside the dropped content.
apply_out="$("$AS" apply --manifest-dir "$PROJECT" --scope project --target cursor 2>&1 | nocolor)"
doctor_out="$("$AS" doctor 2>&1 | nocolor)"
explain_instr="$("$AS" explain house-rules 2>&1 | nocolor)"
explain_skill="$("$AS" explain api-conventions 2>&1 | nocolor)"
combined="$apply_out
$doctor_out
$explain_instr
$explain_skill"

# A genuine warning pairs "cursor" with drop/skip/unsupported/no-instructions —
# in either order (explain says "not supported by: … Cursor"; apply prints the
# note inside Cursor's own block). Once a silent gap (issue #12), now asserted.
drop_phrases='no instruction|no skill|unsupported|not supported|skipp|dropp|can.?t receive|will not'
if printf '%s' "$combined" | grep -iqE "cursor.*($drop_phrases)|($drop_phrases).*cursor" \
   || printf '%s' "$apply_out" | grep -iqE "$drop_phrases"; then
  ok "AgentStack warns that Cursor cannot receive the instruction/skill"
else
  bad "regressed (issue #12): no surface warns that Cursor's instruction+skill are dropped"
  printf '  \033[33m→ apply --target cursor output:\033[0m\n'
  printf '%s\n' "$apply_out" | sed 's/^/      /'
  printf '  \033[33m→ explain house-rules Targets line:\033[0m\n'
  printf '%s\n' "$explain_instr" | grep -i 'target' | sed 's/^/      /'
fi

# ── summary ──────────────────────────────────────────────────────────────────
printf '\n\033[1mSummary:\033[0m %d passed, %d failed' "$PASS" "$FAIL"
[ "$SKIP" -gt 0 ] && printf ', %d skipped' "$SKIP"
printf '\n'
[ "$FAIL" -eq 0 ] || exit 1
