# syntax=docker/dockerfile:1.7
FROM rust:1.98-bookworm AS builder
ENV CARGO_HTTP_TIMEOUT=30 CARGO_NET_RETRY=2
WORKDIR /work
COPY Cargo.toml Cargo.lock ./
COPY contracts ./contracts
COPY backend ./backend
RUN --mount=type=cache,id=po-cargo-registry,target=/usr/local/cargo/registry,sharing=locked \
    --mount=type=cache,id=po-cargo-target-backend,target=/work/target,sharing=locked \
    cargo build --locked --release -p linux-pilot-server && \
    cp target/release/linux-pilot-server /usr/local/bin/linux-pilot-server

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /usr/local/bin/linux-pilot-server /usr/local/bin/linux-pilot-server
COPY backend/appliaction.yaml /etc/linux-pilot/appliaction.yaml
ENV PO_CONFIG=/etc/linux-pilot/appliaction.yaml
EXPOSE 8080 50051
CMD ["linux-pilot-server"]
