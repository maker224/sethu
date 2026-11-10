# syntax=docker/dockerfile:1

# ---------- build ----------
FROM rust:1-bookworm AS builder
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY web ./web
RUN cargo build --release --locked

# ---------- runtime ----------
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 --home-dir /data --shell /usr/sbin/nologin sethu \
    && install -d -o sethu -g sethu /data

COPY --from=builder /src/target/release/sethu /usr/local/bin/sethu

ENV SETHU_BIND=0.0.0.0:8080 \
    SETHU_DATA_DIR=/data \
    RUST_LOG=sethu_server=info

USER sethu
VOLUME ["/data"]
EXPOSE 8080

CMD ["sethu"]
