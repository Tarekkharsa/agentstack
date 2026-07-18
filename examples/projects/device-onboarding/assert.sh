#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — device onboarding matrix: will `init → apply → use` work on
# REAL machines, whatever CLIs and configs they already have?
#
# Every scenario is a fresh fake device: stripped PATH (a stub `claude` only),
# synthetic HOME with per-scenario native configs, isolated AGENTSTACK_HOME.
# Nothing touches your real machine. Asserting: PASS/FAIL, nonzero exit on
# any failure — CI-grade.
#
#   A. CLI-presence matrix: zero CLIs (honest fallback), one CLI, three CLIs
#      with servers in three native formats — imports counted, an inline
#      bearer token lifted to a ${REF} (never plaintext in the manifest),
#      blocked-then-resolved apply, cross-CLI fan-out.
#   B. Config safety: conflicting server definitions surfaced; re-init never
#      clobbers a hand-edited manifest; hand-written .mcp.json entries and
#      CLAUDE.md prose survive apply AND restore; pruning a de-manifested
#      server never touches hand-written entries; apply is idempotent;
#      doctor --ci is green after a write; restore reverts.
#   C. Environment quirks: paths with spaces and unicode (incl. lock → trust
#      → locked --plan), legacy root-manifest layout, non-git projects, an
#      AGENTSTACK_HOME containing spaces (incl. guard denial through it).
#
# This example's first round found four gaps; all four are now fixed and
# asserted here (see FINDINGS.md "Device-onboarding round"): the default-scope
# decision (A3), project-scope pending-removal warnings (via doctor), and — in
# section D — subdirectory walk-up discovery and adopt on hand-EDITED values.
#
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=..., or a built
# target/{release,debug}/agentstack in this repo), git, python3.
# ─────────────────────────────────────────────────────────────────────────────
set -u

HERE="$(cd "$(dirname "$0")" && pwd -P)"

# ── binary resolution: AGENTSTACK_BIN, else PATH, else this repo's build ─────
AS="${AGENTSTACK_BIN:-}"
if [[ -z "$AS" ]]; then
  if command -v agentstack >/dev/null 2>&1; then
    AS="$(command -v agentstack)"
  else
    d="$HERE"
    while [[ "$d" != "/" ]]; do
      for prof in release debug; do
        if [[ -x "$d/target/$prof/agentstack" ]]; then AS="$d/target/$prof/agentstack"; break 2; fi
      done
      d="$(dirname "$d")"
    done
  fi
fi
if [[ -z "$AS" ]]; then
  echo "could not find agentstack: set AGENTSTACK_BIN, add it to PATH, or run 'cargo build --release'" >&2
  exit 2
