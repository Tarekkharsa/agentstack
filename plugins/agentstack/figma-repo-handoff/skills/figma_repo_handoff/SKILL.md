---
description: Translate Figma context into repo-local implementation work without leaking credentials.
---

# Figma Repo Handoff

Use this skill when translating Figma context into repo-local implementation work.

Workflow:

1. Read the relevant repo files before deciding how to implement a Figma change.
2. Prefer existing components, tokens, layout utilities, and test patterns over new abstractions.
3. Keep generated assets and design-specific code scoped to the feature being implemented.
4. Verify the result with the project's normal formatter and focused tests.

Do not paste or persist access tokens. If a Figma or MCP credential is missing, ask the user to configure the native tool's credential flow instead of recording the secret in source.
