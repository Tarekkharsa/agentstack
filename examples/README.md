# examples — what each directory shows

Every entry is self-contained. The ones marked **runnable** carry a
`run-demo.sh` that runs in an isolated temp `HOME`, asserts each claim, and
exits nonzero on any mismatch — so they double as the witness that the docs
stay accurate. The rendered catalogue is
[the demos page](https://tarekkharsa.github.io/agentstack/examples.html).

| Directory | What it shows |
| --- | --- |
| [`first-value-demo/`](first-value-demo/) | **Runnable.** Import two CLIs' native configs into one manifest, serve them live, render on request, restore byte-for-byte. |
| [`everyday-loop-demo/`](everyday-loop-demo/) | **Runnable.** The everyday loop — `yes`, `undo`, `share`, `receive`, `up` — across two isolated machines. Needs v0.18.0 or newer. |
| [`one-manifest-demo/`](one-manifest-demo/) | **Runnable.** One secret-free manifest rendered into each CLI's own native format by `apply`. |
| [`malicious-repo-demo/`](malicious-repo-demo/) | **Runnable.** A cloned repo's MCP server stays inert until you trust it, and machine policy denies what the repo allows. |
| [`guard-demo/`](guard-demo/) | **Runnable.** The cooperative pre-tool-use guard refusing destructive commands, proved by grepping the audit log. |
| [`mcp-profile-lease/`](mcp-profile-lease/) | **Runnable.** The zero-file lease against one real `agentstack mcp` process: open, load a skill, freeze the observed set, close. |
| [`projects/`](projects/) | Seven realistic example projects, each with a README and an `assert.sh` that proves its use case. |
| [`sandbox/`](sandbox/) | A simulated machine for the scripted demos (`demo-firstrun.sh`, `demo-central-library.sh`, `demo-lockdown.sh`), under an isolated `HOME`. |
| [`policies/`](policies/) | Four ready-to-use machine-policy presets — the standing firewall a project can narrow but never loosen. |
| [`workflow-acceptance/`](workflow-acceptance/) | The governed workflow engine's day-one map → reduce pipeline, packaged as a bundle. |
| [`workflow-scale/`](workflow-scale/) | The drive loop's fan-out bench: a latency-controlled mock harness with width and concurrency knobs. |
