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

Two properties of the wire are load-bearing here, and both are why this file
drives the session request-by-request instead of pouring a script into stdin:

  · the server answers requests CONCURRENTLY, so a pipelined script comes back
    out of order — the "before any lease" calls could be answered after the
    lease had already opened, and the fence would look broken (or, worse, look
    fine when it was not),
  · the negotiated protocol version decides the era. Connection-scoped toolset
    leases are legacy-only (docs/concepts.md): a modern connection re-derives
    the trusted default on every request and refuses to fence one. An
    unrecognized version string — the old "2024-11-05" — is NOT negotiated
    down; the handshake settles on the server's latest, which is the modern
    era. This probe's subject is the leased fence, so it asks for a version the
    server actually knows.

It prints one JSON object on stdout: {name: [is_error, text]} for each call, so
assert.sh can make exact assertions about the fence, discovery filtering and
refusals. Newline-delimited JSON-RPC over stdin, one response per line — the
same wire protocol as examples/mcp-profile-lease/lease_demo.py.
"""
import json
import subprocess
import sys


class Session:
    """One stdio MCP connection, driven strictly one request at a time."""

    def __init__(self, argv, cwd=None):
        self.proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.DEVNULL,
            text=True,
            cwd=cwd,
        )
        self.next_id = 0

    def notify(self, method):
        self._send({"jsonrpc": "2.0", "method": method})

    def call(self, method, params):
        self.next_id += 1
        rid = self.next_id
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        while True:
            line = self.proc.stdout.readline()
            if not line:
                self.proc.wait()
                raise SystemExit(
                    f"agentstack mcp closed its stdout before answering {method} "
                    f"(exit {self.proc.returncode})"
                )
            try:
                response = json.loads(line)
            except ValueError:
                continue
            if isinstance(response, dict) and response.get("id") == rid:
                return response

    def tool(self, name, args):
        return self.call("tools/call", {"name": name, "arguments": args})

    def _send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def close(self):
        self.proc.stdin.close()
        self.proc.wait()


def outcome(response):
    if "error" in response:
        return [True, "TRANSPORT_ERROR: " + json.dumps(response["error"])]
    result = response.get("result", {})
    content = result.get("content") or [{}]
    return [bool(result.get("isError")), content[0].get("text", "")]


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: gateway_probe.py AGENTSTACK_BIN PROJECT_DIR")
    agentstack, project = sys.argv[1], sys.argv[2]

    session = Session([agentstack, "mcp", "--auto-project"], cwd=project)
    session.call(
        "initialize",
        {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "probe", "version": "0"},
        },
    )
    session.notify("notifications/initialized")

    broad = {"query": "status items delete admin reset everything"}
    results = {
        "fenced_search": outcome(session.tool("tools_search", broad)),
        "fenced_call": outcome(session.tool("opsbox__get_status", {})),
        "lease": outcome(session.tool("agentstack_lease_open", {"profile": "default"})),
        "search": outcome(session.tool("tools_search", broad)),
        "get_status": outcome(session.tool("opsbox__get_status", {})),
        "delete_everything": outcome(session.tool("opsbox__delete_everything", {})),
        "admin_reset": outcome(session.tool("opsbox__admin_reset", {})),
    }
    session.close()

    print(json.dumps(results))


if __name__ == "__main__":
    main()
