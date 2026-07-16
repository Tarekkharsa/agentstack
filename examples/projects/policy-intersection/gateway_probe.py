#!/usr/bin/env python3
"""Drive the AgentStack gateway through ONE stdio session and print the results.

Usage: gateway_probe.py AGENTSTACK_BIN PROJECT_DIR

It opens a single `agentstack mcp --auto-project` gateway (project discovered by
cwd walk-up), then in that one session it:

  1. calls `tools_search` with a broad query,
  2. calls the proxied `opsbox__get_status`,
  3. calls the proxied `opsbox__delete_everything`,
  4. calls the proxied `opsbox__admin_reset`.

It prints one JSON object on stdout: {name: [is_error, text]} for each call, so
assert.sh can make exact assertions about discovery filtering and refusals.
Newline-delimited JSON-RPC over stdin, one response per line — the same wire
protocol as examples/mcp-profile-lease/lease_demo.py.
"""
import json
import subprocess
import sys


def call(rid, name, args):
    return {
        "jsonrpc": "2.0",
        "id": rid,
        "method": "tools/call",
        "params": {"name": name, "arguments": args},
    }


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: gateway_probe.py AGENTSTACK_BIN PROJECT_DIR")
    agentstack, project = sys.argv[1], sys.argv[2]

    messages = [
        {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "probe", "version": "0"},
            },
        },
        {"jsonrpc": "2.0", "method": "notifications/initialized"},
        call(2, "tools_search", {"query": "status items delete admin reset everything"}),
        call(3, "opsbox__get_status", {}),
        call(4, "opsbox__delete_everything", {}),
        call(5, "opsbox__admin_reset", {}),
    ]
    payload = "".join(json.dumps(m) + "\n" for m in messages)
    completed = subprocess.run(
        [agentstack, "mcp", "--auto-project"],
        input=payload,
        text=True,
        capture_output=True,
        cwd=project,
    )

    by_id = {}
    for line in completed.stdout.splitlines():
        try:
            resp = json.loads(line)
        except Exception:
            continue
        if isinstance(resp, dict) and "id" in resp:
            by_id[resp["id"]] = resp

    def outcome(rid):
        resp = by_id.get(rid, {})
        if "error" in resp:
            return [True, "TRANSPORT_ERROR: " + json.dumps(resp["error"])]
        result = resp.get("result", {})
        content = result.get("content") or [{}]
        return [bool(result.get("isError")), content[0].get("text", "")]

    print(
        json.dumps(
            {
                "search": outcome(2),
                "get_status": outcome(3),
                "delete_everything": outcome(4),
                "admin_reset": outcome(5),
            }
        )
    )


if __name__ == "__main__":
    main()
