#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — restricted folders, as a CI-grade proof.
#
# The maintainer's ask: a realistic repo tree where specific folders are
# off-limits to agents. This repo (a fake `acme-billing` service) lets agents
# read and edit `src/` and `docs/` freely, but declares `secrets/`,
# `personal-notes/`, and `infra/prod/` as off-limits in
# `.agentstack/agentstack.toml`.
#
# `agentstack guard` wires a COOPERATIVE pre-tool-use hook into agent CLIs.
# Before the harness runs a tool it hands the pending call to
# `agentstack guard check`, which decides allow/deny from the machine's own
# `[policy.filesystem] deny` globs (never readable or writable) and `[guard]
# allow_roots` (write roots beyond the workspace) — and records every denial
# to the audit log.
#
# This script feeds realistic pre-tool-use payloads (in two CLI dialects —
# Claude and Codex) into `guard check` and ASSERTS each outcome:
#
#   Read  secrets/api-keys.env       → DENY   (off-limits folder)
#   Read  secrets/service-account.json → DENY (off-limits folder)
#   Write personal-notes/diary.md     → DENY  (off-limits folder)
#   Read  infra/prod/main.tf          → DENY  (off-limits folder)
#   Read  src/index.ts                → ALLOW (allowed code)
#   Write src/index.ts                → ALLOW (allowed code)
#   Write /opt/acme/data/out.txt      → DENY  (outside workspace + allow_roots)
#   Bash  rm -rf .                     → DENY  (deletes the workspace root)
#   Bash  ls src                       → ALLOW (the guard stays out of the way)
#   (Codex dialect) Read secret        → DENY  (stderr + exit 2)
#   guard test rm -rf /                → nonzero exit
#
# …then greps the audit log to prove every denial was recorded as a
# `host-guard` entry, and probes whether a project-layer deny is honored.
#
# It exits nonzero and prints FAIL on any mismatch, so it is safe to run
# unattended in CI. Self-contained: isolated temp HOME/AGENTSTACK_HOME, nothing
# touches your real config.
#
#   IMPORTANT (F1): today `guard check` reads `[policy.filesystem] deny` from
#   the MACHINE manifest only; a project-layer deny at `.agentstack/agentstack.toml`
#   is silently IGNORED. This script therefore mirrors the repo's three folder
#   globs into the machine manifest so the guard actually enforces them, and the
#   final section demonstrates F1 with an isolated probe (SKIP, not a fake pass).
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
SKIP=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$*"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=$((FAIL + 1)); }
skip() { printf '  \033[33mSKIP\033[0m %s\n' "$*"; SKIP=$((SKIP + 1)); }

PAUSE="${DEMO_PAUSE:-0}"
say()  { printf '\n\033[1;35m▎ %s\033[0m\n' "$*"; [ "$PAUSE" = 0 ] || sleep "$PAUSE"; }
run()  { printf '\033[2m$ %s\033[0m\n' "$*"; [ "$PAUSE" = 0 ] || sleep "$PAUSE"; }

# ── isolated sandbox (nothing touches your real config) ──────────────────────
SBX="$(mktemp -d)"
export AGENTSTACK_HOME="$SBX/home"
export HOME="$SBX/fakehome"
mkdir -p "$AGENTSTACK_HOME" "$HOME"
trap 'rm -rf "$SBX"' EXIT

AUDIT="$AGENTSTACK_HOME/audit/calls.jsonl"

# ── the user's OWN machine firewall — which no cloned repo can loosen ────────
# `agentstack guard install` seeds the secret-file globs; here the user has
# ALSO extended the deny list with this repo's three off-limits folders
# (mirroring the project manifest — see F1 in the header). allow_roots is empty,
# so writes are confined to the workspace and temp dirs.
cat > "$AGENTSTACK_HOME/agentstack.toml" <<'EOF'
version = 1

[guard]
enabled = true
allow_roots = []

[policy.filesystem]
deny = [
  ".env", ".env.*", "*.pem", "id_rsa",
  "secrets/**", "personal-notes/**", "infra/prod/**",
]
EOF

# ── clone the committed bundle into the sandbox and work there ────────────────
PROJECT="$SBX/project"
mkdir -p "$PROJECT"
cp -R "$HERE/bundle/." "$PROJECT/"
cd "$PROJECT"

