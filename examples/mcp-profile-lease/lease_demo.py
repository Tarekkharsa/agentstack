#!/usr/bin/env python3
"""Drive one real AgentStack MCP stdio process through a profile lease."""

import json
import subprocess
import sys


def request(request_id, name, arguments):
    return {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": "tools/call",
        "params": {"name": name, "arguments": arguments},
    }


def result_text(response):
    if "error" in response:
        raise RuntimeError(response["error"])
    return json.loads(response["result"]["content"][0]["text"])


def main():
    if len(sys.argv) != 3:
        raise SystemExit("usage: lease_demo.py AGENTSTACK_BIN PROJECT_DIR")

    agentstack, project = sys.argv[1:]
    messages = [
        {"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {}},
        request(2, "agentstack_lease_open", {"profile": "backend"}),
        request(3, "agentstack_list_loadable", {}),
        request(
            4,
            "agentstack_load",
            {"name": "review-checklist", "reason": "review the backend change"},
        ),
        request(5, "agentstack_lease_status", {}),
        request(6, "agentstack_lease_freeze", {"name": "backend-observed"}),
        request(7, "agentstack_lease_close", {}),
    ]
    payload = "".join(json.dumps(message) + "\n" for message in messages)
    completed = subprocess.run(
        [agentstack, "mcp", "--manifest-dir", project],
        input=payload,
        text=True,
        capture_output=True,
        check=True,
    )
    responses = [json.loads(line) for line in completed.stdout.splitlines()]
    if len(responses) != len(messages):
        raise RuntimeError(f"expected {len(messages)} responses, got {len(responses)}")

    opened = result_text(responses[1])
    loadable = result_text(responses[2])
    loaded = result_text(responses[3])
    status = result_text(responses[4])
    frozen_text = responses[5]["result"]["content"][0]["text"]
    closed = result_text(responses[6])

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
