<img alt="agentstack" src="docs/logo.svg" width="380">

> **One agent setup. Every coding CLI.**
> Today you configure the same MCP server once per tool, in a different format
> each time, with your tokens sitting in plain JSON.
> With AgentStack the whole setup — config, skills, servers, instructions —
> lives in one repo you own, any machine gets it with one command, and every CLI
> speaks it: Claude Code, Codex, Cursor, Gemini CLI, OpenCode and
> [eight more](https://tarekkharsa.github.io/agentstack/adapters.html) — served
> live where the tool speaks MCP, written as native files where it does not.

[![CI](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/ci.yml?branch=main&style=flat&label=CI)](https://github.com/Tarekkharsa/agentstack/actions/workflows/ci.yml) [![Conformance](https://img.shields.io/github/actions/workflow/status/Tarekkharsa/agentstack/conformance.yml?style=flat&label=conformance)](https://github.com/Tarekkharsa/agentstack/actions/workflows/conformance.yml) [![Release](https://img.shields.io/github/v/release/Tarekkharsa/agentstack?style=flat&label=release)](https://github.com/Tarekkharsa/agentstack/releases) [![License](https://img.shields.io/badge/license-MIT_OR_Apache--2.0-blue?style=flat)](https://github.com/Tarekkharsa/agentstack/blob/main/LICENSE-MIT)

## Try it in 60 seconds

```sh
curl -fsSL https://raw.githubusercontent.com/Tarekkharsa/agentstack/main/install.sh | sh
export PATH="$HOME/.local/bin:$PATH"          # only if the installer printed "Add to PATH"
agentstack init                               # finds what your CLIs already have, writes it into .agentstack/
agentstack more gateway connect --all --write # connect your CLIs once, so they receive things live
agentstack status                             # is it ready — and if not, the one thing that fixes it
# then restart your coding CLI — it reads its config at startup:
agentstack more why <server>                  # which of your CLIs is being served this, and from where
```

The installer verifies the download against the checksums published with the
release. `more` is the extended toolbox; every `more` command also runs at its
bare name. Step 3 printing `already connected` is success — the interactive
wizard may have done it for you.

![Two CLIs with different half-setups: agentstack imports both into one setup file, connects them so both are served the servers live while the project stays clean, passes doctor with 0 errors, writes each native format on request, and restores the machine byte-for-byte](docs/demos/first-value.svg)

## Your second machine

Publish your setup into a library repo you own, commit it, and the next machine
is one command: `agentstack up --library <git-url> --write`. Secret values and
the trust review stay per machine, by design.
→ [Set up another machine](https://tarekkharsa.github.io/agentstack/start.html#6-set-up-another-machine)

## Where to go next

- **[Get started](https://tarekkharsa.github.io/agentstack/start.html)** — the guided walkthrough, with real output at every step
- **[Documentation](https://tarekkharsa.github.io/agentstack/docs.html)** — concepts, how-tos, migration recipes, troubleshooting
- **[All commands](https://tarekkharsa.github.io/agentstack/reference.html)** — the complete inventory; `agentstack more` lists the toolbox in your terminal
- **[Releases](https://github.com/Tarekkharsa/agentstack/releases)** · [CHANGELOG](CHANGELOG.md) — what shipped, and when
- **[Contributing](CONTRIBUTING.md)** — the fast loop, every CI gate, and the security invariants
- **Support and conduct** — [SUPPORT.md](SUPPORT.md) · [CODE_OF_CONDUCT.md](CODE_OF_CONDUCT.md) · [GOVERNANCE.md](GOVERNANCE.md). Report vulnerabilities privately through [SECURITY.md](SECURITY.md), never in a public issue.

**Supported platforms: macOS and Linux.** A Windows binary is published but is
not exercised by CI — treat it as untested rather than supported.

MIT OR Apache-2.0.
