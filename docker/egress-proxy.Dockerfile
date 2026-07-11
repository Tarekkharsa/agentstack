# The AgentStack egress-proxy sidecar image — the no-direct-route lockdown's
# only bridge between a sandboxed run's internal network and the outside.
#
# Build from the REPO ROOT (the workspace is the build context):
#   docker build -f docker/egress-proxy.Dockerfile -t agentstack/egress-proxy:dev .
#
# Multi-stage: the workspace compiles in a rust:alpine builder (musl, so the
# binary is static against alpine's libc) and only the one binary ships in the
# runtime layer. First build compiles the dependency tree (~minutes); Docker
# caches the layers after that.

FROM rust:alpine AS build
RUN apk add --no-cache musl-dev
WORKDIR /src
COPY . .
RUN cargo build --release -p agentstack-egress --bin agentstack-egress-proxy

FROM alpine:3
COPY --from=build /src/target/release/agentstack-egress-proxy /usr/local/bin/agentstack-egress-proxy
# The proxy needs no privileges: unprivileged ports, no filesystem writes.
USER 65534:65534
ENTRYPOINT ["/usr/local/bin/agentstack-egress-proxy"]
