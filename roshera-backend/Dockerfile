# syntax=docker/dockerfile:1
#
# Roshera CAD — api-server production image.
#
# Two stages: a Rust release build stage, then a slim Debian runtime that
# carries only the single `api-server` binary plus its shared-library deps.
#
# The server binds 0.0.0.0:8081 (api-server/src/main.rs: `let addr =
# "0.0.0.0:8081"`), exposes an unauthenticated liveness probe at GET /health
# (auth_middleware::path_is_exempt allowlists "/" and "/health"), and requires
# a reachable PostgreSQL at boot (main.rs reads DATABASE_URL and calls
# PostgresDatabase::new(..).await? before it starts listening).

# ---------------------------------------------------------------------------
# Build stage
# ---------------------------------------------------------------------------
# Roshera's own crates are edition 2021, but api-server links the vendored
# goose (vendor/goose, rust-version = "1.94.1"), whose dependencies include
# edition-2024 crates (agent-client-protocol-derive): Cargo 1.84 refuses to
# parse them ("feature `edition2024` is required"). The builder therefore
# matches goose's declared minimum. Bookworm keeps the builder glibc aligned
# with the bookworm-slim runtime below.
FROM rust:1.94.1-bookworm AS builder

RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/roshera

# Copy the whole workspace. We deliberately do NOT hand-enumerate crate
# manifests for a dependency-cache layer: the workspace membership drifts
# (members are shared-types, geometry-engine, session-manager, timeline-engine,
# api-server, ai-integration, export-engine, ros-format, verdict-harness,
# assembly-engine — the previous Dockerfile still listed a non-existent
# rag-engine), and a stale manifest list silently breaks the cache layer.
# BuildKit cache mounts below give us dependency caching without that fragility.
#
# Build context = the REPO ROOT (filtered to these two paths by
# Dockerfile.dockerignore): api-server compiles in the agent policy with
# include_str!("<api-server>/../../roshera-agent-policy.md"), i.e. one level
# above the workspace, so the policy lands at /usr/src.
COPY roshera-backend/ .
COPY roshera-agent-policy.md /usr/src/roshera-agent-policy.md

# Build only the server binary (and exactly its dependency graph) in release.
# --mount=type=cache keeps the cargo registry and the target dir warm across
# builds without baking them into the image layer.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/src/roshera/target \
    cargo build --release -p api-server && \
    cp target/release/api-server /usr/local/bin/api-server

# ---------------------------------------------------------------------------
# Runtime stage
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    wget \
    && rm -rf /var/lib/apt/lists/*

# Non-root runtime user.
RUN useradd -m -u 1001 -s /bin/bash roshera

COPY --from=builder /usr/local/bin/api-server /usr/local/bin/api-server

# /app/exports  — export-engine output (STL/OBJ/STEP/ROS).
# /app/data      — mount point reserved for the durability slice (future
#                  snapshot + event-log storage). Nothing is written here yet;
#                  see docs/DEPLOYMENT.md "What does NOT survive a restart".
RUN mkdir -p /app/exports /app/data && chown -R roshera:roshera /app

USER roshera
WORKDIR /app

# The process binds one port: 0.0.0.0:8081 (main.rs). Metrics are served on
# the same listener at GET /api/metrics — there is no separate 9090 listener,
# so the previous `EXPOSE 3000 9090` was doubly wrong.
EXPOSE 8081

# Liveness probe against the real bind port and the real, auth-exempt route.
HEALTHCHECK --interval=30s --timeout=3s --start-period=20s --retries=3 \
    CMD wget -q --spider http://localhost:8081/health || exit 1

CMD ["api-server"]
