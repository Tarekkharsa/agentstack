<!-- INTERNAL SOURCE: this file is the build input for its page on
     https://tarekkharsa.github.io/agentstack/ — readers go to the site.
     Edit here, then run: python3 tools/make-docs-pages.py -->

# Trust a cloned repo

For anyone who clones repos that ship their own agent capabilities and wants
them inert until reviewed. Prerequisite: the CLIs you use, installed on this
machine.

```bash
# Once per machine: register the agentstack gateway in your CLIs
agentstack gateway connect --all --write

# Clone a repo and enter it
git clone <some-repo> && cd <some-repo>

# The repo is inert — an agent here sees control-plane tools only,
# nothing spawned, nothing contacted, no secrets resolved
agentstack trust .          # review what it declares, then pin its digest

agentstack trust --list     # every trusted project + whether it still matches
agentstack trust --revoke   # withdraw trust
```

`gateway connect --all --write` registers agentstack's gateway once in each
CLI's global MCP ([Model Context Protocol](../concepts.md)) config. After that,
every repo you open serves its own MCP servers with no files copied in — but a
repo you just cloned is **inert**: none of its servers run or are contacted, and
no secrets resolve, until you run `agentstack trust .`. Trust shows exactly what
the manifest runs and contacts, then pins the [consent digest](../concepts.md)
of the [manifest](../concepts.md), its local overlay, and the
[lockfile](../concepts.md). Any edit — a `git pull`, an `agentstack lock` —
drops the repo back to inert until you trust it again. To vet one server or
skill in depth first — its provenance, effective policy, and context cost — run
`agentstack explain <name>`; see [see what your agents did](see-what-happened.md).

**What trust covers, and what it doesn't.** Trust pins those three files and
gates whether the declared servers may run; it does **not** vouch for the code
those servers point at. A server that runs a local script authorizes *that
command*, not later edits to it — so review referenced scripts as part of
`trust .`, the way you'd read a `.envrc` before `direnv allow`. The full boundary
is the enforcement matrix's
[What "trusted" does and does not mean](../ENFORCEMENT.md#what-trusted-does-and-does-not-mean).

**Limits.** Trust is consent to a set of bytes, not a sandbox. It gates
activation; it does not confine a server once it runs. For runtime confinement,
see [lock down a run](lock-down-a-run.md).

- [Concepts](../concepts.md) — trust, gateway, consent digest, drift
- [Reference: the zero-files gateway](../reference.md#the-zero-files-gateway---auto-project--trust)
- [Enforcement: what "trusted" means](../ENFORCEMENT.md#what-trusted-does-and-does-not-mean)