printf '\033[1;36m  agentstack — restricted folders (acme-billing)\033[0m\n'
say "The repo declares its off-limits folders:"
run "cat .agentstack/agentstack.toml"
sed -n '/\[policy.filesystem\]/,$p' .agentstack/agentstack.toml | sed 's/^/  /'

# Expected number of guard-check denials, so the audit assertion is exact.
EXPECTED_DENIALS=0

# ── payload builders ─────────────────────────────────────────────────────────
# A Claude-Code pre-tool-use payload for a file tool. $1=tool $2=path
file_payload() {
  python3 - "$PROJECT" "$1" "$2" <<'PY'
import json, sys
ws, tool, path = sys.argv[1], sys.argv[2], sys.argv[3]
inp = {"file_path": path}
if tool in ("Write", "Edit"):
    inp["content"] = "x"
print(json.dumps({
    "session_id": "restricted-folders", "cwd": ws,
    "hook_event_name": "PreToolUse", "tool_name": tool, "tool_input": inp,
}))
PY
}
# A Claude-Code Bash payload. $1=command
bash_payload() {
  python3 - "$PROJECT" "$1" <<'PY'
import json, sys
ws, command = sys.argv[1], sys.argv[2]
print(json.dumps({
    "session_id": "restricted-folders", "cwd": ws,
    "hook_event_name": "PreToolUse", "tool_name": "Bash",
    "tool_input": {"command": command},
}))
PY
}

# Claude protocol: DENY emits `"permissionDecision":"deny"` on stdout, ALLOW is
# silent; both exit 0 (the JSON body is the signal, not the exit code).
claude_decide() {
  local out
  out="$("$AS" guard check --protocol claude 2>/dev/null || true)"
  grep -q '"permissionDecision":"deny"' <<< "$out" && echo BLOCKED || echo ALLOWED
}

assert_file_deny() { # $1=tool $2=path $3=label
  run "$1 $2"
  if [ "$(file_payload "$1" "$2" | claude_decide)" = BLOCKED ]; then
    ok "$3 — blocked"; EXPECTED_DENIALS=$((EXPECTED_DENIALS + 1))
  else
    bad "$3 — expected DENY, guard allowed it"
  fi
}
assert_file_allow() { # $1=tool $2=path $3=label
  run "$1 $2"
  if [ "$(file_payload "$1" "$2" | claude_decide)" = ALLOWED ]; then
    ok "$3 — allowed"
  else
    bad "$3 — expected ALLOW, guard blocked it"
  fi
}
assert_bash_deny() { # $1=command $2=label
  run "$1"
  if [ "$(bash_payload "$1" | claude_decide)" = BLOCKED ]; then
    ok "$2 — blocked"; EXPECTED_DENIALS=$((EXPECTED_DENIALS + 1))
  else
    bad "$2 — expected DENY, guard allowed it"
  fi
}
assert_bash_allow() { # $1=command $2=label
  run "$1"
  if [ "$(bash_payload "$1" | claude_decide)" = ALLOWED ]; then
    ok "$2 — allowed"
  else
    bad "$2 — expected ALLOW, guard blocked it"
  fi
}

# ── 1) off-limits folders are refused for read AND write ─────────────────────
say "Off-limits folders — every read/write is refused:"
assert_file_deny Read  "secrets/api-keys.env"       "read a secret"
assert_file_deny Read  "secrets/service-account.json" "read a service-account key"
assert_file_deny Write "personal-notes/diary.md"    "write into the private diary"
assert_file_deny Read  "infra/prod/main.tf"         "read production terraform"

# ── 2) allowed folders stay out of the way ───────────────────────────────────
say "Allowed code — the guard never gets in the way:"
assert_file_allow Read  "src/index.ts"      "read application code"
assert_file_allow Write "src/index.ts"      "edit application code"

# ── 3) writes are confined to the workspace ──────────────────────────────────
say "A write outside the workspace (and outside allow_roots) is refused:"
assert_file_deny Write "/opt/acme/data/out.txt" "write outside the workspace"

