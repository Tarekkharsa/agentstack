# Governance

AgentStack is currently a solo-maintained, pre-1.0 project.

## Roles and decisions

- **Maintainer:** Tarek Kharsa owns releases, repository administration,
  roadmap ordering, and final merge decisions.
- **Contributors:** anyone may report problems or propose a focused change under
  [`CONTRIBUTING.md`](CONTRIBUTING.md). A contribution does not imply ongoing
  support or commit access.
- Product work follows [`STRATEGY.md`](STRATEGY.md) and the ordered
  [`TODO.md`](TODO.md). Design documents explain contracts; they do not create a
  second roadmap.

Decisions prefer observable user outcomes, compatibility, reversibility, and
preservation of the security invariants. The maintainer records shipped changes
in [`CHANGELOG.md`](CHANGELOG.md) and material enforcement limits in
[`docs/ENFORCEMENT.md`](docs/ENFORCEMENT.md).

## Security-boundary review

Changes to trust granting, policy intersection, digest computation, secret
resolution, authority construction, or upstream dispatch require focused
witnesses and line-by-line review. While there is one maintainer, the second
review may be an independently prompted reviewer, but the release notes must
say what was reviewed and by whom; automation is not represented as community
oversight.

## Commit access and succession

Commit access is granted only after repeated reviewed contributions and an
explicit agreement to uphold the security and release contracts. A new
maintainer must first demonstrate familiarity with `trust`, `policy`, release
provenance, and recovery procedures.

There is no hidden successor today. If the maintainer becomes unavailable, no
one should publish a release or claim continuity without control of the
repository and release credentials plus a fresh review of the publishing path.
The safe fallback is the last attested release and readable generated native
configuration; users can leave through `agentstack uninstall --write`. A future
co-maintainer must update this file with named release and security-review
ownership before continuity is claimed.

## Funding and conflicts

The project has no funding or sponsorship program today. If funding begins,
material sponsors and conflicts that could affect roadmap or review decisions
will be disclosed here.
