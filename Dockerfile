# ==========================================
# Phase 1: Build Rust Backend
# ==========================================
FROM rust:1.85-slim AS builder

WORKDIR /app
# Install build dependencies
RUN apt-get update && apt-get install -y pkg-config libssl-dev curl ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy backend source code
COPY mcp-gateway/ ./

# Build the workspace
RUN cargo build --release

# ==========================================
# Phase 2: Runtime Environment
# ==========================================
FROM debian:bookworm-slim

WORKDIR /app

# Install common MCP runtimes (Node.js, Python, Git)
# Hugging Face Spaces default user ID is 1000, debian works well with it.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    git \
    nodejs \
    npm \
    python3 \
    python3-pip \
    python3-venv \
    && rm -rf /var/lib/apt/lists/*

RUN python3 -m venv /opt/venv
ENV PATH="/opt/venv/bin:$PATH"

# Copy binary from builder
COPY --from=builder /app/target/release/gateway /usr/local/bin/mcp-gateway

# Prepare config file
COPY mcp-gateway/config.v2.json /opt/bootstrap/default-config.json

# Modify default listen address for Hugging Face (0.0.0.0:7860)
RUN sed -i 's/"listen": "127.0.0.1:8765"/"listen": "0.0.0.0:7860"/g' /opt/bootstrap/default-config.json && \
    sed -i 's/"allowNonLoopback": false/"allowNonLoopback": true/g' /opt/bootstrap/default-config.json && \
    mkdir -p /data/config /data/skills /data/www

# Expose Hugging Face mandatory port
EXPOSE 7860

# Set environment variables
ENV RUST_LOG=info
ENV HOME=/app
ENV XDG_CONFIG_HOME=/app
ENV MCP_GATEWAY_CONFIG=/data/config/config.json
ENV MCP_SKILLS_ROOT=/data/skills

#╧рть/dada
# Start the server
# Use 'server' subcommand from gateway-cli
CMD ["sh", "-c", "mkdir -p /data/config /data/skills && if [ ! -f \"$MCP_GATEWAY_CONFIG\" ]; then cp /opt/bootstrap/default-config.json \"$MCP_GATEWAY_CONFIG\"; fi && exec mcp-gateway run --config \"$MCP_GATEWAY_CONFIG\""]