# Stage 1: Build
FROM rust:1.94-slim AS builder
WORKDIR /app

# Install build dependencies
# - pkg-config: required for crate build scripts
# - libssl-dev: needed at build time (reqwest links openssl transitively)
# - cmake, g++: required for fastembed/ONNX runtime (local-embed feature)
# Note: rusqlite uses bundled SQLite (no runtime libsqlite3 needed)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
        cmake \
        g++ \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src/ src/

# Build release binary with all default features (includes local-embed/fastembed)
RUN cargo build --release

# Stage 2: Runtime
FROM debian:trixie-slim

# ca-certificates: required for reqwest HTTPS calls to embedding/extraction APIs
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/brainjar /usr/local/bin/brainjar

# Data directory for SQLite DBs and config
ENV BRAINJAR_DATA_DIR=/data

# /data  — persistent volume for SQLite databases and config
# /kb    — mount your knowledge base source files here
VOLUME ["/data", "/kb"]

# Expose port (reserved for future HTTP API / MCP TCP transport)
EXPOSE 3333

ENTRYPOINT ["brainjar"]
CMD ["--help"]
