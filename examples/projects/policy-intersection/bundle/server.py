#!/usr/bin/env python3
"""A tiny, honest stdio MCP server — the "opsbox" fixture for the policy demo.

It advertises four tools that span the safety spectrum:

  * get_status    — read-only, harmless
  * list_items    — read-only, harmless
  * delete_everything — destructive
  * admin_reset   — privileged

Every tool just echoes back "<name>: ok"; nothing here is actually dangerous.
The point is not what the server *does* — it is what the AgentStack gateway
lets an agent *reach*. The project manifest that ships this server tries to
allow `delete_everything`; the machine policy floor denies anything matching
`delete_*` / `admin_*` on every server. The demo proves the intersection wins:
the repo cannot widen its own blast radius past the machine's floor.
"""
import json
import sys

TOOLS = [
    {
        "name": "get_status",
        "description": "Report ops box status",
        "inputSchema": {"type": "object"},
    },
    {
        "name": "list_items",
        "description": "List managed items",
        "inputSchema": {"type": "object"},
    },
    {
        # The repo's manifest tries to allow this one; the machine floor denies it.
        "name": "delete_everything",
        "description": "Permanently wipe every managed item",
        "inputSchema": {"type": "object"},
    },
    {
        # Privileged; the machine floor denies `admin_*` on every server.
        "name": "admin_reset",
        "description": "Reset the admin account and rotate its credentials",
        "inputSchema": {"type": "object"},
    },
]


def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n")
    sys.stdout.flush()


for line in sys.stdin:
    if not line.strip():
        continue
    try:
        msg = json.loads(line)
    except Exception:
        continue
    method, rid = msg.get("method"), msg.get("id")
    if method == "initialize":
        send(
            {
                "jsonrpc": "2.0",
                "id": rid,
                "result": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "opsbox", "version": "1.0"},
                },
            }
        )
    elif method == "tools/list":
        send({"jsonrpc": "2.0", "id": rid, "result": {"tools": TOOLS}})
    elif method == "tools/call":
        params = msg.get("params") or {}
        name = params.get("name", "?")
        send(
            {
                "jsonrpc": "2.0",
                "id": rid,
                "result": {"content": [{"type": "text", "text": f"{name}: ok"}]},
            }
        )
    elif rid is not None:
        send(
            {
                "jsonrpc": "2.0",
                "id": rid,
                "error": {"code": -32601, "message": "method not found"},
            }
        )
