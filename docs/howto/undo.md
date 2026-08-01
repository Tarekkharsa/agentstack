<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Undo anything

For anyone who wants to reverse something agentstack did. Prerequisite: none —
these work in any project agentstack has touched.

```bash
agentstack undo                    # your recent changes, newest first — pick one, revert to it
agentstack restore                 # the same record as a list of ids (script-friendly)
agentstack restore --last --write  # undo the most recent write
agentstack restore a1b2 --write    # undo one write by its id prefix
agentstack restore claude-code     # fallback: restore one adapter's config from its backup
```

Undo has two faces over one record. Every write agentstack makes — servers,
settings, hooks, instructions, even the owned-server manifest refresh — is
recorded before it lands. `agentstack undo` shows those recorded changes as a
timeline, newest first: pick a point and it reverts to there, and the revert
is itself recorded, so going one step too far is recoverable. `restore` works
the same record one write at a time — the script-friendly primitive underneath
(`restore <adapter>` is a fallback that restores one adapter's config from its
single-slot backup). Reverted files simply show up as pending again; `restore`
lists the recorded writes and the identifier needed to roll each one back (see
[see what your agents did](see-what-happened.md)). Five other actions are
undone by their own verb, because they are not file writes:

| To undo… | Run | What it reverts |
| --- | --- | --- |
| a recorded write (`apply` / `use` / `session` / settings / hooks / instructions) | `agentstack restore <id> --write` | puts the changed native config back and marks it pending |
| a [gateway](../concepts.md) registration | `agentstack gateway disconnect <cli> --write` | removes the gateway entry from that CLI's global config (`--all` for every CLI) |
| the destructive-command [guard](../concepts.md) | `agentstack guard uninstall` | removes every hook it installed and sets `[guard] enabled = false` |
| [trust](../concepts.md) for a repo | `agentstack trust --revoke` | withdraws consent — the repo goes inert again |
| an active [session](../concepts.md) | `agentstack session end` | reverts this directory's ephemeral toolset (`--all` for every session) |
| a server or skill in the manifest | `agentstack remove <name> --write` | drops it from the manifest and the lockfile |

**Limits.** `restore` reverts agentstack's own recorded config writes, not side
effects a tool already had — a file a server deleted is not brought back.
Nothing here permanently deletes your data; each verb reverses one agentstack
change. One edge case: replacing an already-managed skill with the same name is
not snapshotted byte-exact, so its restore is not promised exact.

## Undo all of it

The verbs above each reverse one thing. To reverse **everything** — every
managed region agentstack rendered into every CLI's config, plus its own state
directory — there is one command:

```bash
agentstack uninstall          # show what would be removed; changes nothing
agentstack uninstall --verbose  # ...with the full diff of each file
agentstack uninstall --write  # do it
```

Like every other write in agentstack, it previews first: the bare command lists
what it would take off and stops.

It removes what agentstack manages, not what you wrote. **Your
`agentstack.toml` stays exactly where it is**, so `agentstack apply --write`
brings the whole setup back whenever you want it. Entries you or another tool
added to those same config files by hand are left alone, and so is anything a
different project's manifest manages at global scope.

Because the removal runs through the same machinery as a normal write, every
file it touches is recorded first — so **the uninstall is itself undoable**:

```bash
agentstack restore --last --write   # put it all back
```

That only holds while the ledger exists, and the ledger lives in
`~/.agentstack`, which `uninstall` removes last. Keep it with:

```bash
agentstack uninstall --write --keep-home
```

Two things it does not do: remove the `agentstack` binary (take that off the way
you installed it), and touch a capability's own installed files outside the
regions agentstack renders.

- [Concepts](../concepts.md) — trust, gateway, guard, session, drift
- [Reference: undo — `undo` and `restore`](../reference.md#undo-undo-and-restore)
- [Reference: drift — adopt or apply?](../reference.md#drift-adopt-or-apply)
