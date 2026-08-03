#!/usr/bin/env python3
"""Drive the AgentStack gateway through ONE stdio session and print the results.

Usage: gateway_probe.py AGENTSTACK_BIN PROJECT_DIR

It opens a single `agentstack mcp --auto-project` gateway (project discovered by
cwd walk-up), then in that one session it:

  1. calls `tools_search` and `opsbox__get_status` BEFORE any lease — this
     project declares a toolset, so the fence is closed and the gateway offers
     its control plane only,
  2. opens a lease naming the `default` toolset,
  3. calls `tools_search` with a broad query,
  4. calls the proxied `opsbox__get_status`,
  5. calls the proxied `opsbox__delete_everything`,
  6. calls the proxied `opsbox__admin_reset`.

Steps 1 and 3-6 in ONE session on purpose: the same connection that saw nothing
sees the filtered set the moment a lease names the toolset, so the fence and the
policy intersection are proven against the same gateway state.

It prints one JSON object on stdout: {name: [is_error, text]} for each call, so
assert.sh can make exact assertions about the fence, discovery filtering and
refusals. Newline-delimited JSON-RPC over stdin, one response per line — the
same wire protocol as examples/mcp-profile-lease/lease_demo.py.
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
        call(4, "agentstack_lease_open", {"profile": "default"}),
        call(5, "tools_search", {"query": "status items delete admin reset everything"}),
        call(6, "opsbox__get_status", {}),
        call(7, "opsbox__delete_everything", {}),
        call(8, "opsbox__admin_reset", {}),
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
                "fenced_search": outcome(2),
                "fenced_call": outcome(3),
                "lease": outcome(4),
                "search": outcome(5),
                "get_status": outcome(6),
                "delete_everything": outcome(7),
                "admin_reset": outcome(8),
            }
        )
    )


if __name__ == "__main__":
    main()
