#!/usr/bin/env bash
set -euo pipefail

cd /build

echo "== Unit, CLI, and compile-time integration tests =="
cargo test --workspace --all-targets --locked -- --test-threads=1

echo "== Real Bitcoin Core RPC/BDK integration =="
cargo test --locked -p anzen-integration-tests --test regtest_rpc -- --ignored --nocapture --test-threads=1

echo "== Real rollover/monthly/revocation integration =="
cargo test --locked -p anzen-integration-tests --test regtest_ceremony -- --ignored --nocapture --test-threads=1

echo "== Real time-locked signer recovery and rotation integration =="
cargo test --locked -p anzen-integration-tests --test regtest_recovery -- --ignored --nocapture --test-threads=1

echo "== Live-update refresh cost on a long chain =="
cargo test --locked -p anzen-integration-tests --test regtest_live_refresh -- --ignored --nocapture --test-threads=1
