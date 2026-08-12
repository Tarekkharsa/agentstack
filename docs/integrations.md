<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Integrations

AgentStack is standalone. Its manifest, lock, trust, policy, dynamic delivery,
and machine bootstrap work without a supervisor or custom UI.

An integration may launch a supported CLI, but it does not become a second
configuration system.

## T3 Code

Stock T3 Code can launch and supervise Claude Code, Codex, Cursor, OpenCode,
and other providers. Those processes read the same global gateway and managed
provider configuration AgentStack already owns.

The practical setup is therefore only:

```bash
agentstack init --connect
agentstack status     # inspect this worktree
```

When `doctor` detects T3 Code it checks provider guard coverage and reports
provider-instance home overrides that could point a session away from the
global configuration.

### Several projects and machines

T3 Code Connect can control work on remote machines. AgentStack handles the
configuration on each machine independently:

```text
T3 Code UI
  ├── machine A → project A → local AgentStack gateway
  └── machine B → project B → local AgentStack gateway
                          ↑
                same central library Git repo
```

Bootstrap each machine with the same library:

```bash
agentstack up --library https://github.com/you/ai-setup.git
agentstack up --library https://github.com/you/ai-setup.git --write
```

Project manifests and locks travel with their repositories. Library content
travels through its Git remote. Secrets, trust, policy, and audit evidence stay
on the machine where the agent runs.

No T3 Code fork or AgentStack panel is required. The earlier experimental
custom panel is not the supported setup path; its lasting lesson is that a UI
should present AgentStack's JSON status and preview/consent contracts while the
standalone CLI remains the authority. AgentStack can expose those stable JSON
read and preview/apply contracts without moving authority out of the CLI.

For integrations, `agentstack init --plan` advertises the `init-plan` contract:
it emits JSON with the detected setup and a `plan_digest` without changing the
machine. There is no separate `--json` flag; `--plan` is already the JSON read.
Pass that digest to the approved write step so a UI cannot apply a different
plan from the one the human reviewed.

### JSON contract discovery

Read `features` in every JSON envelope before using an integration contract.
This release advertises: `init-plan`, `apply-setup`, `init-tool-managed-v1`,
`trust-preview`, `trust-consent`, `status-v1`, `profiles-v1`, `diff-v1`,
`restore-last`, `sessions-v1`, `profiles-edit-v1`, `diff-ownership-v1`,
`toolset-create-v2`, `profiles-edit-batch-v1`, `toolset-rename-v1`,
`toolset-delete-v1`, `library-remove-v1`, `manifest-remove-v1`,
`trust-server-blockers-v1`, `trust-review-card-v1`, `trust-card-diff-v1`,
`trust-card-groups-v1`, `activity-skill-load-v1`, `workflow-observe-v1`,
`workflow-serial-roles-v1`, `doctor-advisories-v1`, `doctor-mode-v1`,
`doctor-liveness-v1`, `doctor-probe-v1`, `diff-existence-v1`, `json-reads-v1`,
`gitignore-opt-out-v1`, `doctor-cli-coverage-v1`, `status-honesty-v1`,
`needs-your-yes-v1`, `update-offer-v1`, `package-members-v1`,
`lease-status-v1`, `delivery-routing-v1`, `library-sources-v1`,
`trust-gate-reading-v1`, `instruction-channels-v1`, `image-plan-v1`,
`workflow-role-selection-v1`, `trust-content-drift-v1`, and
`abandoned-render-v1`.

### Fresh worktrees

A fresh worktree has the committed project manifest and lock. With zero-files
delivery it does not need generated MCP configs or copied skill directories.
It still needs local trust because consent is tied to the checkout path.

Run in the worktree:

```bash
agentstack status
```

Then review the checkout:

```bash
agentstack trust .
```

The next agent connection receives the default toolset automatically. A file-
only provider may additionally need the rendered command printed by status.

### Per-run evidence

T3 Code sessions normally attribute to the machine's global audit. For a
separate run identity, create a transparent AgentStack shim and configure that
provider instance to launch it:

```bash
agentstack x shim make claude
```

This is optional and does not change the provider's capabilities.

## Other supervisors

The same rule applies to any launcher: if it starts a supported CLI with its
normal home and working directory, AgentStack works unchanged. If the launcher
overrides either, run `agentstack doctor` on that machine and follow the
reported adapter-specific fix.

Next: [Another machine](start.md#6-set-up-another-machine) ·
[Team setup](howto/team-setup.md) · [Adapter support](adapters.md)
