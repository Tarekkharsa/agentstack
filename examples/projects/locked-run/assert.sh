#!/usr/bin/env bash
# ─────────────────────────────────────────────────────────────────────────────
# AgentStack — `run <harness> --locked`: the Protected host tier, proven
# END-TO-END with a stub harness.
#
#   A locked run refuses to launch unless the project is trusted, every pinned
#   byte still matches, and the declared capabilities fit under the machine
#   ceiling — then freezes an AuthorityGrant and hands the bridge a sealed
#   run-grant artifact it consumes VERBATIM (no disk re-derivation). This
#   script proves:
#
#     1. `--plan`      → prints the full plan, mutates nothing, records nothing.
#     2. Clean run     → grant frozen, harness launched AT THE PROJECT ROOT,
#                        outcome recorded, no bridge-config residue in the repo.
#     3. Frozen bridge → under `mcp --grant`, mutating / secret-resolving
#                        control-plane tools (session_start, lease_open,
#                        add_server) are refused fail-closed; read-only
#                        discovery still answers.
#     4. Tampering     → one flipped byte in the sealed artifact fails machine
#                        authentication; the bridge serves NOTHING.
#     5. Drift         → a post-lock manifest edit re-gates trust and refuses
#                        the run before launch (rule 4).
#     6. D3 pins       → a one-byte edit to a pinned server executable refuses
#                        the run; re-lock + re-trust readmits it.
#     7. Profile fence → `--locked --profile ci` freezes ONLY the fenced
#                        subset into the grant the bridge serves.
#
# Exits nonzero and prints FAIL on any mismatch; safe to run unattended. Runs
# entirely inside an isolated sandbox — nothing touches your real config.
#
# Requires: `agentstack` on PATH (or AGENTSTACK_BIN=..., or a built
# target/release/agentstack in this repo) and python3.
# ─────────────────────────────────────────────────────────────────────────────
set -euo pipefail

HERE="$(cd "$(dirname "$0")" && pwd -P)"

# ── binary resolution: AGENTSTACK_BIN, else PATH, else this repo's release build
AS="${AGENTSTACK_BIN:-}"
if [[ -z "$AS" ]]; then
  if command -v agentstack >/dev/null 2>&1; then
    AS="$(command -v agentstack)"
  else
    d="$HERE"
    while [[ "$d" != "/" ]]; do
      if [[ -x "$d/target/release/agentstack" ]]; then AS="$d/target/release/agentstack"; break; fi
      d="$(dirname "$d")"
    done
  fi
fi
if [[ -z "$AS" ]]; then
  echo "could not find agentstack: set AGENTSTACK_BIN, add it to PATH, or run 'cargo build --release'" >&2
  exit 2
