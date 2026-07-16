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


def call(request_id, name, arguments):
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }


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

    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        call(2, "agentstack_lease_open", {"profile": "docs"}),
        call(3, "agentstack_list_loadable", {}),
        call(4, "agentstack_load", {"name": "api-conventions",
                                    "reason": "design a new endpoint"}),
        call(5, "agentstack_load", {"name": "sql-review",
                                    "reason": "review a migration"}),
        # release-checklist is a real manifest skill but is NOT in the docs
        # profile — the fence must refuse it.
        call(6, "agentstack_load", {"name": "release-checklist",
                                    "reason": "attempt to escape the fence"}),
        call(7, "agentstack_lease_status", {}),
        call(8, "agentstack_lease_close", {}),
    ]
    payload = "".join(json.dumps(m) + "\n" for m in messages)
    completed = subprocess.run(
        [agentstack, "mcp", "--manifest-dir", project],
        input=payload,
        text=True,
        capture_output=True,
    )
    if completed.returncode != 0 and not completed.stdout.strip():
        sys.stderr.write(completed.stderr)
        raise SystemExit(f"agentstack mcp exited {completed.returncode} with no output")

    by_id = {}
    for line in completed.stdout.splitlines():
        if not line.strip():
            continue
        msg = json.loads(line)
        by_id[msg.get("id")] = msg

    # lease_open
    opened = json.loads(response_text(by_id[2]))
    with open(os.path.join(out, "opened.json"), "w") as fh:
        json.dump(opened, fh)

    # list_loadable — record just the names, one per line
    loadable = json.loads(response_text(by_id[3]))
    names = [entry["name"] for entry in loadable["loadable"]]
    with open(os.path.join(out, "loadable.txt"), "w") as fh:
        fh.write("\n".join(names) + "\n")

    # the two in-profile loads — write the returned `instructions` bytes verbatim
    for rid, name in ((4, "api-conventions"), (5, "sql-review")):
        loaded = json.loads(response_text(by_id[rid]))
        with open(os.path.join(out, f"loaded-{name}.txt"), "w") as fh:
            fh.write(loaded["instructions"])
        with open(os.path.join(out, f"loaded-{name}.origin"), "w") as fh:
            fh.write(loaded.get("origin", ""))

    # the fenced-out load — record whatever text came back (error or content)
    with open(os.path.join(out, "refused.txt"), "w") as fh:
        fh.write(response_text(by_id[6]))

    # lease_status — the load trail (names + reasons)
    with open(os.path.join(out, "status.json"), "w") as fh:
        fh.write(response_text(by_id[7]))

    # lease_close
    with open(os.path.join(out, "close.json"), "w") as fh:
        fh.write(response_text(by_id[8]))


if __name__ == "__main__":
    main()
