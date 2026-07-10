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

# A nonzero CLI exit is a FAILURE unless it matches this auth/onboarding
# allowlist — the rot alarm must never classify unknown breakage as a skip.
classify_cli_failure() {
  # Contextual phrases only: bare tokens like `auth`, `credential`, `api_key`,
  # or `browser` also appear in CONFIG and crash errors ("invalid auth mode in
  # config.toml"), which must FAIL — the self-test pins those as negatives.
  if grep -qiE 'authentication required|not logged in|logged out|please sign in|sign in to continue|log ?in required|run [a-z. -]+(login|auth)|missing [a-z_ ]{0,40}api.?key|set [a-z_ ]{0,40}api.?key|api.?key (required|not set|is missing)|unauthorized|onboarding|open [a-z ]{0,40}browser to|session expired|token expired' <<<"$1"; then
    echo skip
  else
    echo fail
  fi
}

if [[ "${1:-}" == "--self-test" ]]; then
  # Regression tests for both classes: auth gates skip, everything else fails.
  [[ "$(classify_cli_failure "Please sign in to continue")" == skip ]]
  [[ "$(classify_cli_failure "error: not logged in - run codex login first")" == skip ]]
  [[ "$(classify_cli_failure "Set the OPENAI_API_KEY environment variable")" == skip ]]
  [[ "$(classify_cli_failure 'unknown field `mcp_server` at line 4')" == fail ]]
  [[ "$(classify_cli_failure "failed to load configuration from config.toml")" == fail ]]
  [[ "$(classify_cli_failure "TOML parse error at line 2, column 1")" == fail ]]
  [[ "$(classify_cli_failure "segmentation fault")" == fail ]]
  # Negative cases: bare auth-adjacent TOKENS inside config/crash errors must
  # NOT be classified as auth gates (the round-5 false positives).
  [[ "$(classify_cli_failure "invalid auth mode in config.toml")" == fail ]]
  [[ "$(classify_cli_failure "unknown credential field in MCP config")" == fail ]]
  [[ "$(classify_cli_failure "invalid api_key value")" == fail ]]
  [[ "$(classify_cli_failure "browser configuration crashed")" == fail ]]
  echo "classify self-test OK"
  exit 0
fi

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
  # Allowlist inversion: only a recognized auth/onboarding gate is a skip;
  # every other nonzero exit — parse errors, unknown fields, crashes, and
  # wording we have never seen — FAILS. A rot alarm must fail unknown.
  if [[ "$(classify_cli_failure "$out")" == skip ]]; then
    echo "live: SKIPPED — '$cli_bin mcp list' hit an auth/onboarding gate. Output:"
    echo "$out" | head -20
  else
    echo "FAIL: $cli_bin exited nonzero and the output matches no known auth gate:"
    echo "$out" | head -20
    exit 1
  fi
fi
echo "Done."
