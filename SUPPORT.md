# Support

AgentStack is maintained on a best-effort basis. There is no paid support, SLA,
or guaranteed response time.

## Where to go

- **Bug or compatibility regression:** open a GitHub issue with the AgentStack
  version, OS, affected coding CLI/version, reproduction steps, and redacted
  `agentstack doctor --json` output.
- **Usage question:** open a GitHub issue and label the question clearly. Check
  the [troubleshooting guide](https://tarekkharsa.github.io/agentstack/troubleshooting.html)
  and [FAQ](https://tarekkharsa.github.io/agentstack/faq.html) first.
- **Security vulnerability:** do **not** open an issue. Follow
  [`SECURITY.md`](SECURITY.md); acknowledgment is targeted within seven days.
- **Conduct concern:** use the private route in
  [`CODE_OF_CONDUCT.md`](CODE_OF_CONDUCT.md).

Never attach secret values, `.env` contents, credentials, raw agent transcripts,
or unredacted private paths. Machine-readable diagnostics are local until you
choose to share them.

## Support window

Only the latest published release receives fixes. macOS and Linux are the
supported release platforms. A Windows archive is published for evaluation but
is not a supported platform until CI exercises the suite there. Adapter fidelity
varies by vendor and is stated in the current
[adapter support matrix](https://tarekkharsa.github.io/agentstack/adapters.html).

Pre-1.0 releases may change machine contracts when a correctness or security
boundary requires it. User-facing removals and migrations are recorded in
[`CHANGELOG.md`](CHANGELOG.md); stable JSON features are negotiated by feature
name rather than guessed from a version string.
