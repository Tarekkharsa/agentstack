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
#   ./conformance-smoke.sh pi pi -          # no MCP by design: skills render instead
#
# Beyond the happy path, the MCP manifest carries a server named "slash/probe":
# legal in most CLIs, but Codex validates names at startup (^[a-zA-Z0-9_-]+$),
# so for codex the render must SKIP it with a spoken reason — writing it would
# error every Codex launch (the upstash/context7 failure, seen live).
set -euo pipefail

# A nonzero CLI exit is a FAILURE unless it matches this auth/onboarding
# allowlist — the rot alarm must never classify unknown breakage as a skip.
classify_cli_failure() {
  # Contextual phrases only: bare tokens like `auth`, `credential`, `api_key`,
  # or `browser` also appear in CONFIG and crash errors ("invalid auth mode in
  # config.toml"), which must FAIL — the self-test pins those as negatives.
  if grep -qiE 'authentication required|no authentication information|not logged in|logged out|please sign in|sign in to continue|log ?in required|run [a-z. -]+(login|auth)|missing [a-z_ ]{0,40}api.?key|set [a-z_ ]{0,40}api.?key|api.?key (required|not set|is missing)|unauthorized|onboarding|open [a-z ]{0,40}browser to|session expired|token expired' <<<"$1"; then
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
  # This harness tests the GLOBAL-scope render path: it checks the fenced
  # $HOME and probes the live CLI from outside $proj, where project-scope
  # artifacts are invisible. The default scope is context-derived (project
  # for a repo manifest), so every write here
  # must pin --scope global explicitly — assert the pins never get "cleaned
  # up" as redundant.
  grep -q 'as apply --scope global --write' "$0"
  grep -q 'as use default --scope global --write' "$0"
  # The rendered lane is what this alarm tests. Delivery sends MCP servers
  # live by default, so without the override `apply --write` writes nothing
  # and refuses — the smoke would have no file to hand the CLI.
  grep -q 'render_locally = true' "$0"
  # Both write paths run behind the trust gate; neither renders untrusted.
  # Two bare call sites (the skills leg and the MCP leg), one per write.
  [[ "$(grep -c '^ *trust_fenced_project$' "$0")" == 2 ]]
  # No capture may swallow its own failure (the 2026-08-11 silent-exit bug):
  # the apply capture must pair with `|| died`, which prints before exiting.
  grep -A1 'apply_out="\$(' "$0" | grep -q '|| died'
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
bin="${AGENTSTACK_BIN:-$here/../../target/debug/agentstack}"

sandbox="$here/runtime/conformance-$adapter"
home="$sandbox/home"
proj="$sandbox/proj"
rm -rf "$sandbox"
mkdir -p "$home" "$proj/.agentstack"

# Some CLIs (opencode) resolve their global config via the XDG Base Directory
# spec (XDG_CONFIG_HOME etc.) rather than deriving it from $HOME. If the
# ambient environment has one of those set, `env HOME=$home ...` alone does
# NOT fence such a CLI — it escapes the sandbox and reads/writes the real
# machine config, which for this fresh sandbox is empty ("No MCP servers
# configured"), producing a false FAIL. Strip the whole XDG family so HOME
# fencing is actually complete.
xdg_unset=(-u XDG_CONFIG_HOME -u XDG_DATA_HOME -u XDG_CACHE_HOME -u XDG_STATE_HOME -u XDG_RUNTIME_DIR)

as() { env "${xdg_unset[@]}" HOME="$home" AGENTSTACK_HOME="$sandbox/ashome" "$bin" "$@"; }

# A capture that dies silently is the worst failure this harness can have.
# `x="$(cmd)"` under `set -e` exits the script with everything the command
# said still sealed inside the variable: the 2026-08-11 nightly lost five jobs
# to exactly that — instant exit 1, zero stdout, nothing to read. Every
# capture below prints its output before it fails.
died() {
  printf '%s\n' "$2"
  echo "FAIL: $1 exited nonzero (its output is above)"
  exit 1
}

# The trust gate covers rendered servers AND materialized skills alike, so an
# untrusted project renders nothing — by design, and the fenced project here
# is no exception. This script wrote that manifest a few lines earlier, so
# consenting to it is honest. Consent without a TTY is deliberately two-step:
# read the exact surface, then present the digest of the bytes that were read.
trust_fenced_project() {
  local preview digest granted
  preview="$(cd "$proj" && as trust --preview 2>&1)" || died 'trust --preview' "$preview"
  digest="$(sed -n 's/.*"surface_digest"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' <<<"$preview")"
  if [[ -z "$digest" ]]; then
    printf '%s\n' "$preview"
    echo "FAIL: trust --preview emitted no surface_digest — cannot consent without a TTY"
    exit 1
  fi
  granted="$(cd "$proj" && as trust --yes --consented-digest "$digest" 2>&1)" \
    || died 'trust --yes' "$granted"
  echo "trust: OK — fenced project consented ($digest)"
}

# ── skills mode (config "-"): the CLI has no MCP support by design (pi) —
# conformance means skills render where the CLI actually loads them.
if [[ "$config_rel" == "-" ]]; then
  mkdir -p "$proj/.agentstack/skills/conformance-skill"
  printf -- '---\nname: conformance-skill\ndescription: conformance probe\n---\nprobe body\n' \
    > "$proj/.agentstack/skills/conformance-skill/SKILL.md"
  cat > "$proj/.agentstack/agentstack.toml" <<TOML
version = 1

[skills.conformance-skill]
path = "./skills/conformance-skill"

[toolsets.default]
skills = ["conformance-skill"]

[targets]
default = ["$adapter"]
TOML
  # An inline skill is loadable content, so trust refuses to pin a surface
  # that isn't itself pinned: lock first, then consent to the locked bytes.
  lock_out="$(cd "$proj" && as lock --write 2>&1)" || died 'lock --write' "$lock_out"
  trust_fenced_project
  (cd "$proj" && as use default --scope global --write)
  skill_md="$home/.pi/agent/skills/conformance-skill/SKILL.md"
  if [[ "$adapter" != "pi" ]]; then
    echo "FAIL: skills mode only knows pi's layout; got adapter '$adapter'"
    exit 1
  fi
  if ! grep -q "probe body" "$skill_md"; then
    echo "FAIL: rendered skill not readable at $skill_md"
    exit 1
  fi
  echo "structural: OK — skill renders (and reads back) under ~/.pi/agent/skills"
  if command -v "$cli_bin" >/dev/null; then
    if env "${xdg_unset[@]}" HOME="$home" "$cli_bin" --version >/dev/null 2>&1; then
      echo "live: OK — '$cli_bin --version' runs against the fenced HOME"
    else
      echo "live: SKIPPED — '$cli_bin --version' exited nonzero (recorded, not fatal: pi exposes no config introspection)"
    fi
  else
    echo "live: SKIPPED — $cli_bin not on PATH"
  fi
  echo "Done."
  exit 0
fi

# Minimal secret-free manifest: one stdio + one http server, plus the
# slash-named startup-validation probe, this adapter only.
#
# `[delivery] render_locally` is load-bearing, and it belongs here rather than
# inside the heredoc (which is unquoted, so backticks in it would run as
# command substitutions). Adapter-rot detection lives in the RENDERED lane:
# the alarm is "the file we write no longer parses in the real CLI", and that
# needs a file. Delivery routes MCP servers to the live lane by default — zero
# files, nothing on disk — so without this override `apply --write` correctly
# refuses with "nothing was delivered" and the smoke has nothing to hand the
# CLI. This is the manifest form of `agentstack x delivery render-locally
# --write`, whose stated reasons include, verbatim, compatibility testing
# against a CLI's own behaviour. Drop it and this alarm stops testing anything.
cat > "$proj/.agentstack/agentstack.toml" <<TOML
version = 1

[servers.conformance_probe]
type = "stdio"
command = "echo"
args = ["conformance"]

[servers.conformance_http]
type = "http"
url = "https://example.com/mcp"

[servers."slash/probe"]
type = "stdio"
command = "echo"
args = ["slash"]

[delivery]
render_locally = true

[targets]
default = ["$adapter"]
TOML

# --scope global is load-bearing: the checks below read the fenced $HOME and
# the live probe runs the CLI from outside $proj, so project-scope artifacts
# (the repo-manifest default since the context-derived scope landed) would be
# invisible to both.
trust_fenced_project

# The output is captured because the name-validation assertions below read it
# — but it is never captured silently: a refusal is printed, then the harness
# fails loudly, instead of `set -e` swallowing the capture whole.
apply_out="$(cd "$proj" && as apply --scope global --write 2>&1)" \
  || died 'apply --scope global --write' "$apply_out"
printf '%s\n' "$apply_out"

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

# Structural: the file must parse in its native format. A python3 too old for
# tomllib (<3.11) degrades LOUDLY to skip-parse — a parse error still fails.
case "$config" in
  *.toml)
    if python3 -c 'import tomllib' 2>/dev/null; then
      python3 -c 'import sys, tomllib; tomllib.load(open(sys.argv[1], "rb"))' "$config"
    else
      echo "structural parse: SKIPPED — python3 lacks tomllib (<3.11); content checks still run"
    fi
    ;;
  *)
    python3 -m json.tool "$config" >/dev/null
    ;;