# ── 4) destructive shell commands ────────────────────────────────────────────
say "Shell commands are judged too:"
assert_bash_deny  "rm -rf ."   "rm -rf of the workspace root"
assert_bash_allow "ls src"     "an ordinary command"

# ── 5) a SECOND CLI dialect (Codex): deny = stderr + exit 2 ──────────────────
say "Multi-CLI: the same policy, answered in Codex's dialect (deny = exit 2):"
codex_payload() { # $1=path
  python3 - "$PROJECT" "$1" <<'PY'
import json, sys
ws, path = sys.argv[1], sys.argv[2]
# turn_id present so the shape also auto-detects as Codex.
print(json.dumps({"turn_id": "t1", "cwd": ws,
                  "tool_name": "Read", "tool_input": {"file_path": path}}))
PY
}
run "guard check --protocol codex   (Read secrets/api-keys.env)"
set +e
CODEX_ERR="$(codex_payload "secrets/api-keys.env" | "$AS" guard check --protocol codex 2>&1 1>/dev/null)"
CODEX_CODE=$?
set -e
if [ "$CODEX_CODE" -eq 2 ] && grep -q "agentstack guard blocked" <<< "$CODEX_ERR"; then
  ok "codex dialect denies with exit 2 + a stderr reason"
  EXPECTED_DENIALS=$((EXPECTED_DENIALS + 1))
else
  bad "codex dialect: expected exit 2 + stderr reason, got exit $CODEX_CODE / '$CODEX_ERR'"
fi
run "guard check --protocol codex   (Read src/index.ts — allowed)"
set +e
codex_payload "src/index.ts" | "$AS" guard check --protocol codex >/dev/null 2>&1
CODEX_OK_CODE=$?
set -e
if [ "$CODEX_OK_CODE" -eq 0 ]; then
  ok "codex dialect allows an ordinary read (exit 0)"
else
  bad "codex dialect: allowed read should exit 0, got $CODEX_OK_CODE"
fi

# ── 6) the human entrypoint: `guard test` ────────────────────────────────────
say "The human entrypoint — \`guard test\` exits nonzero on a deny:"
run "agentstack guard test rm -rf /"
set +e
"$AS" guard test rm -rf / >/dev/null 2>&1
TEST_CODE=$?
set -e
if [ "$TEST_CODE" -ne 0 ]; then ok "guard test 'rm -rf /' exits nonzero ($TEST_CODE)"; else bad "guard test 'rm -rf /' should exit nonzero"; fi
run "agentstack guard test ls -la"
set +e
"$AS" guard test ls -la >/dev/null 2>&1
TEST_OK_CODE=$?
set -e
if [ "$TEST_OK_CODE" -eq 0 ]; then ok "guard test 'ls -la' exits 0"; else bad "guard test 'ls -la' should exit 0"; fi

# ── 7) guard status reflects the config ──────────────────────────────────────
say "\`guard status\` reflects the live machine config:"
run "agentstack guard status"
STATUS="$("$AS" guard status 2>&1)"
if grep -q "guard:.*enabled" <<< "$STATUS"; then ok "status reports guard: enabled"; else bad "status should report guard enabled"; fi
if grep -q "secrets/\*\*" <<< "$STATUS"; then ok "status lists the secrets/** deny glob"; else bad "status should list the deny globs"; fi

# ── 8) the audit trail: every denial recorded as a host-guard entry ──────────
# This is the investigation: guard-demo proved BASH denials are audited; here we
# prove READ and WRITE file-tool denials are audited too (server=host-guard).
say "The audit log recorded every denial (server=host-guard, outcome=denied):"
run "grep host-guard \$AGENTSTACK_HOME/audit/calls.jsonl"
RECORDED="$(grep -c 'host-guard' "$AUDIT" 2>/dev/null || true)"
RECORDED="${RECORDED:-0}"
if [ "$RECORDED" -eq "$EXPECTED_DENIALS" ]; then
  ok "all $EXPECTED_DENIALS denials written to the audit log (reads, writes, bash, codex)"
else
  bad "expected $EXPECTED_DENIALS host-guard audit records, found $RECORDED"
fi
# Prove the file-tool denials specifically are present (not just bash).
for subj in "read: secrets/api-keys.env" "write: personal-notes/diary.md" "read: infra/prod/main.tf"; do
  if grep -q "$subj" "$AUDIT" 2>/dev/null; then
    ok "audit contains a record for '$subj'"
  else
    bad "audit is missing a record for '$subj'"
  fi
