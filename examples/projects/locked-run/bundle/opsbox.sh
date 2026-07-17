#!/bin/sh
# Stand-in stdio MCP server. Never spawned by this example (the stub harness
# exits immediately, and the bridge spawns servers lazily) — it exists so
# `agentstack lock` pins its bytes (D3) and a one-byte edit refuses the run.
exit 0
