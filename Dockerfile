# Leveraging the pre-built Docker images with
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.74.0@sha256:32a7a5011c28e60da84a2a38b81a40d33f1589d70a6da9c91c7df6e6d98c2a2a AS chef
WORKDIR app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json
# Build application
COPY . .
RUN cargo build --release --bin kubit

# We do not need the Rust toolchain to run the binary!
FROM debian:bookworm-slim@sha256:2bc5c236e9b262645a323e9088dfa3bb1ecb16cc75811daf40a23a824d665be9 AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR app
COPY --from=builder /app/target/release/kubit /usr/local/bin
ENTRYPOINT ["/usr/local/bin/kubit"]
