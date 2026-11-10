FROM rust:1.88-bookworm AS base

# libdbus: `anzen-link` reaches Bluetooth through BlueZ on Linux, over D-Bus. Only the workspace's
# tests build that crate here; the `anzen` binary does not depend on it.
RUN apt-get update \
    && apt-get install -y --no-install-recommends jq libdbus-1-dev libsqlite3-dev pkg-config \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# Reduce the workspace to its manifests plus placeholder targets. Only this stage's output is
# copied into the dependency stages, so their layers stay cached while application code changes.
# Cargo lists every target it would build, declared or discovered, so each gets a placeholder
# and new crates, binaries, tests or examples need no change here.
FROM base AS manifests

COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN cargo metadata --no-deps --format-version 1 --locked \
        | jq -r '.packages[].targets[].src_path' > /tmp/targets \
    && find crates -type f ! -name Cargo.toml -delete \
    && while read -r target; do \
        mkdir -p "$(dirname "$target")" && printf 'fn main() {}\n' > "$target"; \
    done < /tmp/targets

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
