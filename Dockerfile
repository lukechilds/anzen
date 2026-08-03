FROM rust:1.85-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends libsqlite3-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
COPY tests ./tests
RUN cargo build --release --locked

FROM builder AS test
RUN cargo test --all-targets --locked --no-run
COPY scripts ./scripts
ENTRYPOINT ["/build/scripts/docker-test.sh"]

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates jq libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/vault-cli /usr/local/bin/vault-cli
COPY scripts /opt/vault/scripts
COPY vault-design.md worklog.md /opt/vault/

WORKDIR /data
ENTRYPOINT ["vault-cli"]
