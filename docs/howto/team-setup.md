<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Share one setup with your team

For a team that wants every member's agent CLIs configured the same way.
Prerequisite: a working [manifest](../concepts.md) in `.agentstack/` (run
`agentstack init`, then `agentstack apply --write`, once).

Two ways to hand a setup over: **commit it** (this page — the repo itself
carries the manifest and lock, and each clone reviews before anything
activates), or **bundle it** — `agentstack share <name>` produces a signed
`.astack` bundle (signing is part of sharing, not a flag) that a teammate
reviews with `agentstack receive <path>`: staged inert and carded first, and a
publisher they recognize shortens the card, never skips it. Use the bundle
when there is no shared repo to commit to.

```bash
# You, once: commit the manifest and its lockfile
git add .agentstack/          # manifest + agentstack.lock
git commit -m "Add agentstack setup"
git push

# Each teammate, after cloning:
agentstack up                  # one command: detects their CLIs, renders the shared setup, names what's left
agentstack secret set GH_PAT   # their own value (keychain by default) — `up` says if one is needed
```

(`up` is v0.18.0 and later; on older releases the same journey is
`agentstack apply --write` then `agentstack doctor`.)

You commit **intent**, not credentials. The [manifest](../concepts.md) is the
reviewed source of truth and the [lockfile](../concepts.md) pins exact
versions and digests, so everyone resolves the same bytes. Secrets appear in
the manifest only as `${REF}` placeholders — each teammate stores their own
value locally with `agentstack secret set`, and `up` renders the shared
config into whatever CLIs they have installed, confirms the result, and
names the fix for anything missing (`apply --write` and `doctor` remain the
step-by-step equivalents).

**Never committed:** secret values (they live per-machine in the OS keychain or
a gitignored `.env`) and the rendered native artifacts — `.mcp.json`,
`.claude/skills/`, the compiled `CLAUDE.md` — which sit behind a managed
`.gitignore` block in the default [static delivery mode](../concepts.md).

**Optional provenance.** A maintainer can `agentstack sign` the lockfile — it
writes a detached ed25519 signature and prints a public key to publish.
Teammates run `agentstack verify --pubkey <key>` to confirm the lockfile is the
one the maintainer signed before they rely on it.

**Limits.** agentstack shares configuration, not trust in referenced code.
`verify` proves who signed the lockfile, not that a server it names is safe to
run. In the [zero-files mode](trust-a-repo.md) each teammate still runs
`agentstack trust .` themselves — consent is per person, per machine.

- [Concepts](../concepts.md) — manifest, lockfile, secrets, delivery modes
- [Reference: `export` / `import`](../reference.md#export--import) — move a whole setup between machines
- [Reference: syncing across machines (`lib sync`)](../reference.md#syncing-across-machines-lib-sync)
