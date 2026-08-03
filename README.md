# Vault CLI MVP

This repository contains a regtest-only Rust/BDK implementation of the renewable Bitcoin vault described in [vault-design.md](vault-design.md). It uses a real Bitcoin Core node for policy validation, transaction relay, calendar locktimes, and block-based recovery.

## Run the narrative end-to-end demo

Docker is the only host dependency:

```bash
./scripts/run-e2e.sh
```

The script always removes this Compose project's old containers and volumes, starts a fresh regtest chain, and runs every behavior serially. It funds the primary vault with exactly 2 BTC, creates the twelve 0.1 BTC monthly authorizations and revocations, exercises calendar and soft-limit behavior, then mines the real 61,200/65,535-block recovery delays while demonstrating every documented loss/theft scenario. The recovery portion is intentionally slow.

## Run all tests

```bash
./scripts/run-tests.sh
```

This runs unit and CLI tests, focused real-node integration tests, and the slow recovery integration test. For fast local development without Docker:

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

## Use the CLI manually

Start Core and invoke the persistent CLI service:

```bash
docker compose --profile manual up -d bitcoind
docker compose run --rm cli init
docker compose run --rm cli policy
docker compose run --rm cli status
```

Everything is hard-wired to regtest and 1 sat/vB. Mnemonics are intentionally printed by the simulated devices for demonstration; this is not production key handling.
