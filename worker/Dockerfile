# syntax=docker/dockerfile:1.7
FROM rust:1.98-bookworm AS builder
ENV CARGO_HTTP_TIMEOUT=30 CARGO_NET_RETRY=2
WORKDIR /work
COPY Cargo.toml Cargo.lock ./
COPY contracts ./contracts
COPY src ./src
COPY ebpf ./ebpf
RUN --mount=type=cache,id=po-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=po-cargo-target-worker,target=/work/target,sharing=locked \
    cargo build --locked --release -p linux-pilot-worker && \
    cargo test --locked --release -p linux-pilot-worker && \
    cp target/release/linux-pilot-worker /usr/local/bin/linux-pilot-worker

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates linux-perf && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/bin/linux-pilot-worker /usr/local/bin/linux-pilot-worker
ENV PO_AGENT__SPOOL_DIR=/var/lib/linux-pilot-worker/spool
CMD ["linux-pilot-worker"]
