#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — two-layer policy intersection, proven END-TO-END through the
# real gateway.
#
#   The repo ships an MCP server ("opsbox") and TRIES to allow a destructive
#   tool for itself. The machine layer — the user's own floor, which no repo
#   can loosen — denies `delete_*` / `admin_*` on every server. The effective
#   policy is the INTERSECTION, and this script proves the floor wins:
#
#     1. Untrusted           → the server is inert; the gateway serves only its
#                              control plane. Nothing spawned, nothing audited.
#     2. Trusted, discovery  → `tools_search` returns only the read-only tools.
#                              `delete_everything` is INVISIBLE, even though the
#                              repo allowlisted it.
#     3. Trusted, execution  → `get_status` succeeds; `delete_everything` and
#                              `admin_reset` are refused, and the refusal NAMES
#                              the machine layer. The audit log records the ok
#                              call as "ok" and the denied calls as "denied"
#                              with the exact rule.
#     4. `explain opsbox`    → shows BOTH policy layers (project + machine).
#     5. `doctor`            → reports the machine-policy summary as "restrictive".
#
# Exits nonzero and prints FAIL on any mismatch; safe to run unattended. Runs
# entirely inside an isolated sandbox — nothing touches your real config.
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

PAUSE="${DEMO_PAUSE:-0}"
say()  { printf '\n\033[1;35m▎ %s\033[0m\n' "$*"; [ "$PAUSE" = "0" ] || sleep "$PAUSE"; }

# ── isolated sandbox: AGENTSTACK_HOME redirects the ENTIRE machine state tree ─
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

INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}'

printf '\033[1;36m  agentstack — two-layer policy intersection\033[0m\n'

# ── the machine floor: the user's own policy, which no repo can loosen ───────
say "Machine layer: deny delete_* / admin_* on EVERY server (rename-proof \"*\")"
cat > "$AGENTSTACK_HOME/agentstack.toml" <<'EOF'
version = 1
[policy.tools]
"*" = ["!delete_*", "!*_admin", "!admin_*"]
EOF
printf '  the repo, meanwhile, allowlists delete_everything for itself:\n'
grep -n 'opsbox = ' "$PROJECT/.agentstack/agentstack.toml" | sed 's/^/    /'

"$AS" lock --manifest-dir "$PROJECT" >/dev/null

# ── 1) UNTRUSTED — the server is inert (control-plane only) ───────────────────
say "1) Untrusted: the gateway serves only its control plane — opsbox is inert"
UNTRUSTED_SEARCH="$(printf '%s\n%s\n' "$INIT" \
  '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"tools_search","arguments":{"query":"status items delete"}}}' \
  | "$AS" mcp --auto-project 2>/dev/null | tail -1)"
if grep -q 'not trusted' <<< "$UNTRUSTED_SEARCH" \
   && ! grep -q 'opsbox__' <<< "$UNTRUSTED_SEARCH"; then
  ok "untrusted: tools_search exposes no proxied tools (project not trusted)"
else
  bad "untrusted: tools_search should expose nothing; got: $UNTRUSTED_SEARCH"
fi

UNTRUSTED_CALL="$(printf '%s\n%s\n' "$INIT" \
  '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"opsbox__get_status","arguments":{}}}' \
  | "$AS" mcp --auto-project 2>/dev/null | tail -1)"
if grep -q 'unknown tool' <<< "$UNTRUSTED_CALL"; then
  ok "untrusted: a direct opsbox__get_status call is rejected as an unknown tool"
else
  bad "untrusted: expected an unknown-tool rejection; got: $UNTRUSTED_CALL"
fi

if [ ! -s "$AGENTSTACK_HOME/audit/calls.jsonl" ]; then
  ok "untrusted: nothing spawned — the audit log has no proxied calls"
else
  bad "untrusted: the audit log recorded a call while the repo was untrusted"
fi

# ── 2 + 3) TRUSTED — discovery filtering + execution firewall + audit ─────────
say "2+3) Trusted: review the manifest, then drive the gateway through one session"
"$AS" trust . --yes >/dev/null 2>&1

PROBE="$(python3 "$HERE/gateway_probe.py" "$AS" "$PROJECT")"

# pull each [is_error, text] pair out of the probe's JSON result
search_text="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['search'][1])" "$PROBE")"
gs_err="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['get_status'][0])" "$PROBE")"
gs_text="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['get_status'][1])" "$PROBE")"
del_err="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['delete_everything'][0])" "$PROBE")"
del_text="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['delete_everything'][1])" "$PROBE")"
adm_err="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['admin_reset'][0])" "$PROBE")"
adm_text="$(python3 -c "import json,sys; print(json.loads(sys.argv[1])['admin_reset'][1])" "$PROBE")"

