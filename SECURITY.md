# Security Policy

AgentStack is a security tool: it trust-gates, firewalls, and audits what AI
agent CLIs are allowed to do on a machine. Vulnerability reports are welcome
and taken seriously.

## Supported versions

Only the [latest release](https://github.com/Tarekkharsa/agentstack/releases)
receives security fixes. There are no maintained older branches.

## Reporting a vulnerability

Please report vulnerabilities privately — do not open a public issue.

- Preferred: [GitHub private vulnerability reporting](https://github.com/Tarekkharsa/agentstack/security/advisories/new)
  on `Tarekkharsa/agentstack`.
- Alternatively: email the maintainer at <tarekkh1997@gmail.com>.

Include what you can: affected version, reproduction steps, and impact
(e.g. which trust, policy, secret, or sandbox guarantee is bypassed).

You should get an acknowledgment within **7 days**. This is a single-maintainer
project, so fix timelines depend on severity and complexity; you'll be kept
informed of progress.

## Threat model

The current threat model and its known limits are documented, not implied:

- [Security review (2026-07-11)](docs/security-review-2026-07-11.html) — the
  most recent full review, including what was found and fixed.
- [Enforcement matrix](docs/ENFORCEMENT.md) — what each guarantee actually
  enforces per CLI and per mode, including where enforcement is advisory.

Reports that show a gap between what those documents claim and what the code
does are especially valuable.
