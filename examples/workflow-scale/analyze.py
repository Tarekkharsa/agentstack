#!/usr/bin/env python3
"""Turn one `agentstack workflow report <id> --json` payload into the scaling
numbers the plan's success table is written against.

Deliberately reads only the SHIPPED report shape — per-step `duration_ms` and
top-level `duration_ms` are already recorded, so Phase 0 needs no new CLI code.
If a future phase adds fields, this stays forward-compatible by using .get().

Usage:  analyze.py <report.json> <width> <concurrency> [ungoverned_wall_s]
Emits one JSON object on stdout (machine-readable) and a human line on stderr.
"""

import json
import statistics
import sys


def main() -> int:
    if len(sys.argv) < 4:
        print(__doc__, file=sys.stderr)
        return 2

    report_path, width, concurrency = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
    ungoverned = float(sys.argv[4]) if len(sys.argv) > 4 else None

    with open(report_path) as fh:
        report = json.load(fh)

    steps = report.get("steps", [])
    durations = [s["duration_ms"] for s in steps if s.get("duration_ms") is not None]

    wall_ms = report.get("duration_ms")
    if wall_ms is None:
        print("report has no terminal duration — the run did not finish", file=sys.stderr)
        return 1

    # The straggler ratio is the whole reason the batch barrier matters: with a
    # barrier, each stage pays its own max; without one, the run pays the tail
    # once. A ratio near 1.0 means the latency distribution is flat and the
    # barrier costs nothing — which is exactly why SCALE_FLAT exists as a
    # control arm rather than as the default.
    straggler = (max(durations) / statistics.median(durations)) if durations else None

    # Throughput floor: with C workers and this exact set of child latencies,
    # no scheduler can finish faster than the total work divided by C. The gap
    # between that and the measured wall is what the drive loop costs — barrier
    # stalls, evidence appends, engine slices, process spawn.
    total_child_ms = sum(durations)
    ideal_ms = total_child_ms / concurrency if concurrency else None
    efficiency = (ideal_ms / wall_ms) if (ideal_ms and wall_ms) else None

    out = {
        "run": report.get("run"),
        "width": width,
        "concurrency": concurrency,
        "outcome": report.get("outcome"),
        "steps": len(steps),
        "steps_completed": sum(1 for s in steps if s.get("state") == "completed"),
        "wall_s": round(wall_ms / 1000, 2),
        "child_total_s": round(total_child_ms / 1000, 2),
        "child_median_ms": round(statistics.median(durations)) if durations else None,
        "child_max_ms": max(durations) if durations else None,
        "straggler_ratio": round(straggler, 2) if straggler else None,
        "ideal_wall_s": round(ideal_ms / 1000, 2) if ideal_ms else None,
        # <1.0 means the drive loop is leaving capacity idle. This is the single
        # number Phase 1 has to move.
        "efficiency": round(efficiency, 3) if efficiency else None,
    }
    if ungoverned is not None:
        out["ungoverned_wall_s"] = round(ungoverned, 2)
        out["governance_overhead_pct"] = round(
            (out["wall_s"] - ungoverned) / ungoverned * 100, 1
        )

    print(json.dumps(out))
    print(
        f"  width={width:>4} conc={concurrency:>3}  wall={out['wall_s']:>7}s  "
        f"ideal={out['ideal_wall_s']}s  efficiency={out['efficiency']}  "
        f"straggler={out['straggler_ratio']}  steps={out['steps_completed']}/{out['steps']}",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