# discovery: read-only tools visible, destructive/privileged INVISIBLE
if grep -q 'opsbox__get_status' <<< "$search_text" && grep -q 'opsbox__list_items' <<< "$search_text"; then
  ok "discovery: tools_search surfaces the read-only tools (get_status, list_items)"
else
  bad "discovery: expected get_status + list_items in search; got: $search_text"
fi
if ! grep -q 'delete_everything' <<< "$search_text"; then
  ok "discovery: delete_everything is INVISIBLE, though the repo allowlisted it (floor wins)"
else
  bad "discovery: delete_everything leaked into search: $search_text"
fi
if ! grep -q 'admin_reset' <<< "$search_text"; then
  ok "discovery: admin_reset is INVISIBLE (denied by the machine floor)"
else
  bad "discovery: admin_reset leaked into search: $search_text"
fi

# execution: allowed call succeeds
if [ "$gs_err" = "False" ] && grep -q 'get_status: ok' <<< "$gs_text"; then
  ok "execution: opsbox__get_status is allowed and returns ok"
else
  bad "execution: get_status should succeed; got err=$gs_err text=$gs_text"
fi

# execution: denied call refused, and the refusal NAMES the machine layer
if [ "$del_err" = "True" ] \
   && grep -q '!delete_\*' <<< "$del_text" \
   && grep -q 'machine policy' <<< "$del_text"; then
  ok "execution: delete_everything is refused, and the refusal names the machine layer"
else
  bad "execution: expected a machine-layer refusal; got err=$del_err text=$del_text"
fi
if [ "$adm_err" = "True" ] && grep -q 'machine policy' <<< "$adm_text"; then
  ok "execution: admin_reset is refused by the machine layer too"
else
  bad "execution: expected admin_reset refused by machine layer; got err=$adm_err text=$adm_text"
fi

# audit: the ok call and the denied call are both recorded with the rule
AUDIT="$AGENTSTACK_HOME/audit/calls.jsonl"
say "Audit log at \$AGENTSTACK_HOME/audit/calls.jsonl:"
sed 's/^/  /' "$AUDIT"

ok_line="$(grep '"tool":"get_status"' "$AUDIT" | tail -1 || true)"
if grep -q '"outcome":"ok"' <<< "$ok_line"; then
  ok "audit: the get_status call is recorded with outcome \"ok\""
else
  bad "audit: expected an ok record for get_status; got: ${ok_line:-<none>}"
fi

del_line="$(grep '"tool":"delete_everything"' "$AUDIT" | tail -1 || true)"
if grep -q '"outcome":"denied"' <<< "$del_line" \
   && grep -q '!delete_\*' <<< "$del_line" \
   && grep -q 'machine policy' <<< "$del_line"; then
  ok "audit: the delete_everything call is recorded as \"denied\" with the rule + layer"
else
  bad "audit: expected a denied record naming the rule; got: ${del_line:-<none>}"
fi

# ── 4) explain shows BOTH policy layers ───────────────────────────────────────
say "4) explain opsbox — surfaces both the project and machine policy layers"
EXPLAIN="$("$AS" explain opsbox --manifest-dir "$PROJECT" 2>&1)"
sed 's/^/  /' <<< "$EXPLAIN"
if grep -q 'Tool policy' <<< "$EXPLAIN" && grep -qi 'get_\*' <<< "$EXPLAIN"; then
  ok "explain: shows the PROJECT tool policy (allow get_*, list_*, …)"
else
  bad "explain: missing the project tool-policy line"
fi
if grep -q 'Tool policy (machine)' <<< "$EXPLAIN" && grep -q 'this project cannot loosen it' <<< "$EXPLAIN"; then
  ok "explain: shows the MACHINE tool policy and that the project cannot loosen it"
else
  bad "explain: missing the machine tool-policy line"
fi
if grep -qi 'Egress' <<< "$EXPLAIN" && grep -qi 'Secret access' <<< "$EXPLAIN"; then
  ok "explain: also surfaces the egress and secret policy dimensions"
else
  bad "explain: missing the egress/secret dimensions"
fi

# ── 5) doctor reports the machine-policy summary as restrictive ───────────────
say "5) doctor — machine-policy summary"
DOCTOR="$("$AS" doctor --manifest-dir "$PROJECT" 2>&1)"
ESC="$(printf '\033')"
DOCTOR_PLAIN="$(sed "s/${ESC}\[[0-9;]*m//g" <<< "$DOCTOR")"
SUMMARY="$(grep -A1 -m1 '^Machine policy$' <<< "$DOCTOR_PLAIN" | tail -1 || true)"
printf '  %s\n' "$SUMMARY"
if grep -qi 'restrictive' <<< "$SUMMARY"; then
  ok "doctor: reports the machine-policy summary as \"restrictive\""
else
  bad "doctor: expected a restrictive machine-policy line; got: ${SUMMARY:-<none>}"
fi

say "Two layers in. The intersection is the effective policy. The floor wins."
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
