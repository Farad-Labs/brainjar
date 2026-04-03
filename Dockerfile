# Stage 1: Build
FROM rust:1.86-slim AS builder
WORKDIR /app

# Install build dependencies
# - pkg-config: required for crate build scripts
# - libssl-dev: needed at build time (reqwest links openssl transitively)
# Note: rusqlite uses bundled SQLite (no runtime libsqlite3 needed)
RUN apt-get update && \
    apt-get install -y --no-install-recommends \
        pkg-config \
        libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src/ src/

# Build release binary
# Note: brainjar has no local-embed/fastembed dependency —
# embeddings are provided by external API backends (Gemini, OpenAI, Ollama).
# SQLite is bundled; reqwest uses rustls so no runtime libssl needed.
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim

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
