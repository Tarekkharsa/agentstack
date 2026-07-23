#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — the zero-file trust gate, in 60 seconds.
#
#   "Clone any repo. Its agents' capabilities are INERT until you review and
#    trust it. Every brokered call is firewalled and audited."
#
# Self-contained: spins up an isolated home + a tiny mock MCP server, so it runs
# anywhere with `agentstack` on PATH and python3. Set DEMO_PAUSE=2.5 for a paced
# screen recording (asciinema); default is snappy.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail
AS="${AGENTSTACK_BIN:-agentstack}"
if [[ "$AS" == */* ]]; then
  AS="$(cd "$(dirname "$AS")" && pwd -P)/$(basename "$AS")"
else
  AS="$(command -v "$AS")"
fi
PAUSE="${DEMO_PAUSE:-0.6}"
say()  { printf '\n\033[1;35m▎ %s\033[0m\n' "$*"; sleep "$PAUSE"; }
run()  { printf '\033[2m$ %s\033[0m\n' "$*"; sleep "$PAUSE"; }
note() { printf '  \033[2m%s\033[0m\n' "$*"; }

# ── isolated sandbox (nothing touches your real config) ──────────────────────
SBX="$(mktemp -d)"; export AGENTSTACK_HOME="$SBX/home"; export HOME="$SBX/fakehome"
mkdir -p "$AGENTSTACK_HOME" "$HOME"
# the sandbox HOME has no git identity; the library-sync scene commits
export GIT_AUTHOR_NAME="demo" GIT_AUTHOR_EMAIL="demo@example.com"
export GIT_COMMITTER_NAME="demo" GIT_COMMITTER_EMAIL="demo@example.com"
SBXC="$(cd "$SBX" && pwd -P)"        # canonical form (macOS: /var → /private/var)
trap 'rm -rf "$SBX"' EXIT

# a tiny zero-dependency stdio MCP server standing in for "some repo's server".
# Two tools: a benign `echo`, and a sensitive `secret_read` you'll firewall.
cat > "$SBX/server.py" <<'PY'
import sys, json
def send(o): sys.stdout.write(json.dumps(o)+"\n"); sys.stdout.flush()
TOOLS=[{"name":"echo","description":"Echo a message back","inputSchema":{"type":"object","properties":{"msg":{"type":"string"}}}},
       {"name":"secret_read","description":"Read a secret file","inputSchema":{"type":"object"}}]
for line in sys.stdin:
    if not line.strip(): continue
    m=json.loads(line); method,rid=m.get("method"),m.get("id")
    if method=="initialize": send({"jsonrpc":"2.0","id":rid,"result":{"protocolVersion":"2024-11-05","capabilities":{"tools":{}},"serverInfo":{"name":"demo","version":"1.0"}}})
    elif method=="tools/list": send({"jsonrpc":"2.0","id":rid,"result":{"tools":TOOLS}})
    elif method=="tools/call":
        a=(m.get("params") or {}).get("arguments") or {}; name=(m.get("params") or {}).get("name")
        out="echo: "+str(a.get("msg","")) if name=="echo" else "(sensitive data)"
        send({"jsonrpc":"2.0","id":rid,"result":{"content":[{"type":"text","text":out}]}})
    elif rid is not None: send({"jsonrpc":"2.0","id":rid,"error":{"code":-32601,"message":"no"}})
PY

# "a stranger's repo" — declares an MCP server AND a tool firewall
REPO="$SBX/some-cloned-repo"; mkdir -p "$REPO/.agentstack"
cat > "$REPO/.agentstack/agentstack.toml" <<EOF
version = 1
[targets]
default = ["claude-code"]
[servers.demo]
type = "stdio"
command = "python3"
args = ["$SBX/server.py"]
[policy]                       # tool firewall: block the sensitive tool
tools = { demo = ["!secret_read"] }
[profiles.default]
servers = ["demo"]
EOF
cd "$REPO"
"$AS" lock --manifest-dir "$REPO" >/dev/null

# helpers to drive the gateway (agentstack mcp) over stdio like an agent would
mcp() { "$AS" mcp --auto-project 2>/dev/null; }
init='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"demo","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}'
search() { printf '%s\n%s\n' "$init" '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"tools_search","arguments":{"query":"echo"}}}' | mcp | pick 9; }
callecho() { printf '%s\n%s\n' "$init" '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"demo__echo","arguments":{"msg":"hi from a trusted repo"}}}' | mcp | pick 9; }
callsecret() { printf '%s\n%s\n' "$init" '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"demo__secret_read","arguments":{}}}' | mcp | pick 9; }
# prettify: swap the sandbox's temp paths (raw + canonical) for friendly names
clean() { sed -e "s#$SBXC/some-cloned-repo#the-repo#g" -e "s#$SBX/some-cloned-repo#the-repo#g" -e "s#$SBXC#/tmp/sandbox#g" -e "s#$SBX#/tmp/sandbox#g"; }
pick() { python3 -c "
import json,sys
subs=sys.argv[2:]
for l in sys.stdin:
    try: o=json.loads(l)
    except: continue
    if o.get('id')==int(sys.argv[1]):
        r=o.get('result') or o.get('error') or {}
        c=r.get('content'); m=(''.join(x.get('text','') for x in c) if isinstance(c,list) else json.dumps(r)).replace(chr(10),' ')
        for s in subs:
            if s: m=m.replace(s+'/some-cloned-repo','the-repo').replace(s,'/tmp/sandbox')
        print('  '+m[:180])
" "$1" "$SBXC" "$SBX"; }

printf '\033[1;36m  agentstack — the zero-file trust gate\033[0m\n'
say "You just cloned some repo. It declares MCP servers. What can its agent touch?"
run "agentstack mcp --auto-project   # (an agent asks the gateway what tools exist)"
note "→ untrusted, so:"; search

say "Nothing. Until YOU review it, none of its servers are spawned or even contacted."
run "agentstack trust ."
"$AS" trust . --yes --consented-digest "$("$AS" trust . --preview | sed -n 's/.*"surface_digest": "\([^"]*\)".*/\1/p')" 2>&1 | clean | sed 's/^/  /' | grep -E "runs|trusted at|Withdraw" || true

say "You saw exactly what it would run, and trusted it (pinned to a content digest)."
run "agentstack mcp --auto-project   # ask again, now trusted"
search

say "Now its tools are live through the gateway. The benign one is brokered end to end:"
run 'demo__echo { "msg": "hi from a trusted repo" }'
callecho
note "…and every brokered call lands in the audit log:"
tail -1 "$AGENTSTACK_HOME/audit/calls.jsonl" | python3 -c "import json,sys;d=json.load(sys.stdin);print('  audit:',{k:d.get(k) for k in ['server','tool','outcome','ms']})"

say "But the repo's manifest also declared a FIREWALL. The sensitive tool is blocked:"
run 'demo__secret_read   # denied by [policy.tools] demo = "!secret_read"'
callsecret
tail -1 "$AGENTSTACK_HOME/audit/calls.jsonl" | python3 -c "import json,sys;d=json.load(sys.stdin);print('  audit:',{k:d.get(k) for k in ['tool','outcome','detail']})"

say "One more gate: your central library syncs machine-to-machine over git. Secrets don't."
git init --bare -q "$SBX/remote.git"
rm -rf "$AGENTSTACK_HOME/lib"
"$AS" lib sync --init --remote "$SBX/remote.git" >/dev/null 2>&1   # clone the (empty) team remote
mkdir -p "$AGENTSTACK_HOME/lib/servers"
cat > "$AGENTSTACK_HOME/lib/servers/payments.toml" <<'EOF'
type = "http"
url = "https://payments.internal/mcp"

[headers]
Authorization = "Bearer sk-live-9f3a1c"
EOF
note 'someone pastes a REAL token into a shared server definition…'
run "agentstack lib sync"
"$AS" lib sync 2>&1 | clean | sed 's/^/  /' || true

say "Blocked before it ever reaches a commit. Make it a \${REF} and it travels safely:"
cat > "$AGENTSTACK_HOME/lib/servers/payments.toml" <<'EOF'
type = "http"
url = "https://payments.internal/mcp"

[headers]
Authorization = "Bearer ${PAYMENTS_TOKEN}"
EOF
run "agentstack lib sync"
"$AS" lib sync 2>&1 | clean | sed 's/^/  /'

say "That's the gate: clone → inert → review → trust → firewalled → audited — and secrets never travel."
printf '\033[1;32m  Nothing an agent can touch that you didn'\''t review. Nothing secret ever pushed.\033[0m\n\n'
