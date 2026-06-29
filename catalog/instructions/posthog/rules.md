# PostHog house rules

> Unofficial, agentstack-authored. Not affiliated with or endorsed by PostHog.

Conventions to follow when querying analytics, building insights, or managing
experiments and feature flags in PostHog on the user's behalf.

## Query hygiene

- Confirm event and property names against the project before building a query;
  do not assume an event like `signup` or `purchase` exists.
- State the date range, population, and active filters alongside every number so
  results are reproducible.
- Prefer unique-user math over raw event counts unless raw volume is the point.
- Exclude internal and bot traffic where the project supports it.

## Insights and dashboards

- Give insights clear, self-explanatory names that state the metric and scope.
- Save reusable questions as named insights on a dashboard rather than re-running
  one-off ad hoc queries.
- Pick the insight type that matches the question: trends for counts over time,
  funnels for drop-off, retention for repeat behavior.

## Experiments and feature flags

- Each experiment has one falsifiable hypothesis and one primary metric.
- Respect the planned sample size and runtime; do not peek-and-stop or call a
  winner early.
- Roll flags out gradually and watch guardrail metrics at each step.

## Boundaries

- Do not present a query result as fact without stating its window and filters.
- Do not change another person's insights, dashboards, experiments, or flags
  without being asked.
- Do not bulk-delete or bulk-edit insights, dashboards, cohorts, or flags;
  confirm first.
- Never flip a feature flag to 100% of users, or kill/launch an experiment,
  without an explicit go-ahead.
- Never paste secrets, personal API keys, or internal credentials into insight
  descriptions, dashboards, or annotations.
