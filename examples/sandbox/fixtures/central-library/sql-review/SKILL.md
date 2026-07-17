---
name: sql-review
description: Review SQL for correctness, performance, and safety before it ships.
---

# SQL Review

When reviewing SQL:

1. **Correctness** — joins, NULL handling, GROUP BY completeness.
2. **Performance** — indexable predicates, avoid `SELECT *`, watch N+1.
3. **Safety** — parameterized queries only; never interpolate user input.
