#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — the malicious-repo demo, as a CI-grade proof.
#
# Runs the SAME hostile bundle three ways and ASSERTS the outcome of each:
#
#   1. Unprotected              → the server phones home; the sink RECEIVES the
#                                 planted secret. (The threat is real.)
#   2. AgentStack, untrusted    → the server is inert; the sink stays EMPTY.
#                                 (Nothing runs until you review it.)
#   3. AgentStack, trusted, but
#      the machine firewall      → the exfil tool is DENIED and audited; the
#      denies the exfil tool        sink stays EMPTY. (No repo can loosen your
#                                    own machine policy.)
#
# It exits nonzero and prints FAIL on any mismatch, so it is safe to run
# unattended: the demo either provably works or fails loudly.
#
# What this demo does NOT claim: that exfiltration is impossible. A trusted
# repo can still use any allowed channel. This demo proves trust + tool-policy;
# `run --sandbox --lockdown` is the separate enforced-egress primitive.
#
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=...) and python3.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd -P)"
AS="${AGENTSTACK_BIN:-agentstack}"
if [[ "$AS" == */* ]]; then
  AS="$(cd "$(dirname "$AS")" && pwd -P)/$(basename "$AS")"
else
  AS="$(command -v "$AS")"
fi
PASS=0
FAIL=0
ok()  { printf '  \033[32mPASS\033[0m %s\n' "$*"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=$((FAIL + 1)); }
# Optional pacing for screen recordings (DEMO_PAUSE=2.5). Off by default so CI
# stays fast; a no-op when unset.
PAUSE="${DEMO_PAUSE:-0}"
pause() { [ "$PAUSE" = "0" ] || sleep "$PAUSE"; }

# ── isolated sandbox (nothing touches your real config) ──────────────────────
SBX="$(mktemp -d)"
export AGENTSTACK_HOME="$SBX/home"
export HOME="$SBX/fakehome"
mkdir -p "$AGENTSTACK_HOME" "$HOME"
trap 'kill "${SINK_PID:-}" 2>/dev/null || true; rm -rf "$SBX"' EXIT

# the planted (fake) credential the hostile server tries to steal
SECRET_FILE="$SBX/planted-credential"
echo "sk-demo-FAKE-not-a-real-secret-0000" > "$SECRET_FILE"
export SECRET_FILE

# copy the committed bundle into the sandbox; point the server at an absolute path
REPO="$SBX/cloned-repo"
mkdir -p "$REPO/.agentstack"
cp "$HERE/bundle/evil_server.py" "$REPO/evil_server.py"
sed "s#\"\./evil_server.py\"#\"$REPO/evil_server.py\"#" \
  "$HERE/bundle/.agentstack/agentstack.toml" > "$REPO/.agentstack/agentstack.toml"

# start the localhost sink on a free port
SINK_LOG="$SBX/sink.log"
: > "$SINK_LOG"
export SINK_LOG
PORT="$(python3 -c 'import socket;s=socket.socket();s.bind(("127.0.0.1",0));print(s.getsockname()[1]);s.close()')"
SINK_PORT="$PORT" python3 "$HERE/sink.py" &
SINK_PID=$!
disown "$SINK_PID" 2>/dev/null || true  # silence job-control "Terminated" on cleanup
sleep 0.5
export SINK_URL="http://127.0.0.1:$PORT"

sink_received() { [ -s "$SINK_LOG" ]; }
reset_sink()    { : > "$SINK_LOG"; }

INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"demo","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}'

printf '\n\033[1mAgentStack — malicious-repo demo (asserting)\033[0m\n'
pause

# ── 1) UNPROTECTED — a bare harness runs the repo's server directly ──────────
printf '\n\033[1m1) Unprotected: a bare harness runs the cloned server\033[0m\n'
pause
reset_sink
printf '%s\n%s\n' "$INIT" \
  '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"exfiltrate","arguments":{}}}' \
  | python3 "$REPO/evil_server.py" > /dev/null 2>&1 || true
sleep 0.3
if sink_received; then
  ok "the sink received exfiltrated data — the threat is real"
else
  bad "expected exfiltration on the unprotected path, sink was empty"
fi

# ── 2) PROTECTED, UNTRUSTED — the trust gate keeps it inert ──────────────────
printf '\n\033[1m2) AgentStack, not yet trusted: the server is inert\033[0m\n'
pause
reset_sink
cd "$REPO"
"$AS" lock --manifest-dir "$REPO" >/dev/null
TOOLS_OUT="$(printf '%s\n%s\n' "$INIT" \
  '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"tools_search","arguments":{"query":"exfiltrate"}}}' \
  | "$AS" mcp --auto-project 2>/dev/null || true)"
if ! grep -q "demo__exfiltrate" <<< "$TOOLS_OUT"; then
  ok "the exfiltrate tool is not exposed while untrusted"
else
  bad "an untrusted repo exposed its tool"
fi
sleep 0.3
if ! sink_received; then
  ok "the sink stayed empty — the server was never spawned"
else
  bad "an untrusted repo still phoned home"
fi

# ── 3) PROTECTED, TRUSTED + machine firewall ─────────────────────────────────
printf '\n\033[1m3) Trusted, but the machine firewall denies the exfil tool\033[0m\n'
pause
reset_sink
# the user's OWN machine policy — which no repo can loosen — denies `exfiltrate`
# on every server (the rename-proof "*" key).
cat > "$AGENTSTACK_HOME/agentstack.toml" <<'EOF'
version = 1
[policy.tools]
"*" = ["!exfiltrate"]
EOF
consent=$("$AS" trust . --preview | sed -n 's/.*"surface_digest": "\([^"]*\)".*/\1/p')
"$AS" trust . --yes --consented-digest "$consent" > /dev/null 2>&1
printf '%s\n%s\n' "$INIT" \
  '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"demo__exfiltrate","arguments":{}}}' \
  | "$AS" mcp --auto-project > /dev/null 2>&1 || true
sleep 0.3
LAST="$(tail -1 "$AGENTSTACK_HOME/audit/calls.jsonl" 2>/dev/null || true)"
if grep -q 'denied' <<< "$LAST"; then
  ok "the call was firewalled and written to the audit log as denied"
else
  bad "expected a denied audit record; got: ${LAST:-<none>}"
fi
if ! sink_received; then
  ok "the sink stayed empty — the exfil call never reached the server"
else
  bad "a firewalled repo still phoned home"
fi

# This fixture intentionally stops at the gateway boundary. Enforced per-host
# egress is a separate composition exercised by `run --sandbox --lockdown`;
# adding it here would turn a fast trust/firewall proof into a Docker demo.

pause
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
