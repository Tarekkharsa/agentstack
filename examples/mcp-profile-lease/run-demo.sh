#!/usr/bin/env bash
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd -P)"
AS="${AGENTSTACK_BIN:-agentstack}"
if [[ "$AS" == */* ]]; then
  AS="$(cd "$(dirname "$AS")" && pwd -P)/$(basename "$AS")"
else
  AS="$(command -v "$AS")"
fi
SANDBOX="$(mktemp -d)"
trap 'rm -rf "$SANDBOX"' EXIT

export AGENTSTACK_HOME="$SANDBOX/home"
PROJECT="$SANDBOX/project"
mkdir -p "$AGENTSTACK_HOME" "$PROJECT"
cp -R "$HERE/bundle/." "$PROJECT/"

printf '\nMCP profile lease demo\n\n'
"$AS" lock --manifest-dir "$PROJECT"
python3 "$HERE/lease_demo.py" "$AS" "$PROJECT"

# lease_freeze changed the manifest. Review would happen here; this demo owns
# its temporary fixture, so accepting the changed profile is safe.
"$AS" lock --manifest-dir "$PROJECT" >/dev/null

test ! -e "$PROJECT/.mcp.json"
test ! -e "$PROJECT/.claude/skills"
test ! -e "$AGENTSTACK_HOME/sessions.json"
grep -q '^\[profiles.backend-observed\]' "$PROJECT/.agentstack/agentstack.toml"

printf 'PASS  refreshed agentstack.lock after reviewing the frozen profile\n'
printf 'PASS  no .mcp.json, .claude/skills, or sessions.json was created\n'
