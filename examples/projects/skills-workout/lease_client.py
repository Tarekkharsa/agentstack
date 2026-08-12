#!/usr/bin/env python3
"""Drive one real `agentstack mcp` stdio process through a `docs` profile lease.

Usage: lease_client.py AGENTSTACK_BIN PROJECT_DIR OUT_DIR

Writes machine-readable artifacts into OUT_DIR for assert.sh to check:
  opened.json          - the lease_open result
  loadable.txt         - one loadable skill name per line
  loaded-<name>.txt    - the exact `instructions` bytes returned by load
  refused.txt          - the text returned when loading a non-profile skill
  status.json          - the lease_status result (the load trail)
  close.json           - the lease_close result

This file is intentionally dumb: it makes the calls and records what came back.
Every PASS/FAIL judgement lives in assert.sh so the counters stay in one place.
"""

import json
import os
import subprocess
import sys


class Session:
    """One stdio MCP connection, driven strictly one request at a time.

    Two facts about the real wire make this shape mandatory:
      · the server answers requests CONCURRENTLY, so a whole script poured into
        stdin comes back out of order — `lease_status` could be answered before
        the loads it is supposed to have recorded,
      · the handshake is the real one: `initialize` must carry
        protocolVersion/capabilities/clientInfo, and `notifications/initialized`
        must follow it before any tools/call is accepted.
    """

    def __init__(self, argv):
        self.proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            text=True,
        )
        self.next_id = 0

    def notify(self, method):
        self._send({"jsonrpc": "2.0", "method": method})

    def request(self, method, params):
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
                message = json.loads(line)
            except ValueError:
                continue
            if isinstance(message, dict) and message.get("id") == rid:
                return message

    def call(self, name, arguments):
        return self.request("tools/call", {"name": name, "arguments": arguments})

    def _send(self, message):
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()

    def close(self):
        self.proc.stdin.close()
        code = self.proc.wait()
        if code != 0:
            raise SystemExit(f"agentstack mcp exited {code}")


def response_text(response):
    """Return the human/JSON text a response carried, whether ok or an error."""
    if "error" in response:
        return json.dumps(response["error"])
    content = response["result"]["content"][0]["text"]
    return content


def main():
    if len(sys.argv) != 4:
        raise SystemExit("usage: lease_client.py AGENTSTACK_BIN PROJECT_DIR OUT_DIR")
    agentstack, project, out = sys.argv[1:]
    os.makedirs(out, exist_ok=True)

    session = Session([agentstack, "mcp", "--manifest-dir", project])
    # The version is load-bearing. Connection-scoped toolset leases are
    # legacy-only (docs/concepts.md): a modern connection re-derives the
    # trusted default on every request and refuses to fence one. An
    # unrecognized version string is NOT negotiated down — the handshake
    # settles on the server's latest, i.e. the modern era — so this client asks
    # for a version the server knows, in the era this lane's lease exists.
    session.request(
        "initialize",
        {
            "protocolVersion": "2025-11-25",
            "capabilities": {},
            "clientInfo": {"name": "skills-workout", "version": "0"},
        },
    )
    session.notify("notifications/initialized")

    opened_response = session.call("agentstack_lease_open", {"profile": "docs"})
    loadable_response = session.call("agentstack_list_loadable", {})
    load_responses = {
        "api-conventions": session.call(
            "agentstack_load",
            {"name": "api-conventions", "reason": "design a new endpoint"},
        ),
        "sql-review": session.call(
            "agentstack_load", {"name": "sql-review", "reason": "review a migration"}
        ),
    }
    # release-checklist is a real manifest skill but is NOT in the docs
    # profile — the fence must refuse it.
    refused_response = session.call(
        "agentstack_load",
        {"name": "release-checklist", "reason": "attempt to escape the fence"},
    )
    status_response = session.call("agentstack_lease_status", {})
    close_response = session.call("agentstack_lease_close", {})
    session.close()

    # lease_open
    opened = json.loads(response_text(opened_response))
    with open(os.path.join(out, "opened.json"), "w") as fh:
        json.dump(opened, fh)

    # list_loadable — record just the names, one per line
    loadable = json.loads(response_text(loadable_response))
    names = [entry["name"] for entry in loadable["loadable"]]
    with open(os.path.join(out, "loadable.txt"), "w") as fh:
        fh.write("\n".join(names) + "\n")

    # the two in-profile loads — write the returned `instructions` bytes verbatim
    for name, response in load_responses.items():
        loaded = json.loads(response_text(response))
        with open(os.path.join(out, f"loaded-{name}.txt"), "w") as fh:
            fh.write(loaded["instructions"])
        with open(os.path.join(out, f"loaded-{name}.origin"), "w") as fh:
            fh.write(loaded.get("origin", ""))

    # the fenced-out load — record whatever text came back (error or content)
    with open(os.path.join(out, "refused.txt"), "w") as fh:
        fh.write(response_text(refused_response))

    # lease_status — the load trail (names + reasons)
    with open(os.path.join(out, "status.json"), "w") as fh:
        fh.write(response_text(status_response))

    # lease_close
    with open(os.path.join(out, "close.json"), "w") as fh:
        fh.write(response_text(close_response))


if __name__ == "__main__":
    main()
