# Architecture

The storefront is a single Fastify service.

- `src/server.ts` boots the HTTP server and reads `PORT`.
- `src/routes.ts` owns the request surface and validates input with zod.
- `src/db.ts` is the data layer (an in-memory stub today, Postgres later).

There is no build step in the sample — `npm run dev` runs it under `tsx`.
