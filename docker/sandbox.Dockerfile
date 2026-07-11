# Example sandbox runner image for `agentstack run --sandbox`.
#
# `run --sandbox` launches your agent HARNESS (Claude Code, Codex, …) inside a
# container whose egress the AgentStack proxy enforces. AgentStack orchestrates
# from the host; it does NOT ship a universal runner image, because every
# harness differs. Build an image that carries the harness you run and point
# AgentStack at it:
#
#   docker build -f docker/sandbox.Dockerfile -t agentstack-sandbox .
#   AGENTSTACK_SANDBOX_IMAGE=agentstack-sandbox \
#     agentstack run claude-code --sandbox --lockdown -- <args>
#
# Without AGENTSTACK_SANDBOX_IMAGE, `run --sandbox` looks for
# `agentstack/sandbox:latest` and fails if you haven't built/tagged one.
#
# This example carries Claude Code on a slim Node base. Swap the install line
# for the harness you use (e.g. `@openai/codex`).
FROM node:22-slim

# curl covers HTTPS egress through the proxy; git and ca-certificates are what
# most agents expect. Keep the image minimal — it is untrusted-code's cage.
RUN apt-get update \
    && apt-get install -y --no-install-recommends curl git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# The harness CLI. Swap this line for whichever harness you launch.
RUN npm install -g @anthropic-ai/claude-code

# AgentStack mounts the project here (read-only unless [policy.filesystem] write
# covers it) and sets this as the working directory.
WORKDIR /workspace
