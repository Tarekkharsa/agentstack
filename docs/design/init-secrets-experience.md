# Secrets at init: choose a store, or none at all

> **Status:** draft for maintainer review (S0)<br/>
> **Date:** 2026-07-17<br/>
> **Origin:** maintainer ruling — secrets management must be right before the
> first public release. Users should choose how their secrets are handled
> during init, and no choice may be required: someone trying the project
> needs zero secret setup; someone adopting it graduates later.<br/>
> **Queue position:** release-gating for public distribution (whenever the
> deferred Phase 0B lane resumes), but S1 is bugfix-grade and in-cut-sized.
> Companion doc: [`init-access-control.md`](init-access-control.md) — same
> init flow, different review concerns, deliberately separate.

## 0. Motivation

Two facts make this design smaller than it first looks:

1. **The store is already cross-platform.** The "keychain" is the `keyring`
   crate — macOS Keychain, Windows Credential Manager, Linux Secret Service
   (`crates/cli/src/secret/keychain.rs`). Windows and desktop-Linux users
   have `agentstack secret set` today. The genuine platform gap is
   **headless Linux** (servers, containers, CI — no DBus secret-service),
   which is precisely where env vars and varlock are the right answer.
2. **"Optional" is already mechanically true.** Resolution is an ordered
   chain, first hit wins: process env → varlock (auto-detected when
   `.env.schema` + the binary exist) → OS keychain → plain project `.env`
   (`crates/cli/src/secret/mod.rs`). Exporting an env var or dropping a
   `.env` file works with zero setup, and rule 5 makes an unresolved
   `${REF}` fail closed *at use time* — trying the project never requires a
   secrets decision.

What is missing is the experience — and two shipped edges that contradict
the "never required" principle:

- **Interactive init hard-fails on an unreachable keychain.** Lifted
  plaintext tokens are stored with `keychain::set(...)?` *before* the
  manifest is written (`crates/cli/src/commands/init.rs:439-443`). On
  headless Linux, init with secrets in the imported CLI configs aborts
  outright unless the user already knows about `--no-keychain`. Fail-closed,
  but a wall in the first sixty seconds — for exactly the users the chain
  was built to serve.
- **Dashboard init silently drops values.** `let _ = keychain::set(...)`
  (`init.rs:58`): the manifest gets `${REF}`s whose values may have landed
  nowhere, with no report. Not a leak — `apply` fails closed on the
  unresolved ref — but a trap that reads as breakage.

## 1. What already exists (build on it, don't duplicate it)

- **The chain and its contract** (`secret/mod.rs`): ordered links behind one
  `Resolver` trait, per-run lookup caching, `Failed` distinguished from
  `Missing`, policy `Denied` terminal. New behavior slots in as links or
  flow changes — the contract does not move.
- **Per-ref source attribution exists.** `SecretSources::source_of`
  (`secret/mod.rs:159`) reports which layer a ref resolves from; `secret
  list`, `explain`, and the dashboard already consume it. The graduation
  surfaces below are new *wording* on this existing machinery.
- **Init's lifting moment is well-built.** Interactive init already shows
  each plaintext token found in live CLI configs, its origin, and the
  commit-safe story (`init.rs:381-406`), and `--dry-run` previews without
  storing. The prompt this design adds attaches to that existing moment.
- **`--no-keychain` exists** (`init.rs:450`) — the escape hatch is shipped;
  it is just undiscoverable and its follow-up advice (`agentstack secret
  set`) leads back to the same unreachable keychain on headless machines.
- **Fail-closed is the law everywhere already** (rule 5): unresolved →
  blocked write/run, never a placeholder in live config. Nothing in this
  design touches that.

## 2. Non-goals

