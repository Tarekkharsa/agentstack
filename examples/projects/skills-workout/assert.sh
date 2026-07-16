#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# skills-workout — one skill set, two delivery paths, identical bytes.
#
#   AgentStack can put a skill in front of an agent two ways:
#     Path A (static render): `agentstack use <profile> --write` symlinks the
#            profile's skills into the CLI's native skills dir (.claude/skills).
#     Path B (zero-files lease): an agent connected to `agentstack mcp` opens a
#            profile lease and pulls the same skills into context on demand —
#            nothing is written to disk.
#
#   This proof runs BOTH against the same manifest and asserts they deliver the
#   SAME skills with byte-identical bodies, that each path respects the profile
#   fence, that static render prunes cleanly without ever clobbering a hand-made
#   skill dir, and that the lease refuses a skill outside the profile.
#
# Self-contained: isolated AGENTSTACK_HOME + HOME, nothing touches real config.
# Exits nonzero and prints FAIL on any mismatch, so it doubles as a CI check.
# Requires: `agentstack` (or AGENTSTACK_BIN=…, or a built target/release build)
# and python3.
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
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$*"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=$((FAIL + 1)); }
skip() { printf '  \033[33mSKIP\033[0m %s\n' "$*"; }
say()  { printf '\n\033[1;35m▎ %s\033[0m\n' "$*"; }

# ── isolated sandbox — redirects the ENTIRE machine state tree ───────────────
SBX="$(mktemp -d)"
export AGENTSTACK_HOME="$SBX/home"
export HOME="$SBX/fakehome"
mkdir -p "$AGENTSTACK_HOME" "$HOME"
trap 'rm -rf "$SBX"' EXIT

PROJECT="$SBX/project"
mkdir -p "$PROJECT"
cp -R "$HERE/bundle/." "$PROJECT/"
cd "$PROJECT"

printf '\033[1;36m  skills-workout — static render vs zero-files lease, identical bytes\033[0m\n'

# ── seed the central library with the two profile-referenced library skills ──
say "Seed the isolated central library (sql-review, incident-runbook):"
"$AS" lib add sql-review       --path "$PROJECT/lib-sources/sql-review"       --write >/dev/null
"$AS" lib add incident-runbook --path "$PROJECT/lib-sources/incident-runbook" --write >/dev/null
if "$AS" lib list 2>&1 | grep -q 'sql-review' && "$AS" lib list 2>&1 | grep -q 'incident-runbook'; then
  ok "library holds sql-review + incident-runbook"
else
  bad "library seeding failed"
fi

# Source-of-truth SKILL.md bodies we will compare both paths against.
SRC_API="$PROJECT/.agentstack/skills/api-conventions/SKILL.md"
SRC_REL="$PROJECT/.agentstack/skills/release-checklist/SKILL.md"
SRC_SQL="$AGENTSTACK_HOME/lib/skills/sql-review/SKILL.md"

# ─────────────────────────────────────────────────────────────────────────────
say "PATH A — static render via 'use <profile> --write'"
# ─────────────────────────────────────────────────────────────────────────────
"$AS" lock >/dev/null
"$AS" use docs --scope project --write >/dev/null

# docs = api-conventions (inline) + sql-review (library), and EXACTLY those two.
A_DOCS="$(cd .claude/skills && ls -1 | sort | tr '\n' ' ' | sed 's/ *$//')"
if [[ "$A_DOCS" == "api-conventions sql-review" ]]; then
  ok "docs render materialized exactly {api-conventions, sql-review}"
else
  bad "docs render set was {$A_DOCS}, expected {api-conventions sql-review}"
fi

# they are symlinks (materialized, not copied)
if [[ -L .claude/skills/api-conventions && -L .claude/skills/sql-review ]]; then
  ok "rendered skills are symlinks into their sources"
else
  bad "rendered skills are not symlinks"
fi

# bodies (followed through the symlink) match their sources byte-for-byte
if diff -q .claude/skills/api-conventions/SKILL.md "$SRC_API" >/dev/null; then
  ok "rendered api-conventions body == inline source"
else
  bad "rendered api-conventions body != inline source"
fi
if diff -q .claude/skills/sql-review/SKILL.md "$SRC_SQL" >/dev/null; then
  ok "rendered sql-review body == library source"
else
  bad "rendered sql-review body != library source"
fi

# Capture Path A's rendered bytes now, before the `all` render prunes them.
A_API_BODY="$(cat .claude/skills/api-conventions/SKILL.md)"
A_SQL_BODY="$(cat .claude/skills/sql-review/SKILL.md)"

# ── never-clobber: a hand-made, unmanaged skill dir must survive a re-render ──
mkdir -p .claude/skills/handmade-local
echo "hand-made, not managed by agentstack" > .claude/skills/handmade-local/SKILL.md

"$AS" use all --scope project --write >/dev/null

# all's "*" = the manifest's inline skills only: api-conventions + release-checklist
A_ALL="$(cd .claude/skills && ls -1 | grep -v '^handmade-local$' | sort | tr '\n' ' ' | sed 's/ *$//')"
if [[ "$A_ALL" == "api-conventions release-checklist" ]]; then
  ok "'all' (\"*\") rendered exactly {api-conventions, release-checklist} — pruned sql-review, added release-checklist"
