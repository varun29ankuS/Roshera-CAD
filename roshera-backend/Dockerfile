# Build stage
FROM rust:1.75 as builder

RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /usr/src/roshera

# Copy manifests for dependency caching
COPY Cargo.toml Cargo.lock ./
COPY shared-types/Cargo.toml shared-types/
COPY geometry-engine/Cargo.toml geometry-engine/
COPY session-manager/Cargo.toml session-manager/
COPY timeline-engine/Cargo.toml timeline-engine/
COPY api-server/Cargo.toml api-server/
COPY ai-integration/Cargo.toml ai-integration/
COPY export-engine/Cargo.toml export-engine/
COPY rag-engine/Cargo.toml rag-engine/

# Build dependencies (cached layer)
RUN mkdir -p shared-types/src && echo "pub fn stub() {}" > shared-types/src/lib.rs && \
    mkdir -p geometry-engine/src && echo "pub fn stub() {}" > geometry-engine/src/lib.rs && \
    mkdir -p session-manager/src && echo "pub fn stub() {}" > session-manager/src/lib.rs && \
    mkdir -p timeline-engine/src && echo "pub fn stub() {}" > timeline-engine/src/lib.rs && \
    mkdir -p api-server/src && echo "fn main() {}" > api-server/src/main.rs && \
    mkdir -p ai-integration/src && echo "pub fn stub() {}" > ai-integration/src/lib.rs && \
    mkdir -p export-engine/src && echo "pub fn stub() {}" > export-engine/src/lib.rs && \
    mkdir -p rag-engine/src && echo "pub fn stub() {}" > rag-engine/src/lib.rs && \
    cargo build --release 2>/dev/null || true && \
    find . -name "*.rs" -path "*/src/*" -delete

# Copy source code
COPY . .

# Build application
RUN cargo build --release

# Runtime stage
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    wget \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -m -u 1001 -s /bin/bash roshera

COPY --from=builder /usr/src/roshera/target/release/api-server /usr/local/bin/

RUN mkdir -p /app/exports && chown -R roshera:roshera /app

USER roshera
WORKDIR /app

EXPOSE 3000 9090

HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
    CMD wget -q --spider http://localhost:3000/health || exit 1

CMD ["api-server"]
