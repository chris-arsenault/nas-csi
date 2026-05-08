# syntax=docker/dockerfile:1

FROM rust:1.95-bookworm AS builder
WORKDIR /workspace

COPY . .
RUN cargo build --release -p nas-csi-node-plugin

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates mount \
    && rm -rf /var/lib/apt/lists/* \
    && command -v mount \
    && command -v umount

COPY --from=builder /workspace/target/release/nas-csi-node-plugin /usr/local/bin/nas-csi-node

ENTRYPOINT ["/usr/local/bin/nas-csi-node"]
