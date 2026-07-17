# Device access control as a first-run experience

> **Status:** draft for maintainer review (A0)<br/>
> **Date:** 2026-07-17<br/>
> **Origin:** every agent CLI on a device should have its file access
> governed; the maintainer wants that to be part of the `init` experience —
> a shipped default, editable at machine scope and narrowable per project —
> instead of a hand-written config only its author knows exists.<br/>
> **Queue position:** post-keystone, in-cut-adjacent. This is UX over
> already-shipped enforcement — no new policy semantics, no new enforcement
> code paths. It does not displace the remaining Phase 0A keystone review;
> it is sized to follow it.

## 0. Motivation

The enforcement machinery for device-wide file access control is shipped:
`[policy.filesystem] deny` with machine ∪ project union semantics (a repo can
only add denies, never subtract), the host guard wired into nine CLIs'
pre-tool-use hooks, `[guard] allow_roots`, denial auditing, and fail-closed
machine-policy loading. What is not shipped is any way a user would ever
*encounter* it:

- A fresh device has no machine manifest, so machine policy is absent —
  which is **open** by design (`crates/cli/src/machine_policy.rs`: explicit
  absence is open; only corruption fails closed). Nothing suggests creating
  one.
- `agentstack init` (project) seeds `policy: Default::default()` and never
  mentions the dimension exists (`crates/cli/src/commands/init.rs`).
- `agentstack guard install` exists and works, but nothing in the setup flow
  offers it. The guard protecting this maintainer's machine exists because
  its author wrote the deny list by hand.

The result is the worst kind of gap for a security tool: the protection is
real, tested, and silently absent on every machine but one. The fix is an
onboarding surface, not new machinery.

## 1. What already exists (build on it, don't duplicate it)

- **`agentstack init --global` is the seam.** It already seeds
  `~/.agentstack/agentstack.toml` from a template, refuses to overwrite
  without `--force`, and previews everything under `--dry-run` before any
  write (`init.rs::run_global`). Today the template carries only
  `[instructions]`; this design extends the template and the flow, not the
  command model.
- **The guard verbs exist.** `agentstack guard install / uninstall / status`
  (`crates/cli/src/commands/guard.rs`) — install detects the CLIs present
  and wires each one's own pre-tool-use hook; status reports per-CLI state.
