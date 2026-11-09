FROM rust:1.88-bookworm AS base

RUN apt-get update \
    && apt-get install -y --no-install-recommends libsqlite3-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Reduce the workspace to its manifests plus placeholder targets. Only this stage's output is
# copied into the dependency stages, so their layers stay cached while application code changes.
FROM base AS manifests

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN find crates -type f ! -name Cargo.toml -delete \
    && for crate in crates/*; do \
        mkdir -p "$crate/src" && printf '' > "$crate/src/lib.rs"; \
    done \
    && printf 'fn main() {}\n' > crates/anzen-cli/src/main.rs \
    && printf 'fn main() {}\n' > crates/anzen-playground/src/main.rs \
    && mkdir -p crates/anzen-vanity/examples crates/anzen-vanity/tests \
    && printf 'fn main() {}\n' > crates/anzen-vanity/examples/benchmark.rs \
    && printf '' > crates/anzen-vanity/tests/gpu.rs \
    && rm crates/anzen-cli/src/lib.rs crates/anzen-playground/src/lib.rs

FROM base AS release-dependencies

COPY --from=manifests /build ./
RUN cargo build --release --locked -p anzen-cli

FROM release-dependencies AS builder

COPY crates ./crates
RUN find crates -name '*.rs' -exec touch {} + && cargo build --release --locked -p anzen-cli

FROM base AS test-dependencies

COPY --from=manifests /build ./
RUN cargo test --workspace --all-targets --locked --no-run

FROM test-dependencies AS test

COPY crates ./crates
COPY test-vectors ./test-vectors
RUN find crates -name '*.rs' -exec touch {} +
RUN cargo test --workspace --all-targets --locked --no-run
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
