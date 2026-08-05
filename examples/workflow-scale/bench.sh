#!/usr/bin/env bash
# The workflow scaling bench — Phase 0.
#
# Sweeps WIDTH x CONCURRENCY against a latency-controlled mock harness in a
# fenced AGENTSTACK_HOME, and reports for each cell: wall clock, the throughput
# floor (total child time / concurrency), the drive loop's efficiency against
# that floor, and the straggler ratio.
#
# Two variables, because they interact and reporting either alone is
# misleading: removing the batch barrier buys little while concurrency is the
# binding constraint, and raising concurrency buys little while the barrier
# serialises the stages. The sweep is what tells them apart.
#
#   ./bench.sh                      # default sweep
#   WIDTHS="5 25" CONC="4 16" ./bench.sh
#   SCALE_FLAT=1 ./bench.sh         # control arm: no straggler tail
#
# Requires: a built `agentstack` on PATH (or AGENTSTACK_BIN), python3.
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd)"
BIN="${AGENTSTACK_BIN:-agentstack}"
WIDTHS="${WIDTHS:-5 25 100}"
CONC="${CONC:-4 16}"

command -v python3 >/dev/null || { echo "python3 is required" >&2; exit 1; }
command -v "$BIN"  >/dev/null || { echo "agentstack not on PATH (set AGENTSTACK_BIN)" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

export AGENTSTACK_HOME="$WORK/home"
mkdir -p "$AGENTSTACK_HOME"
# The mock harness shadows the real `claude` for the whole bench.
export PATH="$HERE/mock:$PATH"
chmod +x "$HERE/mock/claude"

PROJ="$WORK/proj"
cp -R "$HERE/bundle" "$PROJ"

echo "▶ bench workspace: $WORK"
echo "▶ harness: mock (SCALE_FLAT=${SCALE_FLAT:-0}, fast=${SCALE_FAST_MS:-200}ms med=${SCALE_MED_MS:-800}ms slow=${SCALE_SLOW_MS:-4000}ms)"

# --- admission: lock, then trust the pinned bytes -------------------------
( cd "$PROJ" && "$BIN" lock --write >/dev/null )
# `awk NR==1`, not `head -1`: head exits at the first line, SIGPIPEs the
# writers behind it, and `set -o pipefail` turns the 141 into an abort as soon
# as the preview grows past one pipe buffer. awk drains its input.
CONSENT="$(cd "$PROJ" && "$BIN" trust . --preview \
  | sed -n 's/.*"surface_digest": "\([^"]*\)".*/\1/p' | awk 'NR==1')"
[ -n "$CONSENT" ] || { echo "could not read the trust preview digest" >&2; exit 1; }
( cd "$PROJ" && "$BIN" trust . --yes --consented-digest "$CONSENT" >/dev/null )
echo "▶ admitted: workflow pinned and trusted"
echo

RESULTS="$WORK/results.jsonl"
: > "$RESULTS"

# On the missing "ungoverned bookend": an xargs arm at the same width is NOT a
# fair denominator, because the mock draws its latency from a checksum of the
# prompt and the bookend's prompts are not the workflow's prompts — the two arms
# run different latency multisets and the comparison reads as noise (an early
# draft of this script reported -41% "overhead", i.e. the bookend was slower).
# The fair floor is per-run and already available: sum(this run's own child
# durations) / concurrency is the best any scheduler could do with exactly these
# children, so `efficiency` isolates drive-loop cost with no second arm needed.
# Real-model governance overhead stays the acceptance rig's job
# (examples/workflow-acceptance), which measures it against a real bookend.
for width in $WIDTHS; do
  for conc in $CONC; do
    # Machine-owned concurrency cap. Written per cell so the sweep is driven by
    # the real policy path rather than a test-only override.
    cat > "$AGENTSTACK_HOME/agentstack.toml" <<EOF
version = 1

[policy.workflows]
max_concurrent = $conc
EOF

    ( cd "$PROJ" && "$BIN" workflow run scale-bench \
        --args-json "{\"width\":$width}" >/dev/null 2>"$WORK/run.err" ) || {
      echo "  ✗ width=$width conc=$conc FAILED — tail of stderr:" >&2
      tail -5 "$WORK/run.err" >&2
      continue
    }

    run_id="$("$BIN" workflow runs --json \
      | python3 -c 'import json,sys;print(json.load(sys.stdin)["runs"][0]["run"])')"
    "$BIN" workflow report "$run_id" --json > "$WORK/report.json"
    python3 "$HERE/analyze.py" "$WORK/report.json" "$width" "$conc" >> "$RESULTS"
  done
done

echo
echo "▶ summary"
python3 - "$RESULTS" <<'PY'
import json, sys
rows = [json.loads(l) for l in open(sys.argv[1]) if l.strip()]
if not rows:
    print("  no successful cells"); raise SystemExit(1)
hdr = f"{'width':>6} {'conc':>5} {'wall_s':>8} {'ideal_s':>8} {'effic':>6} {'strag':>6} {'steps':>9}"
print(hdr); print("  " + "-" * (len(hdr) - 2))
for r in rows:
    print(f"{r['width']:>6} {r['concurrency']:>5} {r['wall_s']:>8} {r['ideal_wall_s']:>8} "
          f"{r['efficiency']:>6} {r['straggler_ratio']:>6} "
          f"{str(r['steps_completed'])+'/'+str(r['steps']):>9}")
print()
print("  efficiency = ideal_wall / actual_wall. Below ~0.8 means the drive loop")
print("  is leaving worker capacity idle — the batch barrier is the first suspect.")
PY

cp "$RESULTS" "$HERE/results.jsonl" 2>/dev/null || true
echo
echo "▶ raw rows copied to examples/workflow-scale/results.jsonl"
