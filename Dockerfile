FROM rust:1.85-bookworm AS base

RUN apt-get update \
    && apt-get install -y --no-install-recommends libsqlite3-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY Cargo.toml Cargo.lock ./

# Give Cargo source-independent placeholder targets. The release and test dependency
# stages below can then be restored when application code changes.
RUN mkdir -p src \
    && printf 'fn main() {}\n' > src/main.rs \
    && printf '' > src/lib.rs

FROM base AS release-dependencies

RUN cargo build --release --locked

FROM release-dependencies AS builder

COPY src ./src
RUN touch src/*.rs && cargo build --release --locked

FROM base AS test-dependencies

RUN cargo test --lib --locked --no-run

FROM test-dependencies AS test

COPY src ./src
COPY tests ./tests
RUN touch src/*.rs tests/*.rs
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
