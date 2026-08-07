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
COPY test-vectors ./test-vectors
RUN touch src/*.rs tests/*.rs
RUN cargo test --all-targets --locked --no-run
COPY scripts ./scripts
ENTRYPOINT ["/build/scripts/docker-test.sh"]

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends bash ca-certificates jq libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/anzen /usr/local/bin/anzen
COPY scripts /opt/anzen/scripts
COPY anzen-design.md worklog.md /opt/anzen/

WORKDIR /data
ENTRYPOINT ["anzen"]
