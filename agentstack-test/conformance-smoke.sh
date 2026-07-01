#!/usr/bin/env bash
# Conformance smoke: prove a *real* agent CLI still accepts the config
# agentstack renders for it. This is the adapter-rot alarm — golden snapshots
# lock what we write; this checks the CLI on the other end still reads it.
# Fully fenced HOME: never touches your real configs.
#
#   ./conformance-smoke.sh <adapter-id> <cli-binary> <config-path-under-home>
#   ./conformance-smoke.sh claude-code claude .claude.json
#   ./conformance-smoke.sh codex codex .codex/config.toml
#   ./conformance-smoke.sh gemini gemini .gemini/settings.json
set -euo pipefail
adapter="$1"
cli_bin="$2"
config_rel="$3"
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

if [[ -z "${AGENTSTACK_BIN:-}" ]]; then
  (cd .. && cargo build --quiet)
fi
bin="${AGENTSTACK_BIN:-$here/../target/debug/agentstack}"

sandbox="$here/runtime/conformance-$adapter"
home="$sandbox/home"
proj="$sandbox/proj"
rm -rf "$sandbox"
mkdir -p "$home" "$proj/.agentstack"

# Minimal secret-free manifest: one stdio + one http server, this adapter only.
cat > "$proj/.agentstack/agentstack.toml" <<TOML
version = 1

[servers.conformance_probe]
type = "stdio"
command = "echo"
args = ["conformance"]

[servers.conformance_http]
type = "http"
url = "https://example.com/mcp"

[targets]
default = ["$adapter"]
TOML

as() { env HOME="$home" AGENTSTACK_HOME="$sandbox/ashome" "$bin" "$@"; }
(cd "$proj" && as apply --write)

config="$home/$config_rel"
if [[ ! -f "$config" ]]; then
  echo "FAIL: apply --write did not create $config"
  exit 1
fi
if ! grep -q conformance_probe "$config"; then
  echo "FAIL: rendered config lacks the probe server:"
  cat "$config"
  exit 1
fi

# Structural: the file must parse in its native format.
case "$config" in
  *.toml)
    python3 -c 'import sys, tomllib; tomllib.load(open(sys.argv[1], "rb"))' "$config"
    ;;
  *)
    python3 -m json.tool "$config" >/dev/null
    ;;
esac
echo "structural: OK — $config_rel parses and contains the probe server"

# Live: ask the real CLI to read its own config. Strongest signal, but some
# CLIs refuse to run unauthenticated — degrade to structural-only, loudly.
if ! command -v "$cli_bin" >/dev/null; then
  echo "live: SKIPPED — $cli_bin not on PATH"
  echo "Done."
  exit 0
fi
if out="$(env HOME="$home" "$cli_bin" mcp list 2>&1)"; then
  if grep -q conformance_probe <<<"$out"; then
    echo "live: OK — '$cli_bin mcp list' sees the probe server"
  else
    echo "FAIL: $cli_bin ran but does not see the probe server. Output:"
    echo "$out"
    exit 1
  fi
else
  echo "live: SKIPPED — '$cli_bin mcp list' exited nonzero (auth/onboarding gate?). Output:"
  echo "$out" | head -20
fi
echo "Done."
