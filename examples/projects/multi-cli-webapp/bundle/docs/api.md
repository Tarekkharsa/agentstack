# API

Two endpoints, both JSON.

- `GET /products` → `{ "data": [ { id, name, cents } ] }`
- `GET /products/:id` → `{ "data": { id, name, cents } }`, or an error envelope.

Errors are always `{ "error": { "code": string, "message": string } }`. The
codes in use are `bad_request` (400) and `not_found` (404).
