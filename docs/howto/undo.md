<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Undo anything

For anyone who wants to reverse something agentstack did. Prerequisite: none —
these work in any project agentstack has touched.

```bash
agentstack restore                 # list every undoable recorded write
agentstack restore --last --write  # undo the most recent write
agentstack restore a1b2 --write    # undo one write by its id prefix
agentstack restore claude-code     # fallback: restore one adapter's config from its backup
```

`restore` is the single undo verb for **writes**. Every write agentstack makes —
servers, settings, hooks, instructions, even the owned-server manifest refresh —
is recorded before it lands, and `restore` reverts one; `restore <adapter>` is a
fallback that restores one adapter's config from its single-slot backup.
Reverted files simply show up as pending again — the dashboard's Activity tab
lists the same recorded writes, each with the `restore` to roll it back (see
[see what your agents did](see-what-happened.md)). Five other actions are undone
by their own verb, because they are not file writes:

| To undo… | Run | What it reverts |
| --- | --- | --- |
| a recorded write (`apply` / `use` / `session` / settings / hooks / instructions) | `agentstack restore <id> --write` | puts the changed native config back and marks it pending |
| a [gateway](../concepts.md) registration | `agentstack gateway disconnect <cli> --write` | removes the gateway entry from that CLI's global config (`--all` for every CLI) |
| the destructive-command [guard](../concepts.md) | `agentstack guard uninstall` | removes every hook it installed and sets `[guard] enabled = false` |
| [trust](../concepts.md) for a repo | `agentstack trust --revoke` | withdraws consent — the repo goes inert again |
| an active [session](../concepts.md) | `agentstack session end` | reverts this directory's ephemeral profile (`--all` for every session) |
| a server or skill in the manifest | `agentstack remove <name> --write` | drops it from the manifest and the lockfile |

**Limits.** `restore` reverts agentstack's own recorded config writes, not side
effects a tool already had — a file a server deleted is not brought back.
Nothing here permanently deletes your data; each verb reverses one agentstack
change. One edge case: replacing an already-managed skill with the same name is
not snapshotted byte-exact, so its restore is not promised exact.

- [Concepts](../concepts.md) — trust, gateway, guard, session, drift
- [Reference: one undo verb (`restore`)](../reference.md#one-undo-verb-restore)
- [Reference: drift — adopt or apply?](../reference.md#drift-adopt-or-apply)
