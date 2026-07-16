# Storefront house rules

STOREFRONT-HOUSE-RULE-A7: validate every request body with zod at the boundary,
and return the shared `{ error: { code, message } }` envelope on failure.

- Conventional Commits for every message (`feat:`, `fix:`, `chore:`).
- Run `npm run format` before you commit.
- Never invent a new error shape — reuse the envelope in `src/routes.ts`.
