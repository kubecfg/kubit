# Leveraging the pre-built Docker images with
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.73.0@sha256:e7b86439d9c95b5d05f248819e556677fd24bcde38758f10841d4d45279cdf24 AS chef
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
FROM debian:bookworm-slim@sha256:6cc67f78e0e8295b4fbe055eba0356648f149daf15db9179aa51fcfa9cc131cd AS runtime
RUN apt-get update && apt-get install -y \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*
WORKDIR app
COPY --from=builder /app/target/release/kubit /usr/local/bin
ENTRYPOINT ["/usr/local/bin/kubit"]