- **No new secret backend.** No `keyring` major bump, no kernel-keyutils
  store, no home-grown encrypted file. The headless-Linux answer is the
  chain's existing links (env, varlock, `.env`), documented honestly.
  A native headless store is deferred until real demand (rule 6: propose,
  don't add).
- **Agentstack never writes varlock's files.** Varlock is detect-and-
  resolve; its setup is its own CLI's job. The offer is a pointer, not a
  wrapper. Write paths remain exactly two: keychain and project `.env`.
- **No secrets prompt when there is nothing to store.** A project with no
  lifted tokens initializes with zero secrets interaction, always.
- **Rule 5 is untouched.** Manifests hold `${REF}`s only; no mode, flag, or
  fallback ever serializes a value into a manifest or rendered config.

## 3. The init experience

The prompt appears **only when lifted secrets exist**, at the existing
lifting moment, after the per-token origin listing:

```
Where should these 3 values be stored?

  1. OS credential store (Keychain / Credential Manager / Secret Service)  [default]
  2. Project .env file — plaintext on disk, kept out of git
  3. Nowhere — keep the ${REF}s; I'll provide values via env, varlock,
     or `agentstack secret set` later
```

- **Option 1** is the default and preselected when the store probes as
  reachable (one cheap read at prompt time). When the probe fails, the
  prompt says so and the default moves to 3 — the flow *informs and
  continues* instead of aborting. This replaces the hard `?` at
  `init.rs:441` (edge 1).
- **Option 2** writes `NAME=value` lines into the project `.env` (append,
  never clobber existing keys) and verifies the file is gitignored —
  adding it to the managed gitignore block if not. The label always carries
  "plaintext on disk"; honesty is in the option text, not a later lecture.
- **Option 3** prints the fail-closed consequence plainly: *"these refs
  won't resolve until you provide values; apply/run will block on them by
  name."* This is the existing `--no-keychain` path made discoverable, with
  advice that works on every platform (env and varlock listed before
  `secret set`).
- **When varlock is detected**, one line above the prompt notes refs will
  resolve through it first, and option 3's text leads with it.
- **Flags win over prompts:** `--no-keychain` keeps its meaning (= option
  3, no prompt); a new `--secrets keychain|dotenv|none` selects
  non-interactively.
- **Non-interactive paths (dashboard init) never prompt:** default to the
  keychain, and on any store failure **report the unstored refs by name**
  in the summary instead of dropping silently (edge 2). The dashboard can
  offer the same three choices in its own UI later; the CLI contract is
  only "never silent."
- `--dry-run` previews the chosen destination ("would store 3 values in
  the OS credential store") and writes nothing, as today.

## 4. The machine-level preference

Re-asking at every project init is friction; the machine manifest gains an
optional preference consumed by init and `secret set`:

```toml
[secrets]
default_store = "keychain"   # or "dotenv" | "none" — absent = prompt
```

- Set by the first interactive choice ("remember this? [y/N]"), editable by
  hand like everything else in the personal layer.
- It is a *default*, not policy: `--secrets` overrides per invocation, and
  it never affects resolution order — the chain stays env → varlock →
  keychain → `.env` regardless.

## 5. Graduation: from trying to managing

The path from "values in a plaintext `.env`" to "managed store" should be
one visible nudge and one command — never a gate:

- **`secret list` / doctor labeling.** `source_of` already knows a ref
  resolves from `.env`; `secret list` marks those rows `plaintext (.env)`
  and doctor gains one **informational** finding: *"N secret(s) resolve
  from plaintext `.env` — `agentstack secret lift` moves them to the OS
  credential store."* Informational, not warning: `.env` is a legitimate
  choice the user made at init, and headless machines may keep it forever.
- **`agentstack secret lift`** — the graduation verb: for each ref the
  manifest uses that currently resolves from `.env`, store the value in the
  keychain, then remove the line from `.env` (prompted, `--keep` to copy
  without removing). The manifest doesn't change — it already holds
  `${REF}`s — so this is a value move, not a migration.
- **Docs page: "Where do secrets live?"** — one table (env / varlock /
  keychain / `.env`), what each is for (CI / teams-and-vaults / daily
  desktop / trying-it-out + headless), and the headless-Linux paragraph
  stating plainly that Secret Service needs a desktop bus and what to use
  instead.

## 6. Honest posture (labels, not promises)

- Cross-platform naming: user-facing copy says **"OS credential store"**
  with the three concrete names in parentheses — "keychain" alone reads as
  macOS-only, which undersold the shipped truth.
- The `.env` option is always labeled plaintext-on-disk at the moment of
  choice. No wording implies agentstack encrypts it.
- **Coherence with the guard** (companion doc): the default deny list
  blocks the *harness* from reading `.env` through governed hooks, while
  *agentstack* resolves `${REF}`s from it at render/call time. That is the
  model working as intended — agents receive secrets only through governed
  injection and never read the file — and the docs page in §5 says so in
  one sentence, because users who notice the two features will otherwise
  assume they conflict.
- Storing to the keychain does not make a secret unreachable by other
  processes running as the same user; no copy claims OS-level isolation
  beyond what the credential store provides.

## 7. Staged implementation

- **S0 — approve this design.** Settle: the three-option prompt and its
  default logic, `--secrets` flag shape, `[secrets] default_store`,
  `secret lift`, and the open questions.
- **S1 — fix the two edges (bugfix-grade, in-cut-sized).** Unreachable
  keychain stops aborting interactive init (probe → inform → default to
  refs-only); dashboard init reports unstored refs by name.
  *Witnesses:* init with lifted secrets and a failing store completes,
  writes a manifest whose refs are honestly reported as unstored, and
  stores nothing in plaintext without consent; the dashboard summary names
  every unstored ref.
- **S2 — the choice.** The three-option prompt at the lifting moment,
  `--secrets`, `.env` write path with managed-gitignore verification,
  `[secrets] default_store` + remember-prompt.
  *Witnesses:* option 2 never clobbers an existing `.env` key and refuses
  to write an un-gitignorable `.env` (no `.gitignore`, not a git repo →
  proceed with an explicit warning acknowledgment); no path stores values
  in two places.
- **S3 — graduation surfaces.** `secret list` plaintext labels, the doctor
  informational finding, `secret lift` (+ `--keep`), the "Where do secrets
  live?" docs page.
  *Witness:* `lift` moves a value keychain-ward, removes exactly the lifted
  lines, and a subsequent `apply` resolves every ref identically.

## 8. Open questions for S0

1. **Prompt inventory at init-time.** Should the prompt also appear when
   the manifest references `${REF}`s that resolve *nowhere* (not just when
   lifting plaintext)? (Recommend: no prompt — print the existing
   unresolved-refs summary with the same three-way advice; prompting on
   every unresolved ref punishes the try-first flow.)
2. **`default_store = "dotenv"` scope.** A machine-level default of
   "plaintext per-project file" is a footgun candidate. Keep it as a legal
   value (headless machines may genuinely want it), or restrict the
   remembered default to `keychain`/`none`? (Recommend: legal but never
   offered by the remember-prompt — reachable only by editing the file.)
3. **`secret lift` deletion semantics.** Removing lines from a user's
   `.env` edits a file agentstack doesn't own. Prompt per file, or is the
   command-level prompt plus `--keep` enough? (Recommend: command-level
   prompt showing the exact lines to be removed; `.env` may hold non-agent
   variables and they must survive untouched.)
4. **Headless-Linux detection.** Should doctor detect "no Secret Service
   bus" and proactively surface the env/varlock/`.env` guidance, or is the
   docs page enough? (Recommend: yes in doctor — it is one probe, and it
   converts the platform gap from a surprise into a labeled state.)
