# ---- build ----
FROM rust:1.91-slim AS builder
WORKDIR /app

# Cache dependency compilation separately from the sources.
COPY Cargo.toml Cargo.lock ./
COPY stemson-macros ./stemson-macros
COPY src ./src
RUN cargo build --release --locked

# ---- runtime ----
FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates tzdata \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
COPY --from=builder /app/target/release/stemson /usr/local/bin/stemson
COPY config ./config

ENV STEMSON_CONFIG=/app/config/config.docker.yaml
EXPOSE 8080
CMD ["stemson"]
