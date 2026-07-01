#!/usr/bin/env bash
# Central library demo — end to end, against the simulated machine in runtime/.
# Puts a server + skill into ~/.agentstack/lib, references them BY NAME from a
# project, activates the profile, and proves it landed in a real CLI config with
# the secret resolved at write time — while the lock keeps only the digest.
# Idempotent: re-run any time (uses --replace).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

# Build the debug binary the ./as wrapper uses.
( cd .. && source "$HOME/.cargo/env" 2>/dev/null || true; cargo build --quiet )

proj="projects/central-demo"        # manifest only — references by name
src="fixtures/central-library"      # demo INPUT fixtures, outside any project
line() { printf '\n\033[1m== %s ==\033[0m\n' "$1"; }

line "1. Put a server + skill into the CENTRAL LIBRARY (~/.agentstack/lib)"
./as lib add-server kibana --file "$src/kibana.toml" --replace --write
./as lib add sql-review --path "$src/sql-review" --replace --write

line "2. What's in the library now"
./as lib list

line "3. The project references them BY NAME — no inline definitions, no files"
cat "$proj/agentstack.toml"

line "4. Resolve the secret locally, then activate the profile (--write)"
export KIBANA_TOKEN="demo-secret-value"
./as --manifest-dir "$proj" use central --write

line "5. Explain the server: origin, provenance, lock status, secrets"
./as --manifest-dir "$proj" explain kibana

line "6. Proof: the library server landed in the simulated Claude config"
python3 - "$here/runtime/home/.claude.json" <<'PY' || cat "$here/runtime/home/.claude.json"
import json, sys
cfg = json.load(open(sys.argv[1]))
srv = cfg.get("mcpServers", {}).get("kibana")
print(json.dumps(srv, indent=2) if srv else "(kibana not found)")
PY

line "7. The lockfile pinned the DEFINITION digest — never the secret value"
grep -A2 "\[\[server\]\]" "$proj/agentstack.lock" 2>/dev/null || echo "(no lock written)"

# --- optional: reproducibility report -------------------------------------
# `doctor` also has a Reproducibility section that re-resolves each library ref
# and compares its digest to the lock. (Plain `doctor` additionally lists Drift
# for the just-written config, which is a separate, expected apply-time concern.)
line "Optional — reproducibility report (library items vs the lock)"
./as --manifest-dir "$proj" doctor 2>&1 | awk '/Reproducibility/{p=1} /Plugin recipes/{p=0} p' || true