fi
[[ "$AS" == */* ]] && AS="$(cd "$(dirname "$AS")" && pwd -P)/$(basename "$AS")" || AS="$(command -v "$AS")"

PASS=0; FAIL=0
ok()  { printf '  \033[32mPASS\033[0m %s\n' "$*"; PASS=$((PASS+1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=$((FAIL+1)); }
hdr() { printf '\n\033[1;36m▎ %s\033[0m\n' "$*"; }

GIT_BIN="$(command -v git)"; PY_BIN="$(command -v python3)"
git() { "$GIT_BIN" "$@"; }
python3() { "$PY_BIN" "$@"; }

ROOT_SBX="$(mktemp -d)"
trap 'rm -rf "$ROOT_SBX"' EXIT
DEV_N=0

device() { # fresh fake device; $1 (optional): project dir name
  DEV_N=$((DEV_N + 1))
  SBX="$ROOT_SBX/dev$DEV_N"; H="$SBX/home"; P="$SBX/${1:-proj}"
  mkdir -p "$H" "$P" "$SBX/bin"
  printf '#!/bin/sh\nexit 0\n' > "$SBX/bin/claude"
  chmod 755 "$SBX/bin/claude"
  export HOME="$H" AGENTSTACK_HOME="$SBX/ashome" PATH="$SBX/bin:/usr/bin:/bin"
  cd "$P"
}
seed_manifest() {
  mkdir -p .agentstack
  printf 'version = 1\n[servers.docs]\ntype = "http"\nurl = "https://docs.example/mcp"\n[targets]\ndefault = ["claude-code"]\n' \
    > .agentstack/agentstack.toml
}

printf '\033[1;36m  agentstack — device onboarding matrix\033[0m\n'

# ═══ A. CLI-presence matrix ══════════════════════════════════════════════════

hdr "A1) zero CLIs — honest fallback, everything still green"
device
rm -f "$SBX/bin/claude"   # truly zero CLIs
OUT=$("$AS" init 2>&1) && ok "init exits 0 on a CLI-less device" || bad "init failed: $OUT"
grep -qi "No supported CLIs detected" <<<"$OUT" && ok "honest zero-CLI message + starter manifest" || bad "detection: $(grep -i detect <<<"$OUT")"
[ -f .agentstack/agentstack.toml ] && ok "starter manifest written" || bad "no manifest"
"$AS" apply --write >/dev/null 2>&1 && ok "apply graceful with zero targets" || bad "apply failed"
"$AS" doctor >/dev/null 2>&1 && ok "doctor exits 0" || bad "doctor failed"

hdr "A2) one CLI, empty config"
device
echo '{}' > "$H/.claude.json"
OUT=$("$AS" init 2>&1)
grep -q "Detected 1" <<<"$OUT" && ok "detects exactly 1 CLI" || bad "$(grep -i detect <<<"$OUT")"
grep -q "Imported 0" <<<"$OUT" && ok "imports 0 from an empty config" || bad "$(grep -i import <<<"$OUT")"
grep -q 'claude-code' .agentstack/agentstack.toml && ok "targets include claude-code" || bad "targets wrong"

hdr "A3) three CLIs, three formats, one inline bearer token"
device
cat > "$H/.claude.json" <<'EOF'
{"mcpServers":{"github":{"type":"http","url":"https://api.githubcopilot.com/mcp/","headers":{"Authorization":"Bearer ghp_FAKE1234567890abcdefFAKE"}},"files":{"type":"stdio","command":"npx","args":["-y","@example/files-mcp"]}}}
EOF
mkdir -p "$H/.codex" "$H/.cursor"
printf '[mcp_servers.linear]\ncommand = "npx"\nargs = ["-y", "@linear/mcp"]\n' > "$H/.codex/config.toml"
echo '{"mcpServers":{}}' > "$H/.cursor/mcp.json"
OUT=$("$AS" init --no-keychain 2>&1)
grep -q "Detected 3" <<<"$OUT" && ok "detects 3 CLIs" || bad "$(grep -i detect <<<"$OUT")"
grep -q "Imported 3" <<<"$OUT" && ok "imports 3 servers across json + toml" || bad "$(grep -i import <<<"$OUT")"
M=.agentstack/agentstack.toml
grep -q 'ghp_FAKE' "$M" && bad "PLAINTEXT TOKEN IN THE MANIFEST" || ok "no plaintext token in the manifest"
grep -q '\${' "$M" && ok "secret lifted to a \${REF}" || bad "no \${REF} in manifest"
"$AS" apply --write >/dev/null 2>&1 && bad "apply exited 0 despite unresolved ref" || ok "apply --write blocks the unresolved ref (nonzero exit)"
REF=$(grep -o '\${[A-Z0-9_]*}' "$M" | head -1 | tr -d '${}')
env "$REF=fake-value" "$AS" apply --write >/dev/null 2>&1 && ok "apply succeeds once the ref resolves via env" || bad "resolved apply failed"
# Default scope is project inside a repo manifest (docs/design/default-scope.md),
# so the fan-out lands in the repo's .mcp.json — never in ~/.claude.json.
grep -q 'linear' .mcp.json && ok "codex-imported server fanned out to the project claude config" || bad "cross-CLI fan-out missing"
grep -q 'linear' "$H/.claude.json" && bad "repo apply leaked into the global claude config" || ok "global claude config untouched by the repo apply"

# ═══ B. Config safety ════════════════════════════════════════════════════════

hdr "B1) conflicting definitions of one server name are surfaced"
device
echo '{"mcpServers":{"api":{"type":"http","url":"https://a.example/mcp"}}}' > "$H/.claude.json"
mkdir -p "$H/.cursor"
echo '{"mcpServers":{"api":{"url":"https://b.example/mcp"}}}' > "$H/.cursor/mcp.json"
OUT=$("$AS" init 2>&1)
grep -qiE "conflict|differs|mismatch" <<<"$OUT" && ok "conflict surfaced, not silently picked" || bad "silent pick: $(grep -o '[ab].example' .agentstack/agentstack.toml | tr '\n' ' ')"

hdr "B2) re-init never clobbers a hand-edited manifest"
device
echo '{"mcpServers":{"github":{"type":"http","url":"https://x.example/mcp"}}}' > "$H/.claude.json"
"$AS" init >/dev/null 2>&1
echo "# my note" >> .agentstack/agentstack.toml
"$AS" init >/dev/null 2>&1
grep -q "# my note" .agentstack/agentstack.toml && ok "hand edit survived re-init" || bad "re-init clobbered the manifest"

hdr "B3) hand-written .mcp.json + CLAUDE.md prose survive apply AND restore"
device
echo '{}' > "$H/.claude.json"
git init -q .
echo '{"mcpServers":{"my-hand-server":{"type":"http","url":"https://hand.example/mcp"}}}' > .mcp.json
printf '# My project\n\nHand-written intro prose.\n' > CLAUDE.md
mkdir -p .agentstack
printf 'version = 1\n[instructions.team]\npath = "./team.md"\n[targets]\ndefault = ["claude-code"]\n' > .agentstack/agentstack.toml
printf 'Team rules from agentstack.\n' > .agentstack/team.md
"$AS" apply --scope project --write >/dev/null 2>&1
grep -q "my-hand-server" .mcp.json && ok "hand-written server survived apply" || bad "hand server gone"
grep -q "Hand-written intro prose" CLAUDE.md && ok "hand prose survived" || bad "prose clobbered"
grep -q "agentstack:start" CLAUDE.md && ok "managed region compiled alongside the prose" || bad "no managed region"
if [ -f .gitignore ] && grep -q "mcp.json" .gitignore; then bad "managed gitignore hid the HAND-written .mcp.json"; else ok "hand-written config never gitignored"; fi
"$AS" restore --last --write >/dev/null 2>&1
grep -q "Hand-written intro prose" CLAUDE.md && ok "restore kept the prose" || bad "restore ate the prose"
grep -q "agentstack:start" CLAUDE.md && bad "restore left the managed region" || ok "restore removed only the managed region"

hdr "B4) manifest is the truth: apply re-renders over a rogue edit"
device
echo '{}' > "$H/.claude.json"
seed_manifest
"$AS" apply --scope project --write >/dev/null 2>&1
python3 -c "import json; d=json.load(open('.mcp.json')); d['mcpServers']['docs']['url']='https://rogue.example/mcp'; json.dump(d,open('.mcp.json','w'))"
"$AS" apply --scope project --write >/dev/null 2>&1
grep -q "docs.example" .mcp.json && ok "manifest's truth re-rendered over the edit" || bad "rogue edit survived apply --write"

hdr "B5) pruning a de-manifested server never touches hand entries"
device
echo '{}' > "$H/.claude.json"
git init -q .
echo '{"mcpServers":{"my-hand-server":{"type":"http","url":"https://hand.example/mcp"}}}' > .mcp.json
mkdir -p .agentstack
printf 'version = 1\n[servers.managed-a]\ntype = "http"\nurl = "https://a.example/mcp"\n[servers.managed-b]\ntype = "http"\nurl = "https://b.example/mcp"\n[targets]\ndefault = ["claude-code"]\n' > .agentstack/agentstack.toml
"$AS" apply --scope project --write >/dev/null 2>&1
python3 - <<'EOF'
text = open(".agentstack/agentstack.toml").read()
open(".agentstack/agentstack.toml","w").write(
    text.replace('[servers.managed-b]\ntype = "http"\nurl = "https://b.example/mcp"\n', ""))
EOF
"$AS" apply --scope project --write >/dev/null 2>&1
grep -q "managed-b" .mcp.json && bad "managed-b not pruned" || ok "de-manifested server pruned"
grep -q "managed-a" .mcp.json && ok "still-manifested server kept" || bad "managed-a lost"
grep -q "my-hand-server" .mcp.json && ok "hand-written server untouched by pruning" || bad "PRUNING ATE THE HAND ENTRY"

hdr "B6) idempotency + doctor --ci + restore round-trip"
device
echo '{"mcpServers":{"docs":{"type":"http","url":"https://docs.example/mcp"}}}' > "$H/.claude.json"
"$AS" init >/dev/null 2>&1 && "$AS" apply --write >/dev/null 2>&1
OUT=$("$AS" apply 2>&1)
grep -qiE "no changes|up to date|0 target" <<<"$OUT" && ok "second apply reports nothing to do" || bad "not idempotent: $(tail -2 <<<"$OUT")"
"$AS" doctor --ci >/dev/null 2>&1 && ok "doctor --ci green after apply" || bad "doctor --ci red"
"$AS" restore --last --write >/dev/null 2>&1
OUT=$("$AS" apply 2>&1)
grep -qiE "to apply|would change" <<<"$OUT" && ok "restore reverted — changes pending again" || bad "restore no-op"

# ═══ C. Environment quirks ═══════════════════════════════════════════════════

hdr "C1) project path with spaces (through lock → trust → locked --plan)"
device "My Projects/app one"
echo '{}' > "$H/.claude.json"
seed_manifest
"$AS" apply --scope project --write >/dev/null 2>&1 && grep -q docs .mcp.json && ok "apply in a spaced path" || bad "apply failed"
"$AS" lock >/dev/null 2>&1 && echo y | "$AS" trust . >/dev/null 2>&1 && ok "lock + trust in a spaced path" || bad "lock/trust failed"
"$AS" run claude-code --locked --plan 2>&1 | grep -q "would proceed" && ok "locked --plan green in a spaced path" || bad "locked plan failed"

hdr "C2) unicode project path"
device "wörk/项目"
echo '{}' > "$H/.claude.json"
seed_manifest
"$AS" apply --scope project --write >/dev/null 2>&1 && grep -q docs .mcp.json && ok "apply in a unicode path" || bad "apply failed"
"$AS" doctor >/dev/null 2>&1 && ok "doctor green in a unicode path" || bad "doctor failed"

hdr "C3) legacy root agentstack.toml (no .agentstack/)"
device "legacy"
echo '{}' > "$H/.claude.json"
printf 'version = 1\n[servers.docs]\ntype = "http"\nurl = "https://docs.example/mcp"\n[targets]\ndefault = ["claude-code"]\n' > agentstack.toml
"$AS" apply --scope project --write >/dev/null 2>&1 && grep -q docs .mcp.json && ok "legacy layout applies" || bad "apply failed"
"$AS" lock >/dev/null 2>&1 && [ -f agentstack.lock ] && ok "lock lands beside the legacy manifest" || bad "lock misplaced"
echo y | "$AS" trust . >/dev/null 2>&1
"$AS" run claude-code --locked --plan 2>&1 | grep -q "would proceed" && ok "locked --plan green on the legacy layout" || bad "legacy locked plan failed"

hdr "C4) non-git project"
device "plain"
echo '{}' > "$H/.claude.json"
seed_manifest
"$AS" apply --scope project --write >/dev/null 2>&1 && ok "apply without git" || bad "apply failed"
"$AS" doctor >/dev/null 2>&1 && ok "doctor green without git" || bad "doctor failed"

hdr "C5) AGENTSTACK_HOME containing spaces (guard enforces through it)"
device "proj"
mkdir -p "$SBX/agent stack home"
export AGENTSTACK_HOME="$SBX/agent stack home"
echo '{}' > "$H/.claude.json"
seed_manifest
"$AS" init --global --force >/dev/null 2>&1
[ -f "$AGENTSTACK_HOME/agentstack.toml" ] && ok "init --global into a spaced machine home" || bad "no machine manifest"
"$AS" guard test cat .env >/dev/null 2>&1 && bad "guard allowed .env through a spaced home" || ok "guard denies .env through a spaced home"

# ═══ D. Fixes that closed this example's first-round gaps ════════════════════

hdr "D1) commands walk up to the project root from a subdirectory"
device "proj"
echo '{}' > "$H/.claude.json"
seed_manifest
git init -q .
mkdir -p src/deep && cd src/deep
"$AS" 2>&1 | grep -q "agentstack.toml" && ok "bare agentstack finds the root manifest from src/deep" || bad "overview lost the root manifest from a subdir"
"$AS" doctor >/dev/null 2>&1 && ok "doctor resolves the root manifest from src/deep" || bad "doctor errored from a subdir"
"$AS" apply --scope project --write >/dev/null 2>&1
if [ -f ../../.mcp.json ] && [ ! -f .mcp.json ]; then ok "apply from a subdir renders at the PROJECT ROOT"; else bad "apply from a subdir misplaced the render"; fi
"$AS" init 2>&1 | grep -qiE "already|initialized|--force" && ok "init from a subdir refuses to silently nest" || bad "init nested a second manifest"

hdr "D2) adopt pulls a hand-EDITED field of a manifest-known server"
device "proj2"
echo '{}' > "$H/.claude.json"
seed_manifest
"$AS" apply --scope project --write >/dev/null 2>&1
python3 -c "import json; d=json.load(open('.mcp.json')); d['mcpServers']['docs']['url']='https://docs-eu.example/mcp'; json.dump(d,open('.mcp.json','w'), indent=2)"
"$AS" adopt --scope project --write >/dev/null 2>&1
grep -q "docs-eu.example" .agentstack/agentstack.toml && ok "adopt pulled the hand-edited url into the manifest" || bad "adopt ignored the hand-edit"
"$AS" apply --scope project --write >/dev/null 2>&1
grep -q "docs-eu.example" .mcp.json && ok "the hand-edit survives the next apply (not reverted)" || bad "next apply reverted the adopted edit"

cd /
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
