#!/usr/bin/env bash
# The closed-loop demo — one reviewed setup, every CLI, a firewall, and receipts.
# A vendor publishes a versioned pack (git repo + pack.toml + tag); a fresh
# machine installs it, spreads it across every CLI, firewalls one of its tools,
# watches an agent's call get refused live, reads the audit receipts, sees what
# the server costs in context, and picks up the vendor's next tag with one
# `upgrade`. Fully fenced: throwaway HOME, never your real configs. Idempotent.
#
# Record it (memory: vhs stalls on this machine — use asciinema + agg):
#   mkdir -p runtime
#   DEMO_PAUSE=2.5 asciinema rec runtime/closed-loop.cast -c ./demo-closed-loop.sh --overwrite
#   agg --font-size 16 runtime/closed-loop.cast ../../docs/closed-loop.gif
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

# Build the debug binary.
( cd .. && source "$HOME/.cargo/env" 2>/dev/null || true; cargo build --quiet )

bin="$here/../../target/debug/agentstack"
sandbox="$here/runtime/closed-loop"
home="$sandbox/home"
proj="$sandbox/proj"
packrepo="$sandbox/acme-pack"

rm -rf "$sandbox"
mkdir -p "$home" "$proj" "$packrepo"

# agentstack against the fenced machine — never your real HOME. The pack's
# secret resolves from process env (first link in the resolver chain).
as() { env HOME="$home" AGENTSTACK_HOME="$sandbox/ashome" ACME_TOKEN=demo-token "$bin" "$@"; }
line() { sleep "${DEMO_PAUSE:-0}"; printf '\n\033[1m== %s ==\033[0m\n' "$1"; }
git_q() { git -c user.name=acme -c user.email=dev@acme.dev -C "$packrepo" "$@" >/dev/null 2>&1; }

# ─── The vendor side: a pack repo. Its MCP server is a tiny local stdio server
#     (stand-in for `npx @acme/mcp`) with two tools: search_docs, delete_index.
cat > "$proj/acme-mcp.sh" <<'SH'
#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9][0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"acme","version":"1"}}}\n' "$id" ;;
    *'"method":"tools/list"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"search_docs","description":"Search Acme docs.","inputSchema":{"type":"object","properties":{"query":{"type":"string"}}}},{"name":"delete_index","description":"Delete a search index.","inputSchema":{"type":"object"}}]}}\n' "$id" ;;
    *'"search_docs"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"3 docs matched \\"indexing\\""}],"isError":false}}\n' "$id" ;;
    *'"delete_index"'*)
      printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"index deleted"}],"isError":false}}\n' "$id" ;;
  esac
done
SH

mkdir -p "$packrepo/skills/sql-review"
cat > "$packrepo/pack.toml" <<'TOML'
name = "acme"
description = "Acme's agent setup: docs MCP + the SQL review skill."

[server]
type = "stdio"
command = "sh"
args = ["./acme-mcp.sh"]
secret_env = ["ACME_TOKEN"]

[[skill]]
name = "sql-review"
path = "skills/sql-review"
TOML
cat > "$packrepo/skills/sql-review/SKILL.md" <<'MD'
---
name: sql-review
description: Review SQL migrations before they ship.
---
Check every migration for missing indexes and unbounded scans.
MD
git_q init
git_q add .
git_q commit -m "acme pack v1.0.0"
git_q tag v1.0.0
packurl="file://$packrepo"

# ─── The consumer side: a fresh machine that already runs Claude Code.
cat > "$home/.claude.json" <<'JSON'
{ "mcpServers": {} }
JSON
cd "$proj"

line "0. A vendor publishes a pack — a git repo with a pack.toml, tagged v1.0.0"
sed -n '1,12p' "$packrepo/pack.toml"

line "1. A fresh machine: import what exists, then install the pack AT ITS TAG"
as init --yes >/dev/null
as add from "git:$packurl@v1.0.0" --write

line "2. Secrets stayed \${REF}s — the manifest is commit-safe"
grep -A 4 'servers.acme' .agentstack/agentstack.toml

line "3. apply --write — the whole pack spreads to every CLI on this machine"
as apply --write

line "4. Firewall it: [policy.tools] — deny the destructive tool at the gateway"
cat >> .agentstack/agentstack.toml <<'TOML'

[policy.tools]
acme = ["!delete_*"]
TOML
tail -2 .agentstack/agentstack.toml

line "5. An agent calls through the gateway: search OK — delete REFUSED, by rule"
as mcp <<'EOF' 2>/dev/null | python3 -c '
import sys, json
for l in sys.stdin:
    try: m = json.loads(l)
    except ValueError: continue
    if m.get("id") in (2, 3) and "result" in m:
        r = m["result"]; text = r["content"][0]["text"]
        mark = "\033[31m✗\033[0m" if r.get("isError") else "\033[32m✓\033[0m"
        tool = "acme__search_docs " if m["id"] == 2 else "acme__delete_index"
        print(f"  {mark} {tool} → {text}")
'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"demo-agent","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"acme__search_docs","arguments":{"query":"indexing"}}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"acme__delete_index","arguments":{}}}
EOF

line "6. Receipts: every brokered call is audited — digests, never values"
as audit --calls

line "7. What does the server cost? Context-cost lens, measured live"
as report usage --live

line "8. The vendor ships v1.1.0 — one command picks it up, previews, re-pins"
sed -i '' 's/unbounded scans/unbounded scans and lock contention/' \
  "$packrepo/skills/sql-review/SKILL.md" 2>/dev/null || \
  sed -i 's/unbounded scans/unbounded scans and lock contention/' "$packrepo/skills/sql-review/SKILL.md"
git_q add . && git_q commit -m "v1.1.0" && git_q tag v1.1.0
as upgrade acme --yes --write

line "9. Done — one manifest: every CLI, a firewall, receipts, versioned upgrades"
grep -E 'source|version' .agentstack/agentstack.toml | grep acme-pack || true
printf 'Fenced sandbox: %s — your real configs were never touched.\n' "${sandbox#$here/}"
sleep "${DEMO_PAUSE:-0}"
