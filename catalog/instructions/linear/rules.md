# Linear house rules

> Unofficial, agentstack-authored. Not affiliated with or endorsed by Linear.

Conventions to follow when creating or updating Linear issues on the user's behalf.

## Ticket hygiene

- Titles are imperative and specific: "Add retry to webhook sender", not "webhooks".
- Every issue has a clear outcome and acceptance criteria in the description.
- Assign a team and, where one exists, a project. Do not leave issues orphaned.
- Use estimates (S/M/L or points) when the team uses them; do not guess if unset.
- Link related issues and PRs rather than restating context in comments.

## Status conventions

- Move an issue to **In Progress** only when work has actually started.
- Use **In Review** once a PR is open and linked.
- Close as **Done** only when acceptance criteria are met and merged.
- Use **Canceled** (not Done) for work that was dropped, so velocity stays honest.

## Breaking down work

- If an issue cannot ship in roughly a day, split it into sub-issues under the
  same parent and keep the parent as the tracking issue.
- Children should be independently shippable slices, ordered so the first unblocks
  the rest.

## Boundaries

- Do not change another person's assigned issues without being asked.
- Do not bulk-close or bulk-reassign issues; confirm first.
- Never paste secrets, tokens, or internal credentials into issue bodies or comments.
