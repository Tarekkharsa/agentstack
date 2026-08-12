#!/usr/bin/env python3
"""Drive one real AgentStack MCP stdio process through a profile lease."""

import json
import subprocess
import sys


def result_text(response):
    if "error" in response:
        raise RuntimeError(response["error"])
    text = response["result"]["content"][0]["text"]
    if response["result"].get("isError"):
        raise RuntimeError(text)
    return json.loads(text)


class Session:
    """One stdio MCP connection, driven strictly one request at a time.

    Sequential is load-bearing, not tidiness: the server handles requests
    CONCURRENTLY, so a whole script poured into stdin at once comes back out
    of order — `lease_status` can be answered before `lease_open` has run, and
    the demo would read a lease it had not opened yet. Each call below writes
    one line and waits for the reply carrying its own id.
    """

    def __init__(self, argv, cwd=None):
        self.proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
            cwd=cwd,
        )
        self.next_id = 0

    def notify(self, method):
        self._send({"jsonrpc": "2.0", "method": method})

    def call(self, message):
        self.next_id += 1
        message = dict(message, jsonrpc="2.0", id=self.next_id)
        self._send(message)
        while True:
            line = self.proc.stdout.readline()
            if not line:
                self.proc.wait()
                raise SystemExit(
                    f"agentstack mcp closed its stdout before answering "
                    f"{message['method']} (exit {self.proc.returncode})"
                )
            try:
                response = json.loads(line)
            except ValueError:
                continue
            if isinstance(response, dict) and response.get("id") == message["id"]:
                return response

    def tool(self, name, arguments):
        return self.call(
            {"method": "tools/call", "params": {"name": name, "arguments": arguments}}
        )

    def _send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def close(self):
        self.proc.stdin.close()
        code = self.proc.wait()
        if code != 0:
            raise SystemExit(f"agentstack mcp exited {code}")


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: lease_demo.py AGENTSTACK_BIN PROJECT_DIR")

    agentstack, project = sys.argv[1:]
    # A real MCP handshake, not a stub: the server speaks the protocol proper,
    # so `initialize` must carry protocolVersion/capabilities/clientInfo and be
    # followed by the `notifications/initialized` notification before any
    # tools/call is accepted. The notification has no id and gets no response,
    # which is why the replies below are matched BY ID rather than by position.
    #
    # The version is load-bearing, and it must be one the server actually
    # supports: an unknown string (the old "2024-11-05") is not negotiated down
    # — the handshake settles on the server's LATEST instead, and a modern
    # connection refuses connection-scoped toolset leases by design ("a modern
    # MCP connection re-derives the trusted default on each request", see
    # docs/concepts.md). This demo's whole subject is the connection lease, so
    # it speaks the era in which that lease exists.
    session = Session([agentstack, "mcp", "--manifest-dir", project])
    session.call(
        {
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": {"name": "lease-demo", "version": "0"},
            },
        }
    )
    session.notify("notifications/initialized")

    opened = result_text(session.tool("agentstack_lease_open", {"profile": "backend"}))
    loadable = result_text(session.tool("agentstack_list_loadable", {}))
    loaded = result_text(
        session.tool(
            "agentstack_load",
            {"name": "review-checklist", "reason": "review the backend change"},
        )
    )
    status = result_text(session.tool("agentstack_lease_status", {}))
    frozen_text = session.tool(
        "agentstack_lease_freeze", {"name": "backend-observed"}
    )["result"]["content"][0]["text"]
    closed = result_text(session.tool("agentstack_lease_close", {}))
    session.close()

    assert opened["opened"] == "backend"
    assert opened["native_files_written"] is False
    assert any(skill["name"] == "review-checklist" for skill in loadable["loadable"])
    assert loaded["loaded"] == "review-checklist"
    assert loaded["newly_loaded"] is True
    assert status["profile"] == "backend"
    assert [entry["name"] for entry in status["loads"]] == ["review-checklist"]
    assert "backend-observed" in frozen_text
    assert "agentstack lock" in frozen_text
    assert closed["closed"] == "backend"
    assert closed["native_restore_needed"] is False

    print("PASS  opened backend lease without native files")
    print("PASS  discovered and loaded only the profile skill")
    print("PASS  recorded one in-memory load with its reason")
    print("PASS  froze the observed set into backend-observed")
    print("PASS  closed the lease without a restore")


if __name__ == "__main__":
    main()