esac
echo "structural: OK — $config_rel parses and contains the probe server"

# Startup-validation probe: a CLI that rejects the name must get a config
# WITHOUT it plus a spoken skip; every other CLI must receive it verbatim.
case "$adapter" in
  codex)
    if grep -q 'slash/probe' "$config"; then
      echo "FAIL: codex config contains 'slash/probe' — Codex rejects that name at every startup"
      exit 1
    fi
    if ! grep -q "skipping 'slash/probe'" <<<"$apply_out" \
       || ! grep -qi "rejects this server name" <<<"$apply_out"; then
      echo "FAIL: the skip must be spoken, not silent. apply output:"
      printf '%s\n' "$apply_out"
      exit 1
    fi
    echo "name-validation: OK — slash/probe skipped for codex, loudly"
    ;;
  *)
    if ! grep -q 'slash/probe' "$config"; then
      echo "FAIL: $adapter accepts slash names but the render dropped 'slash/probe'"
      exit 1
    fi
    echo "name-validation: OK — slash/probe rendered for $adapter"
    ;;
esac

# Live: ask the real CLI to read its own config. Strongest signal, but some
# CLIs refuse to run unauthenticated — degrade to structural-only, loudly.
if ! command -v "$cli_bin" >/dev/null; then
  echo "live: SKIPPED — $cli_bin not on PATH"
  echo "Done."
  exit 0
fi
# Per-CLI probe: most CLIs expose `mcp list` as a subcommand; Copilot CLI
# (>= 1.0.x) runs slash-style commands through -i instead.
probe() {
  case "$adapter" in
    copilot-cli) env "${xdg_unset[@]}" HOME="$home" "$cli_bin" -i "mcp list" 2>&1 ;;
    *) env "${xdg_unset[@]}" HOME="$home" "$cli_bin" mcp list 2>&1 ;;
  esac
}
if out="$(probe)"; then
  if grep -q conformance_probe <<<"$out"; then
    echo "live: OK — '$cli_bin mcp list' sees the probe server"
  elif [[ "$(classify_cli_failure "$out")" == skip ]]; then
    # Some CLIs (Copilot) print their auth gate and still exit 0.
    echo "live: SKIPPED — '$cli_bin' hit an auth/onboarding gate (exit 0). Output:"
    head -20 <<<"$out"
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
    head -20 <<<"$out"
  else
    echo "FAIL: $cli_bin exited nonzero and the output matches no known auth gate:"
    head -20 <<<"$out"
    exit 1
  fi
fi
echo "Done."