- **The matcher is multi-spelling.** `deny_glob_check`
  (`crates/cli/src/guard.rs`) tests every deny glob against the absolute
  path, the workspace-relative path (across symlink-resolved spellings —
  #23), and the bare basename. So basename entries (`.env`), extension
  globs (`*.pem`), and workspace-relative prefixes (`vault/**`) all work
  today. Whether a glob itself may be home-anchored (`~/.aws/**`) is
  unverified — settled as open question 2 before any such entry enters the
  template.
- **Union semantics are already the law.** Machine deny ∪ project deny; the
  project layer can only narrow (rule 2). A per-project override experience
  needs zero new semantics — it is "add entries to the project manifest."
- **Surfacing exists.** `doctor` inspects machine policy
  (`machine_policy::inspect`), `explain` shows effective policy, denials are
  recorded to `~/.agentstack/audit/calls.jsonl`, and the enforcement matrix
  already words the guard honestly (cooperative ¶, catches accidents not
  malice, fails closed on unreadable config).

## 2. Non-goals

- **No positive grants.** "These agents may read only these folders" is
  Phase 1 Workspace Grants (`FsRules.read` is informational on the host
  path today, and the sandbox rounds partial write scopes down). This
  design ships a deny-list-plus-guard experience and must not let init copy
  imply scoped reads exist.
- **No kernel claims.** The guard is cooperative — the harness must honor
  its own hook protocol. Every sentence init prints is bound by the
  enforcement matrix's ¶ wording.
- **No semantics changes.** Absence of machine policy stays open;
  corruption stays fail-closed; the union stays a union. This design makes
  the default *present*, it does not make absence closed.
- **No silent installation.** The guard edits other CLIs' config files;
  that never happens without an explicit per-flow yes (existing `init`
  house style: preview before any write).
- **No new verb in v1.** The experience lives in `init --global` and
  project `init`. A memorable alias (`agentstack protect`) is open
  question 1, not a requirement.

## 3. The device-setup flow (`init --global`, extended)

The global template gains two blocks and the flow gains one offer:

```toml
[guard]
enabled = true
# Writes outside a project workspace are blocked by default. Add roots
# agents may legitimately write to:
# allow_roots = ["~/notes"]

[policy.filesystem]
# Files no agent on this machine may read or write, in any project.
# Matched against absolute, workspace-relative, and bare-filename
# spellings. Projects can ADD entries in their own manifest; nothing can
# subtract from this list.
deny = [
  ".env", ".env.local", ".env.*.local",
  ".env.production", ".env.development",
  "id_rsa", "id_ed25519", "id_ecdsa",
  "*.pem",
  ".netrc",
  "credentials.json",
]
```

After the manifest is written (same preview/confirm discipline as today),
the flow makes one offer:

```
Machine policy written. The host guard enforces it inside each CLI's own
hook system — it blocks accidental secret reads and destructive commands;
it is not a sandbox.

Detected CLIs: claude-code, codex, cursor
Install the guard into these 3 CLIs? [Y/n]
```

Yes runs the existing `guard install`; no prints the one-liner to do it
later. `--dry-run` previews the template including the policy blocks and
performs no guard changes. The dashboard's non-interactive init gets the
same template but never auto-installs the guard (it reports the pending
offer instead).

## 4. The default deny list is a reviewed security artifact

The template list above is the proposal; it is deliberately conservative,
because the failure mode of an over-broad default is not inconvenience —
it is the user uninstalling the guard. Selection rules:

| Entry | Why | False-positive risk |
|---|---|---|
| `.env` family | The canonical secret file; already proven on this machine | Low — agents rarely need to read it, and blocking is the point |
| `id_rsa`, `id_ed25519`, `id_ecdsa` | SSH private keys by basename, wherever they live | Negligible — `git` over SSH reads keys itself, not through a harness file tool |
| `*.pem` | Key material by extension | Low; a repo legitimately full of `.pem` fixtures adds nothing back (union) — it must fork the machine list, which is honest |
| `.netrc` | Plaintext credentials by convention | Negligible |
| `credentials.json` | The common cloud-SDK download name | Moderate — the one entry worth debating at A0 |

Explicitly **not** in v1 defaults: `*.key` (matches too many non-secret
files), browser profile directories (guard reads would break nothing, but
unverified glob anchoring — Q2), and `~/.aws/**` / `~/.ssh/**` style
home-anchored entries (same Q2; once verified, they are the strongest
candidates for v2 of the template, shipped as commented examples first).

Changing the list later is `agentstack init --global --force` (re-seed) or
editing the file — `doctor` validates it either way.

## 5. Project scope: encounter, narrow, verify

- **Project `init` emits a commented block** in the generated manifest, the
  same pattern as the commented server examples:

  ```toml
  # [policy.filesystem]
  # deny = ["fixtures/prod-dump/**"]   # adds to the machine list; cannot subtract
  ```

- **Project `init` prints protection status** as one line: guard installed
  and enabled (with the machine deny count), or `guard not installed — run:
  agentstack guard install`.
- **`doctor` gains two informational findings** (not errors — absence is a
  legal state): no machine manifest / no `[policy.filesystem]` at machine
  scope, and guard-not-installed while a policy exists that only the guard
  enforces on the host path. Wording points at `init --global` and `guard
  install` respectively.
- **`explain` already shows effective policy** — the docs for this feature
  route "what is actually denied right now, and which layer said so"
  through it rather than inventing a new surface.

## 6. Honest posture (labels, not promises)

- Init copy uses the matrix's own vocabulary: *"blocks accidental secret
  reads and destructive commands inside supported CLIs' hook systems; it is
  not a sandbox and does not constrain a harness that ignores its own
  hooks."* The words "protect", "control", and "enforce" never appear
  without that qualifier in first-run output.
- Claude Desktop and Junie have no hook surface; `guard status` already
  knows per-CLI state, and the install offer lists only CLIs it can
  actually wire — it never claims device-wide coverage beyond them.
- The default list stops *file-tool and shell-token access through governed
  hooks*. It does not stop an MCP server process from reading anything (that
  is the sandbox tier), and no init sentence may blur that line.

## 7. Staged implementation

Small stages; only A1 touches anything security-adjacent (the template
content and the guard-offer flow), and it changes no enforcement semantics.

- **A0 — approve this design.** Settle: the default deny list (§4, entry by
  entry), the guard-offer wording, no-new-verb, and the open questions.
- **A1 — device setup.** Extend the global template with `[guard]` +
  `[policy.filesystem]` defaults; add the post-write guard-install offer;
  dashboard parity (report, don't auto-install).
  *Witnesses:* `--dry-run` shows the policy blocks and writes nothing;
  an existing machine manifest is never overwritten without `--force`;
  declining the offer performs zero guard writes; with the seeded default,
  a `cat .env` through a hooked CLI is denied and audited.
- **A2 — project surfaces.** Commented `[policy.filesystem]` block in
  project init output; the protection-status line; the two doctor
  informational findings.
  *Witness:* a project-added deny glob is enforced in union with the
  machine list (extends the existing #11 guard test).
- **A3 — docs + demo.** A short "protect this device" docs page off the
  README ladder; optionally a fourth demo clip (init --global → guard
  offer → blocked `.env` read) in the established asciinema pipeline.

## 8. Open questions for A0

1. **Verb.** Keep everything under `init --global`, or add `agentstack
   protect` as a discoverable alias for template-seed + guard-install?
   (Recommend: no alias in v1; one less verb, and the README ladder can
   market the flow without a new command.)
2. **Home-anchored deny globs.** Does the ruleset glob matcher expand `~`
   inside *globs* (the path side expands it; the glob side is unverified)?
   Verify with a witness before any `~/.aws/**`-style entry ships, even
   commented. If unsupported, decide whether to add anchoring or keep the
   template basename-only.
3. **`credentials.json`.** In or out of the v1 default list? (Recommend:
   in, commented, so the user opts in by uncommenting — the one entry with
   real false-positive potential.)
4. **Doctor severity.** Should "policy exists but guard not installed"
   escalate from info to warning once the user has run `init --global`
   (they opted into the model, then lost enforcement)? (Recommend: yes —
   info before opt-in, warning after.)
