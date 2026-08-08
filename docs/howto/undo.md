<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Undo anything

For anyone who wants to reverse something agentstack did. Prerequisite: none —
these work in any project agentstack has touched.

```bash
agentstack x restore                 # everything undoable, newest first, as a list of ids
agentstack x restore --last --write  # undo the most recent write
agentstack x restore a1b2 --write    # undo one write by its id prefix
agentstack x restore claude-code     # fallback: restore one adapter's config from its backup
```

Every file write agentstack makes — servers, settings, hooks, instructions, even
the owned-server manifest refresh — is recorded before it lands, and `restore`
works that record one write at a time. Reverting is itself recorded, so going
one step too far is recoverable. (`restore <adapter>` is a fallback that
restores one adapter's config from its single-slot backup.) Reverted files
simply show up as pending again; `restore` lists the recorded writes and the
identifier needed to roll each one back (see
[see what your agents did](see-what-happened.md)). Five other actions are
undone by their own verb, because they are not file writes:

| To undo… | Run | What it reverts |
| --- | --- | --- |
| a recorded write (`apply` / `use` / `session` / settings / hooks / instructions) | `agentstack x restore <id> --write` | puts the changed native config back and marks it pending |
| a [gateway](../concepts.md) registration | `agentstack x gateway disconnect <cli> --write` | removes the gateway entry from that CLI's global config (`--all` for every CLI) |
| the destructive-command [guard](../concepts.md) | `agentstack x guard uninstall` | removes every hook it installed and sets `[guard] enabled = false` |
| [trust](../concepts.md) for a repo | `agentstack trust --revoke` | withdraws consent — the repo goes inert again: no servers written or served, no skills materialized, no house rules compiled, no hooks or extensions rendered |
| an active [session](../concepts.md) | `agentstack x session end` | puts every file back **and** takes off any skills the session materialized (`--all` for every session) |
| a server config the rendered lane left behind | `agentstack x unrender --write` | removes only server files AgentStack wrote for harnesses now served live; previews without `--write`, and is itself undoable |
| a server or skill in the manifest | `agentstack x remove <name> --write` | drops it from the manifest and the lockfile |
| a [skill](add-a-skill.md) materialized on disk | `agentstack use --write` on a toolset that omits it, or `agentstack x uninstall --write` | takes the delivered skill off — `restore` cannot, see below |

**Skills are outside the `restore` ledger.** A recorded change is a file and its
pre-write bytes, and a delivered skill is neither: it is a symlink to a
directory, or a copied tree. Nothing is captured for it, and every undo screen
says so in the same words — *materialized skills are not recorded here — this
ledger holds files, and a delivered skill is a linked directory.* So `restore`
brings the config edits back and leaves the skills in place. Re-materialize them
by activating a toolset that includes them (`agentstack use --write`); take them
off with `agentstack x uninstall --write`.

The ledger is narrow on purpose. `restore` acts on a path with no ownership
test, so a ledger that also carried skills would delete a skill directory you
put there by hand after delivery.

One consequence to know: a session that materialized **only** skills records no
ledger entry at all, so there is nothing for `restore` to roll back.

```console
$ agentstack x restore --last --write
error: nothing to undo — no recorded write that isn't already undone
```

`agentstack x session end` is the whole revert for a session — it puts the files
back and takes the skills off in one step.

In v0.18.0 and later, `agentstack undo` shows the same record as a timeline —
pick a point and it reverts to there. See
[newer than the stable release](../start.md#newer-than-the-stable-release).

**Undoing is never gated.** Delivering into a project needs
[a review first](trust-a-repo.md) — writing a server config, materializing a
skill, compiling house rules, rendering a hook or an extension all refuse until
you run `agentstack trust .`. Taking those bytes back off disk does not. Every
verb above, plus `x unrender` and `x uninstall`, works in a repo that is
untrusted or whose review has gone stale, because removal is the inert
direction. You never have to consent to something in order to get rid of it.

**Limits.** `restore` reverts agentstack's own recorded config writes, not side
effects a tool already had — a file a server deleted is not brought back.
Nothing here permanently deletes your data; each verb reverses one agentstack
change. Materialized skills are the one thing `restore` does not carry, for the
reason above; they come back with `agentstack use --write`.

## Undo all of it

The verbs above each reverse one thing. To reverse **everything** — every
managed region agentstack rendered into every CLI's config, plus its own state
directory — there is one command:

```bash
agentstack x uninstall          # show what would be removed; changes nothing
agentstack x uninstall --verbose  # ...with the full diff of each file
agentstack x uninstall --write  # do it
```

Like every other write in agentstack, it previews first: the bare command lists
what it would take off and stops.

It removes what agentstack manages, not what you wrote. **Your
`agentstack.toml` stays exactly where it is**, so `agentstack apply --write`
brings the whole setup back whenever you want it. Entries you or another tool
added to those same config files by hand are left alone, and so is anything a
different project's manifest manages at global scope.

Because the removal runs through the same machinery as a normal write, every
file it touches is recorded first — so **the config half of the uninstall is
itself undoable**:

```bash
agentstack x restore --last --write   # put the config edits back
agentstack use --write                # re-materialize the skills it pruned
```

Skills need the second command because they are outside the ledger, as above;
the CLI names both when the project has any. That only holds while the ledger
exists, and the ledger lives in
`~/.agentstack`, which `uninstall` removes last. Keep it with:

```bash
agentstack x uninstall --write --keep-home
```

Two things it does not do: remove the `agentstack` binary (take that off the way
you installed it), and touch a capability's own installed files outside the
regions agentstack renders.

- [Concepts](../concepts.md) — trust, gateway, guard, session, drift
- [Reference: undo — `undo` and `restore`](../reference.md#undo-undo-and-restore)
- [Reference: drift — adopt or apply?](../reference.md#drift-adopt-or-apply)
