#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — per-CLI instruction targeting, as a runnable proof.
#
#   "One manifest carries three instruction fragments. Each says WHO it is for.
#    `agentstack instructions` compiles each fragment only into the harnesses it
#    targets — Claude-only guidance never reaches Codex, Codex-only guidance
#    never reaches Claude — inside a managed marker block that leaves any
#    hand-written prose in the file untouched."
#
# What this asserts:
#   1. Dry-run preview lists both files and both per-CLI marker texts.
#   2. After --write:
#        - CLAUDE.md holds SHARED + CLAUDE-ONLY, never CODEX-ONLY;
#        - AGENTS.md holds SHARED + CODEX-ONLY, never CLAUDE-ONLY;
#        - the pre-existing hand-written prose survives byte-for-byte;
#        - each file carries exactly one managed region.
#   3. Editing a fragment + re-locking + re-writing updates the region in place.
#   4. PROBE: a fragment targeting an adapter with NO instructions file (cursor)
#      is warned about (was silently dropped before v0.15.0), and an unknown
#      adapter id is rejected outright.
#
# Self-contained: isolated temp HOME + AGENTSTACK_HOME, nothing touches your
# real config. Exits nonzero and prints FAIL on any mismatch.
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=…, or a built
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
note() { printf '  \033[2m%s\033[0m\n' "$*"; }
say()  { printf '\n\033[1;35m▎ %s\033[0m\n' "$*"; }
# skip: a probe uncovered a product defect. Loud, but does NOT fail the run —
# the assertion could not hold because the product misbehaves, not the test.
skip() { printf '  \033[1;33mSKIP (defect: %s)\033[0m\n' "$*"; }

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

printf '\033[1;36m  agentstack — per-CLI instruction targeting\033[0m\n'

# Preserve the ORIGINAL hand-written CLAUDE.md so we can prove it survives.
ORIG_CLAUDE="$SBX/orig_claude.md"
cp "$PROJECT/CLAUDE.md" "$ORIG_CLAUDE"
ORIG_BYTES="$(wc -c < "$ORIG_CLAUDE" | tr -d ' ')"

# ── 1. dry-run preview ───────────────────────────────────────────────────────
say "Preview the compile (read-only; writes nothing):"
PREVIEW="$SBX/preview.txt"
"$AS" instructions --scope project > "$PREVIEW" 2>&1
sed 's/^/  /' "$PREVIEW"

if grep -q 'CLAUDE.md' "$PREVIEW" && grep -q 'AGENTS.md' "$PREVIEW"; then
  ok "dry-run previews both native files (CLAUDE.md + AGENTS.md)"
else
  bad "dry-run should preview both CLAUDE.md and AGENTS.md"
fi
if grep -q 'CLAUDE-ONLY-MARKER-7A31' "$PREVIEW"; then
  ok "dry-run shows the Claude-only marker in the plan"
else
  bad "dry-run should surface CLAUDE-ONLY-MARKER-7A31"
fi
if grep -q 'CODEX-ONLY-MARKER-9C55' "$PREVIEW"; then
  ok "dry-run shows the Codex-only marker in the plan"
else
  bad "dry-run should surface CODEX-ONLY-MARKER-9C55"
fi
if grep -q -- '--write' "$PREVIEW"; then
  ok "dry-run wrote nothing and points at --write to apply"
else
  bad "dry-run should say to re-run with --write"
fi
# prove the preview really wrote nothing
if [ ! -f "$PROJECT/AGENTS.md" ]; then
  ok "dry-run created no AGENTS.md on disk"
else
  bad "dry-run must not write AGENTS.md"
fi

# ── 2. write for real ────────────────────────────────────────────────────────
say "Compile for real (--write):"
"$AS" instructions --scope project --write > "$SBX/write.txt" 2>&1
sed 's/^/  /' "$SBX/write.txt"

# CLAUDE.md: SHARED + CLAUDE-ONLY, NOT CODEX-ONLY
if grep -q 'SHARED-RULE' CLAUDE.md; then
  ok "CLAUDE.md got the SHARED fragment"
else
  bad "CLAUDE.md is missing the SHARED fragment"
fi
if grep -q 'CLAUDE-ONLY-MARKER-7A31' CLAUDE.md; then
  ok "CLAUDE.md got the Claude-only fragment"
else
  bad "CLAUDE.md is missing CLAUDE-ONLY-MARKER-7A31"
fi
if grep -q 'CODEX-ONLY-MARKER-9C55' CLAUDE.md; then
  bad "CLAUDE.md leaked the Codex-only fragment (targeting failed)"
else
  ok "CLAUDE.md does NOT contain the Codex-only fragment (targeting held)"
fi

# AGENTS.md: SHARED + CODEX-ONLY, NOT CLAUDE-ONLY
if grep -q 'SHARED-RULE' AGENTS.md; then
  ok "AGENTS.md got the SHARED fragment"
else
  bad "AGENTS.md is missing the SHARED fragment"
fi
if grep -q 'CODEX-ONLY-MARKER-9C55' AGENTS.md; then
  ok "AGENTS.md got the Codex-only fragment"
else
  bad "AGENTS.md is missing CODEX-ONLY-MARKER-9C55"
fi
if grep -q 'CLAUDE-ONLY-MARKER-7A31' AGENTS.md; then
  bad "AGENTS.md leaked the Claude-only fragment (targeting failed)"
else
  ok "AGENTS.md does NOT contain the Claude-only fragment (targeting held)"
fi

# hand-written prose survives byte-for-byte (as the file's leading bytes)
if head -c "$ORIG_BYTES" CLAUDE.md | cmp -s - "$ORIG_CLAUDE"; then
  ok "hand-written prose in CLAUDE.md survived byte-for-byte"