fi
if [[ "$AS" == */* ]]; then
  AS="$(cd "$(dirname "$AS")" && pwd -P)/$(basename "$AS")"
else
  AS="$(command -v "$AS")"
fi

PASS=0
FAIL=0
ok()   { printf '  \033[32mPASS\033[0m %s\n' "$*"; PASS=$((PASS + 1)); }
bad()  { printf '  \033[31mFAIL\033[0m %s\n' "$*"; FAIL=$((FAIL + 1)); }

PAUSE="${DEMO_PAUSE:-0}"
say()  { printf '\n\033[1;35m▎ %s\033[0m\n' "$*"; [ "$PAUSE" = "0" ] || sleep "$PAUSE"; }

# ── isolated sandbox: AGENTSTACK_HOME redirects the ENTIRE machine state tree ─
SBX="$(mktemp -d)"
export AGENTSTACK_HOME="$SBX/home"
export HOME="$SBX/fakehome"
mkdir -p "$AGENTSTACK_HOME" "$HOME"
trap 'rm -rf "$SBX"' EXIT

# ── stub harness: a fake `claude` on PATH that records where it was spawned ──
BIN="$SBX/bin"
mkdir -p "$BIN"
cat > "$BIN/claude" <<EOF
#!/bin/sh
pwd -P > "$SBX/harness-cwd"
exit 0
EOF
chmod 755 "$BIN/claude"
export PATH="$BIN:$PATH"

# clone the committed bundle into the sandbox and work there
PROJECT="$SBX/project"
mkdir -p "$PROJECT"
cp -R "$HERE/bundle/." "$PROJECT/"
cd "$PROJECT"

RUNS="$AGENTSTACK_HOME/runs"
# newest run dir under the isolated home
newest_run() { ls -t "$RUNS" 2>/dev/null | head -1; }

INIT='{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}'

call() { printf '{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"%s","arguments":%s}}' "$1" "${2:-\{\}}"; }

printf '\033[1;36m  agentstack — run --locked: the Protected host tier\033[0m\n'

# ── pin + trust: the locked tier's entry requirements ────────────────────────
say "Pin the surface (lock) and grant consent (trust) — locked runs demand both"
"$AS" lock --manifest-dir "$PROJECT" >/dev/null
"$AS" trust . --yes >/dev/null 2>&1

# ── 1) --plan: the auditable description, zero mutation ──────────────────────
say "1) run claude-code --locked --plan — describe everything, run nothing"
PLAN="$("$AS" run claude-code --locked --plan 2>&1)" || { bad "--plan exited nonzero: $PLAN"; }
if grep -qi 'trust' <<< "$PLAN" && [ ! -e "$SBX/harness-cwd" ]; then
  ok "--plan prints the assembled plan and never launches the harness"
else
  bad "--plan should print the plan without launching; got: $PLAN"
fi
if [ ! -d "$RUNS" ] || [ -z "$(ls "$RUNS" 2>/dev/null)" ]; then
  ok "--plan records no run evidence (mutates nothing)"
else
  bad "--plan left run evidence behind: $(ls "$RUNS")"
fi

# ── 2) clean locked run ──────────────────────────────────────────────────────
say "2) run claude-code --locked — every gate passes, the grant freezes, the harness runs"
OUT="$("$AS" run claude-code --locked 2>&1)" || { bad "locked run failed: $OUT"; }
if grep -q 'authority grant frozen' <<< "$OUT"; then
  ok "run: the AuthorityGrant froze (digest printed) before launch"
else
  bad "run: expected 'authority grant frozen'; got: $OUT"
fi
if grep -q 'run grant handed to the gateway' <<< "$OUT"; then
  ok "run: the sealed run-grant artifact was handed to the gateway"
else
  bad "run: expected the grant handoff line; got: $OUT"
fi
if [ "$(cat "$SBX/harness-cwd" 2>/dev/null)" = "$(cd "$PROJECT" && pwd -P)" ]; then
  ok "run: the harness was spawned AT THE PROJECT ROOT"
else
  bad "run: harness cwd was '$(cat "$SBX/harness-cwd" 2>/dev/null)'"
fi
RUN1="$(newest_run)"
EVENTS="$RUNS/$RUN1/events.jsonl"
if grep -q '"event":"grant_frozen"' "$EVENTS" && grep -q '"outcome":"completed"' "$EVENTS"; then
  ok "evidence: events.jsonl records grant_frozen and the completed outcome"
else
  bad "evidence: expected grant_frozen + completed outcome in $EVENTS"
fi
if [ ! -e "$PROJECT/.mcp.json" ] && ! ls "$PROJECT"/*.agentstack-locked.lock >/dev/null 2>&1; then
  ok "hygiene: the launch-scoped bridge config and sentinel are gone after the run"
else
  bad "hygiene: locked-run residue left in the project"
fi

GRANT="$RUNS/$RUN1/grant.json"
if [ ! -f "$GRANT" ]; then
  # fall back: find the artifact wherever the run dir put it
  GRANT="$(find "$RUNS/$RUN1" -name 'grant.json' | head -1)"
fi

# ── 3) the frozen bridge refuses the mutating control plane ──────────────────
say "3) mcp --grant — mutating/secret-resolving control-plane tools are refused fail-closed"
for tool in agentstack_session_start agentstack_lease_open agentstack_add_server; do
  RESP="$(printf '%s\n%s\n' "$INIT" "$(call "$tool" '{"profile":"ci","scope":"project"}')" \
    | "$AS" mcp --grant "$GRANT" 2>/dev/null | tail -1)"
  if grep -q 'unavailable under a frozen run grant' <<< "$RESP"; then
    ok "frozen bridge: $tool is refused for the run's duration"
  else
    bad "frozen bridge: $tool should be refused; got: $RESP"
  fi
done
RESP="$(printf '%s\n%s\n' "$INIT" "$(call agentstack_lease_status)" \
  | "$AS" mcp --grant "$GRANT" 2>/dev/null | tail -1)"
if grep -q 'unavailable under a frozen run grant' <<< "$RESP"; then
  bad "frozen bridge: read-only lease_status should still answer; got refusal"
else
  ok "frozen bridge: read-only discovery (lease_status) still answers"
fi

# ── 4) one flipped byte in the artifact → machine authentication fails ───────
say "4) tamper with the sealed artifact — the MAC refuses, nothing is served"
TAMPERED="$SBX/tampered-grant.json"
python3 - "$GRANT" "$TAMPERED" <<'EOF'
import json, sys
signed = json.load(open(sys.argv[1]))
mac = signed["mac"]
signed["mac"] = ("0" if mac[0] != "0" else "1") + mac[1:]
json.dump(signed, open(sys.argv[2], "w"))
EOF
ERR="$(printf '%s\n' "$INIT" | "$AS" mcp --grant "$TAMPERED" 2>&1 >/dev/null || true)"
if grep -q 'REFUSING the frozen run grant' <<< "$ERR" && grep -q 'machine authentication' <<< "$ERR"; then
  ok "tamper: the bridge refuses loudly — failed machine authentication"
else
  bad "tamper: expected a machine-authentication refusal; got: $ERR"
fi
TOOLS="$(printf '%s\n%s\n' "$INIT" \
  '{"jsonrpc":"2.0","id":9,"method":"tools/list"}' \
  | "$AS" mcp --grant "$TAMPERED" 2>/dev/null)"
if ! grep -q 'opsbox__' <<< "$TOOLS" && ! grep -q 'scratchpad__' <<< "$TOOLS"; then
  ok "tamper: the refused bridge proxies NOTHING (no upstream tools advertised)"
else
  bad "tamper: upstream tools leaked through a refused grant"
fi

# ── 5) drift: a post-lock manifest edit refuses before launch (rule 4) ───────
say "5) edit the manifest after lock — the locked run refuses before launch"
cp "$PROJECT/.agentstack/agentstack.toml" "$SBX/manifest.orig"
printf '\n# drifted after lock\n' >> "$PROJECT/.agentstack/agentstack.toml"
rm -f "$SBX/harness-cwd"
if OUT="$("$AS" run claude-code --locked 2>&1)"; then
  bad "drift: the locked run should have refused; it ran: $OUT"
else
  ok "drift: the locked run refused (exit nonzero) after the manifest edit"
fi
if [ ! -e "$SBX/harness-cwd" ]; then
  ok "drift: the harness never launched"
else
  bad "drift: the harness launched despite drift"
fi
cp "$SBX/manifest.orig" "$PROJECT/.agentstack/agentstack.toml"

# ── 6) D3: a one-byte edit to a pinned executable refuses; re-gate readmits ──
say "6) flip one byte in a pinned server executable — refused until re-lock + re-trust"
printf '# one more byte\n' >> "$PROJECT/opsbox.sh"
if OUT="$("$AS" run claude-code --locked 2>&1)"; then
  bad "D3: the run should have refused after the executable edit; it ran"
else
  if grep -qi 'opsbox' <<< "$OUT"; then
    ok "D3: refused before launch, naming the drifted executable surface"
  else
    ok "D3: refused before launch after the executable edit"
  fi
fi
"$AS" lock --manifest-dir "$PROJECT" >/dev/null
"$AS" trust . --yes >/dev/null 2>&1
if "$AS" run claude-code --locked >/dev/null 2>&1; then
  ok "D3: re-lock + re-trust readmits the run (consent re-gate, not a lockout)"
else
  bad "D3: expected the re-gated project to run again"
fi

# ── 7) --profile is a FENCE: the grant freezes only the subset ────────────────
say "7) run claude-code --locked --profile ci — the grant carries ONLY the fenced subset"
OUT="$("$AS" run claude-code --locked --profile ci 2>&1)" || bad "fenced run failed: $OUT"
if grep -q "profile fence: 'ci'" <<< "$OUT"; then
  ok "fence: the run states the profile fence up front"
else
  bad "fence: expected the profile-fence line; got: $OUT"
fi
RUN_FENCED="$(newest_run)"
GRANT_FENCED="$(find "$RUNS/$RUN_FENCED" -name 'grant.json' | head -1)"
SERVERS="$(python3 -c "
import json,sys
h = json.load(open(sys.argv[1]))['handoff']
print(','.join(sorted(s['name'] for s in h['servers'])))
" "$GRANT_FENCED")"
if [ "$SERVERS" = "opsbox" ]; then
  ok "fence: the frozen grant names ONLY opsbox — scratchpad is outside the fence"
else
  bad "fence: expected servers=opsbox in the grant; got: $SERVERS"
fi
SECRETLESS="$(grep -c 'sk-\|password\|BEGIN ' "$GRANT_FENCED" || true)"
if [ "$SECRETLESS" = "0" ] && python3 -c "
import json,sys
h = json.load(open(sys.argv[1]))['handoff']
cmds = [s['server'].get('command','') for s in h['servers']]
sys.exit(0 if all(c.startswith('./') for c in cmds) else 1)
" "$GRANT_FENCED"; then
  ok "fence: the artifact carries \${REF}-only definitions — no argv, no secret values"
else
  bad "fence: unexpected content in the sealed artifact"
fi

say "Nothing runs until it's trusted; nothing trusted runs unobserved."
printf '\n\033[1mSummary:\033[0m %d passed, %d failed\n' "$PASS" "$FAIL"
[ "$FAIL" -eq 0 ] || exit 1