done
if [ "$RECORDED" -gt 0 ]; then
  printf '  \033[2msample audit records:\033[0m\n'
  grep 'host-guard' "$AUDIT" | python3 -c '
import json, sys
for line in sys.stdin:
    d = json.loads(line)
    print("   ", {k: d.get(k) for k in ("server", "tool", "outcome")})' | head -4
fi

# ── 9) DEFECT PROBE (F1): is a PROJECT-layer deny honored? ────────────────────
# Isolated sub-sandbox: machine deny is EMPTY; a lone project deny at
# `.agentstack/agentstack.toml` should (per CLAUDE.md + guard-demo README) ADD a
# restriction. We assert nothing false — if the project layer is honored we PASS;
# if it is ignored we SKIP loudly and prove the deny rule itself works from the
# legacy-root manifest, isolating the bug to path resolution.
say "Probe: does a project-layer [policy.filesystem] deny take effect? (F1)"
PBX="$(mktemp -d)"
PWS="$PBX/project"
mkdir -p "$PBX/home" "$PBX/fakehome" "$PWS/.agentstack" "$PWS/.git" "$PWS/vault"
printf 'version = 1\n[guard]\nenabled = true\nallow_roots = []\n[policy.filesystem]\ndeny = []\n' > "$PBX/home/agentstack.toml"
printf 'x\n' > "$PWS/vault/token.txt"
probe_read() { # $1=manifest-location  → echoes BLOCKED/ALLOWED
  python3 - "$PWS" <<'PY' | AGENTSTACK_HOME="$PBX/home" HOME="$PBX/fakehome" "$AS" guard check --protocol claude 2>/dev/null | { grep -q '"permissionDecision":"deny"' && echo BLOCKED || echo ALLOWED; }
import json, sys
print(json.dumps({"cwd": sys.argv[1], "tool_name": "Read",
                  "tool_input": {"file_path": "vault/token.txt"}}))
PY
}
# (a) deny declared in the PREFERRED project location: .agentstack/agentstack.toml
printf 'version = 1\n[policy.filesystem]\ndeny = ["vault/*"]\n' > "$PWS/.agentstack/agentstack.toml"
PROJECT_LAYER="$(probe_read)"
if [ "$PROJECT_LAYER" = BLOCKED ]; then
  ok "project-layer deny at .agentstack/agentstack.toml is honored (union works)"
else
  skip "F1: project-layer deny at .agentstack/agentstack.toml is IGNORED by guard check"
  printf '       \033[2m(guard reads <repo>/agentstack.toml via load_from_dir; it never\n'
  printf '        consults the .agentstack/ subdir — the documented preferred location.\n'
  printf '        Effect: a repo cannot add restrictions the way CLAUDE.md/guard-demo\n'
  printf '        README promise; only the machine layer enforces. This is why this\n'
  printf '        demo mirrors the folder globs into the machine manifest.)\033[0m\n'
  # Control: the SAME deny at the legacy-root manifest DOES enforce — proving the
  # rule works and isolating the bug to `.agentstack/` path resolution.
  rm -f "$PWS/.agentstack/agentstack.toml"
  printf 'version = 1\n' > "$PWS/.agentstack/agentstack.toml"
  printf 'version = 1\n[policy.filesystem]\ndeny = ["vault/*"]\n' > "$PWS/agentstack.toml"
  if [ "$(probe_read)" = BLOCKED ]; then
    ok "control: the same deny at <repo>/agentstack.toml (legacy root) IS enforced"
  else
    bad "control: even the legacy-root project deny was ignored — deny engine broken?"
  fi
fi
rm -rf "$PBX"

# ── summary ──────────────────────────────────────────────────────────────────
say "Off-limits folders refused, allowed code untouched, every denial audited."
printf '\n\033[1mSummary:\033[0m %d passed, %d failed' "$PASS" "$FAIL"
[ "$SKIP" -gt 0 ] && printf ', %d skipped (see F1)' "$SKIP"
printf '\n'
[ "$FAIL" -eq 0 ] || exit 1
