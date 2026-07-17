# locked-run — the Protected host tier, end-to-end

`agentstack run <harness> --locked` promotes a host run to the Protected tier:
it refuses to launch unless the project is explicitly trusted, every pinned
byte in the integrity surface still matches (manifest, lock, and D3-pinned
local executables), and the declared capabilities fit under the machine
policy ceiling. When every gate passes it freezes an `AuthorityGrant`, seals
its bridge projection into a machine-authenticated run-grant artifact, and
launches the harness with a launch-scoped MCP config pointing at
`agentstack mcp --grant <artifact>` — a bridge that consumes the frozen
surface **verbatim** and never re-derives authority from disk.

This is pre-launch gating plus a frozen capability surface — not kernel
isolation. `--lockdown` is the kernel fence.

## What `assert.sh` proves

1. **`--plan`** prints the fully-assembled plan and mutates nothing — no run
   evidence, no harness launch.
2. **A clean locked run** freezes the grant (digest printed), hands the sealed
   artifact to the bridge, spawns the harness **at the project root**, records
   `grant_frozen` → `completed` in the run's `events.jsonl`, and leaves no
   bridge-config residue in the repo afterwards.
3. **The frozen bridge refuses the mutating control plane**: under
   `mcp --grant`, `agentstack_session_start` (which resolves secrets into
   native configs), `agentstack_lease_open`, and `agentstack_add_server` are
   refused fail-closed for the run's duration; read-only discovery still
   answers.
4. **Tampering fails machine authentication**: one flipped byte in the sealed
   artifact and the bridge refuses loudly, proxying nothing.
5. **Drift re-gates** (rule 4): a post-lock manifest edit makes consent stale
   and the run refuses before launch.
6. **D3 pins are load-bearing**: a one-byte edit to a pinned server executable
   (`opsbox.sh`) refuses the run; `lock` + `trust` readmit it — a consent
   re-gate, not a lockout.
7. **`--profile` is a fence**: under `--locked --profile ci` the frozen grant
   names only the fenced subset (`opsbox`, never `scratchpad`), and the
   artifact carries `${REF}`-only definitions — no argv, no secret values.

## Run it

```bash
cargo build --release            # or AGENTSTACK_BIN=/path/to/agentstack
bash examples/projects/locked-run/assert.sh
```

Isolated temp `HOME` + `AGENTSTACK_HOME`, a stub `claude` harness on `PATH` —
nothing touches your real config, no network, no Docker.

## Historical note

Writing this example exposed a real gap: in the preferred `.agentstack/`
layout, D3 executable pins were derived against the manifest dir instead of
the project root, so local server executables were silently never pinned and
step 6 launched instead of refusing. The fix normalizes to the project root
inside `derive_executable_pins` — the one function every pin producer and
verifier funnels through — with a unit witness beside it; step 6 is the
end-to-end witness.
