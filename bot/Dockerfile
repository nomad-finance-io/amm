# syntax=docker/dockerfile:1.7
#
# Build context must be the repo root (so path deps `../client` and
# `../programs/cp-swap` resolve). From the repo root:
#
#   docker build -f bot/Dockerfile -t raydium-bot .
#
FROM rust:1.83-slim-bookworm AS builder
WORKDIR /workspace

RUN apt-get update \
 && apt-get install -y --no-install-recommends pkg-config libssl-dev ca-certificates \
 && rm -rf /var/lib/apt/lists/*

# Copy the path-dep crates first so cached layers survive bot/ source changes.
COPY programs/cp-swap programs/cp-swap
COPY client client

# Stage 1: prime the dep cache with a stub bot main.
COPY bot/Cargo.toml bot/Cargo.toml
RUN mkdir -p bot/src && echo 'fn main() {}' > bot/src/main.rs \
 && cargo build --manifest-path bot/Cargo.toml --release \
 && rm -rf bot/src bot/target/release/deps/bot* bot/target/release/bot*

# Stage 2: real source.
COPY bot/src bot/src
RUN cargo build --manifest-path bot/Cargo.toml --release

FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app
COPY --from=builder /workspace/bot/target/release/bot /app/bot
USER nonroot
ENTRYPOINT ["/app/bot"]
