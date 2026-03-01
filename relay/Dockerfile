FROM rust:1.83-slim AS builder

RUN apt-get update && apt-get install -y pkg-config libssl-dev && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Copy workspace manifests and all crate Cargo.tomls for dependency resolution
COPY jarvis-rs/Cargo.toml jarvis-rs/Cargo.lock ./
COPY jarvis-rs/crates/jarvis-relay/Cargo.toml crates/jarvis-relay/Cargo.toml
COPY jarvis-rs/crates/jarvis-app/Cargo.toml crates/jarvis-app/Cargo.toml
COPY jarvis-rs/crates/jarvis-common/Cargo.toml crates/jarvis-common/Cargo.toml
COPY jarvis-rs/crates/jarvis-config/Cargo.toml crates/jarvis-config/Cargo.toml
COPY jarvis-rs/crates/jarvis-platform/Cargo.toml crates/jarvis-platform/Cargo.toml
COPY jarvis-rs/crates/jarvis-tiling/Cargo.toml crates/jarvis-tiling/Cargo.toml
COPY jarvis-rs/crates/jarvis-renderer/Cargo.toml crates/jarvis-renderer/Cargo.toml
COPY jarvis-rs/crates/jarvis-ai/Cargo.toml crates/jarvis-ai/Cargo.toml
COPY jarvis-rs/crates/jarvis-social/Cargo.toml crates/jarvis-social/Cargo.toml
COPY jarvis-rs/crates/jarvis-webview/Cargo.toml crates/jarvis-webview/Cargo.toml

# Stub all crate sources so cargo can resolve the workspace
RUN mkdir -p crates/jarvis-relay/src && echo 'fn main() {}' > crates/jarvis-relay/src/main.rs && \
    mkdir -p crates/jarvis-app/src && echo '' > crates/jarvis-app/src/lib.rs && \
    mkdir -p crates/jarvis-common/src && echo '' > crates/jarvis-common/src/lib.rs && \
    mkdir -p crates/jarvis-config/src && echo '' > crates/jarvis-config/src/lib.rs && \
    mkdir -p crates/jarvis-platform/src && echo '' > crates/jarvis-platform/src/lib.rs && \
    mkdir -p crates/jarvis-tiling/src && echo '' > crates/jarvis-tiling/src/lib.rs && \
    mkdir -p crates/jarvis-renderer/src && echo '' > crates/jarvis-renderer/src/lib.rs && \
    mkdir -p crates/jarvis-ai/src && echo '' > crates/jarvis-ai/src/lib.rs && \
    mkdir -p crates/jarvis-social/src && echo '' > crates/jarvis-social/src/lib.rs && \
    mkdir -p crates/jarvis-webview/src && echo '' > crates/jarvis-webview/src/lib.rs

# Cache dependencies
RUN cargo build --release -p jarvis-relay 2>/dev/null || true

# Copy real relay source
COPY jarvis-rs/crates/jarvis-relay/src crates/jarvis-relay/src

# Build for real
RUN touch crates/jarvis-relay/src/main.rs && \
    cargo build --release -p jarvis-relay

# --- Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/jarvis-relay /usr/local/bin/jarvis-relay

ENV PORT=8080
EXPOSE 8080

CMD ["jarvis-relay", "--port", "8080"]
