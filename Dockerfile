# Leveraging the pre-built Docker images with
# cargo-chef and the Rust toolchain
FROM lukemathwalker/cargo-chef:latest-rust-1.71.0@sha256:899b15ba862c30466e0fe9f2d99068ca77b229cd7cbad5f86c4e52e23d71f895 AS chef
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
FROM debian:bullseye-slim@sha256:fd3b382990294beb46aa7549edb9f40b11a070f959365ef7f316724b2e425f90 AS runtime
WORKDIR app
COPY --from=builder /app/target/release/kubit /usr/local/bin
ENTRYPOINT ["/usr/local/bin/kubit"]
