#!/usr/bin/env bash
# Evidence assertions for the mapreduce-acceptance workflow.
# Checks the recorder output that ALREADY ships (run dirs + events.jsonl),
# so it works against any engine stage — and against the interim courier
# path — unchanged.
#
# Usage:
#   ls ~/.agentstack/runs > /tmp/runs-before.txt     # BEFORE the workflow
#   <run the workflow>
#   ./check-evidence.sh /tmp/runs-before.txt <rig-dir> [expected-children]
#
# Asserts:
#   1. exactly N (default 5) new child runs exist
#   2. every child passed all four gates (trust, locked-verify,
#      rendered-verify, policy-admission) and completed with exit 0
#   3. every child has a DISTINCT frozen grant digest
#   4. the 3 earliest-starting children (the map fan-out) overlapped in
#      wall-clock — a common window with all three simultaneously live
#   5. the rig's .mcp.json is absent (or byte-identical if a before-sha
#      file <rig-dir>/.mcp.sha256 was snapshotted) — children shared no config
set -euo pipefail

BEFORE="${1:?usage: check-evidence.sh <runs-before.txt> <rig-dir> [expected-children]}"
RIG="${2:?usage: check-evidence.sh <runs-before.txt> <rig-dir> [expected-children]}"
EXPECTED="${3:-5}"
RUNS_DIR="${AGENTSTACK_HOME:-$HOME/.agentstack}/runs"

NEW_IDS=$(comm -13 <(sort "$BEFORE") <(ls "$RUNS_DIR" | sort))
COUNT=$(echo "$NEW_IDS" | grep -c . || true)
if [ "$COUNT" -ne "$EXPECTED" ]; then
  echo "FAIL: expected $EXPECTED new child runs, found $COUNT:" >&2
  echo "$NEW_IDS" >&2
  exit 1
fi
echo "ok: $COUNT new child runs"

# ids travel via env, not stdin — the heredoc already owns stdin
NEW_IDS="$NEW_IDS" RUNS_DIR="$RUNS_DIR" EXPECTED="$EXPECTED" python3 - <<'PY'
import json, os, sys

runs_dir = os.environ["RUNS_DIR"]
ids = [l.strip() for l in os.environ["NEW_IDS"].splitlines() if l.strip()]
assert len(ids) == int(os.environ["EXPECTED"]), f"id list lost in transit: {ids}"
GATES = {"trust", "locked-verify", "rendered-verify", "policy-admission"}
grants, windows = {}, {}

for rid in ids:
    gates_passed, start, end, outcome, exit_code, grant = set(), None, None, None, None, None
    with open(os.path.join(runs_dir, rid, "events.jsonl")) as f:
        for line in f:
            e = json.loads(line)
            kind = e.get("event")
            if kind == "attempt_started":
                start = e["ts"]
            elif kind == "gate_decision" and e.get("passed"):
                gates_passed.add(e["gate"])
            elif kind == "grant_frozen":
                grant = e["grant_digest"]
            elif kind == "locked_outcome":
                end, outcome, exit_code = e["ts"], e["outcome"], e.get("exit_code")
    missing = GATES - gates_passed
    assert not missing, f"{rid}: gates not passed: {missing}"
    assert outcome == "completed" and exit_code == 0, f"{rid}: outcome={outcome} exit={exit_code}"
    assert grant, f"{rid}: no grant_frozen event"
    grants[rid] = grant
    windows[rid] = (start, end)
    print(f"ok: {rid} gates green, exit 0, grant {grant[7:19]}…")

assert len(set(grants.values())) == len(ids), f"grant digests not distinct: {grants}"
print(f"ok: {len(ids)} distinct grant digests (per-run identity)")

# Map overlap: the 3 earliest starters must share a live window.
trio = sorted(windows.values())[:3]
latest_start = max(w[0] for w in trio)
earliest_end = min(w[1] for w in trio)
assert latest_start < earliest_end, f"map children did not overlap: {trio}"
print(f"ok: 3-way map overlap, common window {earliest_end - latest_start}s")
PY

if [ -f "$RIG/.mcp.json" ]; then
  if [ -f "$RIG/.mcp.sha256" ] && shasum -a 256 -c "$RIG/.mcp.sha256" >/dev/null 2>&1; then
    echo "ok: .mcp.json present but byte-identical to snapshot"
  else
    echo "FAIL: rig .mcp.json exists and does not match a snapshot — a child touched shared config" >&2
    exit 1
  fi
else
  echo "ok: rig .mcp.json absent throughout"
fi

echo "PASS: all evidence assertions hold"
