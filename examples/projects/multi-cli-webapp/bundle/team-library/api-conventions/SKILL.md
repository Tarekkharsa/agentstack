---
name: api-conventions
description: House API conventions for the storefront — REST resource shapes, the shared error envelope, and pagination rules.
---

# API conventions

STOREFRONT-SKILL-CONV-Q3: these are the conventions every storefront endpoint follows.

## Resources

- Success responses wrap the payload: `{ "data": <resource-or-list> }`.
- A resource is `{ id, name, cents }`. Prices are integer cents, never floats.

## Errors

- Always the shared envelope: `{ "error": { "code": string, "message": string } }`.
- Known codes: `bad_request` (400), `not_found` (404), `conflict` (409).

## Pagination

- List endpoints accept `?limit` (default 20, max 100) and `?cursor`.
- Return the next cursor as `{ "data": [...], "next": string | null }`.
