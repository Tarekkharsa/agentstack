<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Share one setup with your team

For a team that wants every member's agent CLIs configured the same way.
Prerequisite: a working [manifest](../concepts.md) in `.agentstack/` (run
`agentstack init`, then `agentstack apply --write`, once).

You hand a setup over by **committing it**: the repo itself carries the
manifest and lock, and each clone reviews before anything activates.

```bash
# You, once: commit the manifest and its lockfile
git add .agentstack/          # manifest + agentstack.lock
git commit -m "Add agentstack setup"
git push

# Each teammate, after cloning:
agentstack trust .             # review what the repo declares, then approve it
agentstack secret set GH_PAT   # their own value (keychain by default)
agentstack apply --write       # render the shared setup into the CLIs they have
agentstack doctor              # confirm the result; every warning names its fix
```

**`trust .` comes first, and it is not optional.** A clone starts untrusted, and
an untrusted project refuses to deliver: `apply --write` writes no server
definitions and compiles no house rules, and `use --write` materializes no skill
files. Each refusal is loud, exits nonzero, and names `agentstack trust .` —
run it before `apply`, not after, or the teammate's first command fails for a
reason the sequence never mentioned. The lockfile is already committed and
already pinned, so there is nothing to re-lock first; see
[trust a cloned repo](trust-a-repo.md) for the full boundary.

**Changing the setup later re-opens it for everyone.** Editing the manifest or
re-running `agentstack lock --write` moves the consent surface, so your own
project drifts too — re-approve with `agentstack trust .` before you `apply`.
Once you push, each teammate's `git pull` drifts their grant the same way, and
their next `apply --write` or `use --write` refuses until they review what
moved. The review is a diff of what changed since they last said yes, not a
re-read of the whole surface, so batch a run of edits into one commit rather
than trickling them out.

You commit **intent**, not credentials. The [manifest](../concepts.md) is the
reviewed source of truth and the [lockfile](../concepts.md) pins exact
versions and digests, so everyone resolves the same bytes. Secrets appear in
the manifest only as `${REF}` placeholders — each teammate stores their own
value locally with `agentstack secret set`.

In v0.18.0 and later, `agentstack x up` collapses the teammate's four commands
into one, and `agentstack x share <name>` / `agentstack x receive <path>` move a
setup as a signed `.astack` bundle when there is no shared repo to commit to —
staged inert and carded first. See
[newer than the stable release](../start.md#newer-than-the-stable-release).

**Never committed:** secret values (they live per-machine in the OS keychain or
a gitignored `.env`) and the rendered native artifacts — `.mcp.json`,
`.claude/skills/`, the compiled `CLAUDE.md` — which sit behind a managed
`.gitignore` block whenever they are written — see
[delivery routing](../concepts.md#delivery-modes) for which capabilities are
written at all and which are served live.

**Optional provenance.** A maintainer can `agentstack x sign` the lockfile — it
writes a detached ed25519 signature and prints a public key to publish.
Teammates run `agentstack x verify --pubkey <key>` to confirm the lockfile is the
one the maintainer signed before they rely on it.

**Limits.** agentstack shares configuration, not trust in referenced code.
`verify` proves who signed the lockfile, not that a server it names is safe to
run. [Consent does not travel](trust-a-repo.md): each teammate runs
`agentstack trust .` themselves, in every delivery mode, and so does each of
their machines and each fresh checkout — the grant is keyed to a path on one
machine, never to the repo. A CI runner is no different; see
[use it in CI](ci.md) for the headless form.

- [Concepts](../concepts.md) — manifest, lockfile, secrets, delivery modes
- [Reference: `export` / `import`](../reference.md#export--import) — move a whole setup between machines
- [Reference: syncing across machines (`lib sync`)](../reference.md#syncing-across-machines-lib-sync)