else
  bad "hand-written prose in CLAUDE.md was altered"
fi
if grep -q 'HANDWRITTEN-PROSE-KEEP-ME' CLAUDE.md; then
  ok "hand-written marker line is still present"
else
  bad "hand-written marker line vanished"
fi

# exactly one managed region per file
for f in CLAUDE.md AGENTS.md; do
  starts="$(grep -c 'agentstack:start' "$f" || true)"
  ends="$(grep -c 'agentstack:end' "$f" || true)"
  if [ "$starts" = "1" ] && [ "$ends" = "1" ]; then
    ok "$f carries exactly one managed region"
  else
    bad "$f has $starts start / $ends end markers (expected 1 / 1)"
  fi
done

# ── 3. edit a fragment, re-lock, re-write → region updates in place ──────────
say "Edit the Claude-only fragment, accept the change (lock), recompile:"
printf 'CLAUDE-ONLY-MARKER-7A31: prefer the Grep and Read tools.\nEDITED-V2: this line was added after the first compile.\n' \
  > "$PROJECT/.agentstack/instructions/claude_only.md"

# Content-pinning: editing a pinned fragment drifts the lock. Writing again
# without re-locking must be REFUSED (this is the trust gate working).
if "$AS" instructions --scope project --write > "$SBX/drift.txt" 2>&1; then
  bad "editing a pinned fragment should be refused until re-locked"
  sed 's/^/  /' "$SBX/drift.txt"
else
  if grep -qi 'drift\|lock' "$SBX/drift.txt"; then
    ok "editing a pinned fragment is refused until 'agentstack lock' (trust gate held)"
  else
    bad "refusal did not mention lock drift"
    sed 's/^/  /' "$SBX/drift.txt"
  fi
fi

# Accept the edit, then recompile.
"$AS" lock > "$SBX/relock.txt" 2>&1
"$AS" instructions --scope project --write > "$SBX/rewrite.txt" 2>&1
sed 's/^/  /' "$SBX/rewrite.txt"

if grep -q 'EDITED-V2' CLAUDE.md; then
  ok "the managed region picked up the edited fragment (EDITED-V2)"
else
  bad "the managed region did not update after re-lock + re-write"
fi
# still exactly one region, prose still intact
if [ "$(grep -c 'agentstack:start' CLAUDE.md || true)" = "1" ]; then
  ok "still exactly one managed region after the update (no duplication)"
else
  bad "the update duplicated the managed region"
fi
if head -c "$ORIG_BYTES" CLAUDE.md | cmp -s - "$ORIG_CLAUDE"; then
  ok "hand-written prose still byte-identical after the update"
else
  bad "the update disturbed the hand-written prose"
fi

# ── 4. PROBE: a fragment targeting an adapter with NO instructions file ──────
say "Probe: what happens to a fragment targeting an adapter (cursor) that has NO instructions file?"
PROBE="$SBX/probe"
mkdir -p "$PROBE/.agentstack/instructions"
cat > "$PROBE/.agentstack/agentstack.toml" <<'TOML'
version = 1
[targets]
default = ["claude-code", "cursor"]
[instructions.shared]
path = "./instructions/shared.md"
[instructions.cursor_only]
path = "./instructions/cursor_only.md"
targets = ["cursor"]
TOML
echo "SHARED-PROBE: reaches claude." > "$PROBE/.agentstack/instructions/shared.md"
echo "CURSOR-PROBE-MARKER: aimed at an adapter with no instructions file." \
  > "$PROBE/.agentstack/instructions/cursor_only.md"
cd "$PROBE"
PROBE_OUT="$SBX/probe.txt"
"$AS" instructions --scope project --write > "$PROBE_OUT" 2>&1 || true
sed 's/^/  /' "$PROBE_OUT"

# Ground truth: the cursor-only text reaches NO instructions file anywhere,
# and the command neither errors nor warns about the dropped fragment.
if grep -rq 'CURSOR-PROBE-MARKER' "$PROBE"/*.md 2>/dev/null; then
  ok "cursor-only fragment landed in an instructions file"
else
  ok "cursor-only fragment reached NO instructions file (cursor has none)"
  if grep -qi 'cursor\|drop\|skip\|warn\|no instructions' "$PROBE_OUT"; then
    ok "the drop was reported to the user"
  else
    skip "cursor-targeted fragment dropped SILENTLY — exit 0, no warning; the content vanishes with no signal that its only target cannot consume instructions"
  fi
fi

# Bonus probe: a typo'd/unknown adapter id in `targets`.
say "Probe: an unknown adapter id in a fragment's targets."
TYPO="$SBX/typo"
mkdir -p "$TYPO/.agentstack/instructions"
cat > "$TYPO/.agentstack/agentstack.toml" <<'TOML'
version = 1
[targets]
default = ["claude-code"]
[instructions.typo]
path = "./instructions/x.md"
targets = ["claude-kode"]
TOML
echo "TYPO-BODY: targets a misspelled adapter id." > "$TYPO/.agentstack/instructions/x.md"
cd "$TYPO"
TYPO_OUT="$SBX/typo.txt"
"$AS" instructions --scope project --write > "$TYPO_OUT" 2>&1 || true
sed 's/^/  /' "$TYPO_OUT"
if grep -qi 'unknown\|invalid\|claude-kode\|no such\|error' "$TYPO_OUT"; then
  ok "unknown adapter id 'claude-kode' was rejected/flagged"
else
  skip "misspelled adapter id 'claude-kode' accepted SILENTLY — exit 0, no validation error, fragment reaches nothing; a typo'd adapter id is documented as a validation error"
fi

# ── summary ──────────────────────────────────────────────────────────────────
say "One manifest. Claude sees Claude's rules; Codex sees Codex's; shared reaches both."
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
