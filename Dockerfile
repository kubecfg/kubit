# Leveraging the pre-built Docker images with
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.73.0@sha256:09ec7a922dc592d980f3fcfa97b873e1a678ad2fb252671569a65187f1cd4a75 AS chef
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
FROM debian:bookworm-slim@sha256:b55e2651b71408015f8068dd74e1d04404a8fa607dd2cfe284b4824c11f4d9bd AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR app
COPY --from=builder /app/target/release/kubit /usr/local/bin
ENTRYPOINT ["/usr/local/bin/kubit"]
