#!/usr/bin/env bash
set -euo pipefail

cd /build

echo "== Unit, CLI, and compile-time integration tests =="
cargo test --all-targets --locked -- --test-threads=1

echo "== Real Bitcoin Core RPC/BDK integration =="
cargo test --locked --test regtest_rpc -- --ignored --nocapture --test-threads=1

echo "== Real rollover/monthly/revocation integration =="
cargo test --locked --test regtest_ceremony -- --ignored --nocapture --test-threads=1

echo "== Slow real 61,200/65,535-block recovery integration =="
cargo test --locked --test regtest_recovery -- --ignored --nocapture --test-threads=1