else
  bad "'all' render set was {$A_ALL}, expected {api-conventions release-checklist}"
fi

# "*" does NOT sweep in library skills — a documented semantic, asserted here.
if [[ ! -e .claude/skills/sql-review && ! -e .claude/skills/incident-runbook ]]; then
  ok "\"*\" expands to manifest skills only — no library skills (sql-review/incident-runbook) pulled in"
else
  bad "\"*\" unexpectedly pulled a library skill into the render"
fi

# the hand-made dir is untouched — pruning never clobbers what it did not create
if [[ -d .claude/skills/handmade-local ]] \
   && grep -q "hand-made" .claude/skills/handmade-local/SKILL.md; then
  ok "hand-made unmanaged skill dir survived the re-render (never-clobber)"
else
  bad "hand-made unmanaged skill dir was clobbered"
fi

# ─────────────────────────────────────────────────────────────────────────────
say "PATH B — zero-files lease via 'agentstack mcp'"
# ─────────────────────────────────────────────────────────────────────────────
echo y | "$AS" trust . >/dev/null

OUT="$SBX/leaseout"
python3 "$HERE/lease_client.py" "$AS" "$PROJECT" "$OUT"

# the lease opened on the docs profile and wrote no native files
if grep -q '"opened": "docs"' "$OUT/opened.json" \
   && grep -q '"native_files_written": false' "$OUT/opened.json"; then
  ok "lease opened on 'docs' and wrote no native files"
else
  bad "lease_open did not report {opened: docs, native_files_written: false}"
fi

# discovery is fenced: EXACTLY docs' two skills + the built-in using-agentstack manual
LOADABLE="$(sort "$OUT/loadable.txt" | tr '\n' ' ' | sed 's/ *$//')"
if [[ "$LOADABLE" == "api-conventions sql-review using-agentstack" ]]; then
  ok "list_loadable fenced to {api-conventions, sql-review} + using-agentstack manual"
else
  bad "list_loadable returned {$LOADABLE}, expected the docs skills + using-agentstack"
fi

# each in-profile load returned the skill's SKILL.md bytes; check origin routing
if diff -q "$OUT/loaded-api-conventions.txt" "$SRC_API" >/dev/null; then
  ok "loaded api-conventions body == inline source"
else
  bad "loaded api-conventions body != inline source"
fi
if diff -q "$OUT/loaded-sql-review.txt" "$SRC_SQL" >/dev/null; then
  ok "loaded sql-review body == library source"
else
  bad "loaded sql-review body != library source"
fi
if [[ "$(cat "$OUT/loaded-api-conventions.origin")" == "manifest" \
   && "$(cat "$OUT/loaded-sql-review.origin")" == "library" ]]; then
  ok "load reported correct origins (api-conventions=manifest, sql-review=library)"
else
  bad "load reported wrong origins"
fi

# the fence holds: a real manifest skill outside the profile is refused
if grep -qi 'not loadable' "$OUT/refused.txt"; then
  ok "lease refused release-checklist (real skill, not in docs profile) — fence holds"
else
  bad "lease did not refuse the out-of-profile skill; got: $(cat "$OUT/refused.txt")"
fi

# the load trail records what was loaded, with the agent's stated reasons
if grep -q '"name": "api-conventions"' "$OUT/status.json" \
   && grep -q '"reason": "design a new endpoint"' "$OUT/status.json" \
   && grep -q '"name": "sql-review"' "$OUT/status.json" \
   && grep -q '"reason": "review a migration"' "$OUT/status.json"; then
  ok "lease_status recorded the load trail (names + reasons)"
else
  bad "lease_status did not record the expected load trail"
fi
# the refused load never entered the trail
if ! grep -q '"name": "release-checklist"' "$OUT/status.json"; then
  ok "the refused load left no trace in the trail"
else
  bad "the refused load leaked into the load trail"
fi

# closing the lease needs no native restore (nothing was ever written)
if grep -q '"closed": "docs"' "$OUT/close.json" \
   && grep -q '"native_restore_needed": false' "$OUT/close.json"; then
  ok "lease closed cleanly, no native restore needed"
else
  bad "lease_close did not report a clean {closed: docs, native_restore_needed: false}"
fi

# ─────────────────────────────────────────────────────────────────────────────
say "THE POINT — both paths delivered byte-identical skill bodies"
# ─────────────────────────────────────────────────────────────────────────────
if [[ "$A_API_BODY" == "$(cat "$OUT/loaded-api-conventions.txt")" ]]; then
  ok "api-conventions: static-rendered bytes == lease-loaded bytes"
else
  bad "api-conventions: static render and lease disagree"
fi
if [[ "$A_SQL_BODY" == "$(cat "$OUT/loaded-sql-review.txt")" ]]; then
  ok "sql-review: static-rendered bytes == lease-loaded bytes"
else
  bad "sql-review: static render and lease disagree"
fi

printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
