# syntax=docker/dockerfile:1

FROM rust:1.95-bookworm AS builder
WORKDIR /workspace

RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

COPY . .
RUN cargo build --release -p nas-csi-driver

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /workspace/target/release/nas-csi-driver /usr/local/bin/nas-csi-controller

ENTRYPOINT ["/usr/local/bin/nas-csi-controller"]
