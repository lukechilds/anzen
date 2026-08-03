# Vault CLI MVP

[![CI](https://github.com/lukechilds/vault/actions/workflows/ci.yml/badge.svg)](https://github.com/lukechilds/vault/actions/workflows/ci.yml)

This repository contains a regtest-only Rust/BDK implementation of the renewable Bitcoin vault described in [vault-design.md](vault-design.md). It uses a real Bitcoin Core node for policy validation, transaction relay, calendar locktimes, and block-based recovery.

## Run the end-to-end tests

Docker is the only host dependency:

```bash
./scripts/run-e2e.sh --list
./scripts/run-e2e.sh monthly-spend
./scripts/run-e2e.sh lost-phone lost-hww
./scripts/run-e2e.sh all
```

With no arguments, the runner behaves like `all`. Selected tests run serially, and each gets a fresh regtest chain and vault state so it can be read and reproduced independently. Separate runner invocations also use isolated Compose projects, so concurrent local tests cannot stop or erase one another. Output is limited to user actions, the corresponding CLI commands, essential policy/transaction results, expected safety rejections, and compact mining progress. Displayed commands omit the internal `--data-dir` argument, retain the terminal's default color, and show their results in muted grey. Every completed step starts a new paragraph with a short `✅` outcome so the test can be understood by skimming those lines.

The named tests cover setup/policy, monthly spend, monthly revoke, partial funding, lost or stolen phone, lost or stolen HWW, missing cloud backup, both devices lost, cloud compromise, both keys compromised, and forgotten rollover. The spend demonstrations fund exactly 2 BTC and build twelve 0.1 BTC allowances.

Recovery tests mine the real 61,200/65,535-block CSV delays. Running one is intentionally slow; running `all` is substantially slower because every recovery test proves its delay on an independent chain.

## Continuous integration

GitHub Actions runs formatting, Clippy, unit/CLI tests, the real Bitcoin Core integration suite, and every isolated end-to-end test on pushes to `main` and on every pull request. The workflow can also be started manually. End-to-end tests run serially through `./scripts/run-e2e.sh all`; superseded runs on the same branch are cancelled because the exact recovery-delay tests take a while.

The quality job restores Cargo registry and `target/` data with the GitHub Actions cache. Docker jobs build through Buildx with separate GHA-backed `test` and `runtime` cache scopes, load the resulting images into the runner, and tell the Compose scripts to use those images without rebuilding. The Dockerfile compiles dependencies before copying application source, so dependency layers remain reusable when Rust code changes.

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
