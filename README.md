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

The named tests cover setup/policy, monthly spend, monthly revoke, partial funding, lost or stolen phone, lost or stolen HWW, missing cloud backup, both devices lost, cloud compromise, both keys compromised, and both on-time and forgotten annual rollover. The spend demonstrations fund exactly 2 BTC and build twelve 0.1 BTC allowances.

Recovery tests mine the real 61,200/65,535-block CSV delays, and the on-time rollover test mines a 52,560-block year before continuing to the old recovery deadline. Running one is intentionally slow; running `all` is substantially slower because every long-delay test proves its behavior on an independent chain.

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

Start Core and define a small shell helper. The Compose volume preserves vault state and JSON handoff files between CLI invocations:

```bash
docker compose --profile manual up -d bitcoind
vault() { docker compose run --rm cli "$@"; }
```

Everything is hard-wired to regtest and 1 sat/vB. Mnemonics are intentionally printed by the simulated devices for demonstration; this is not production key handling. The examples below assume the vault has confirmed regtest funds; `./scripts/run-e2e.sh monthly-spend` demonstrates funding 2 BTC from a freshly mined hot wallet.

### Create a vault

Initialize each simulated device separately, then combine their public keys into the static cold-storage policy:

```bash
vault phone init
vault hww init
vault init
vault policy
```

The new vault starts with monthly spending disabled. `vault init` prints the static cold-storage descriptor, vault address, and recovery delays, but does not create or sign a monthly spending policy.

### Set or replace the monthly policy

The phone proposes the policy and signs its side of every PSBT. The HWW independently validates the high-level policy, asks for one approval, and signs the complete batch. The phone verifies the approved JSON, broadcasts the rollover, and stores each authorization and revocation as an individually encrypted artifact:

```bash
vault phone set-policy --monthly-limit 10000000 --output policy.json
vault hww confirm-policy policy.json --output approved-policy.json
vault phone activate-policy approved-policy.json
vault policy
```

`10000000` sats is 0.1 BTC. Set `--monthly-limit 0` through the same three-step protocol to disable monthly authorizations while still rolling all funds into cold storage. Policy JSON may also be piped with `--output -`; file handoff is clearer for the interactive HWW approval.

### Execute a monthly spend

The month is the calendar month recorded in the active schedule. An authorization becomes valid once Bitcoin median-time-past is beyond 00:00 UTC on its first day:

```bash
vault phone authorize 2026-09
```

To keep only a 0.01 BTC soft limit from a 0.1 BTC authorization, immediately return the difference to cold storage:

```bash
vault phone apply-soft-limit 2026-09 --limit 1000000
```

The signed monthly limit is the security boundary. The adjustable soft limit is a phone-side action and may be any value from zero through the signed monthly limit.

### Revoke a future monthly spend

Before an authorization matures, the phone can broadcast its conflicting presigned revocation without the HWW:

```bash
vault phone revoke 2026-10
```

Once the revocation confirms, the corresponding authorization can no longer spend that monthly chunk.

### Replace a lost phone

If the encrypted cloud backup survives, the HWW decrypts it into a portable recovery object. After installing that object on the replacement phone, rotate immediately to a fresh phone key and vault address:

```bash
vault hww decrypt-phone-backup \
  .vault-data/cloud/phone-seed-backup.json \
  --output phone-recovery.json
vault phone restore phone-recovery.json

vault phone rotate-key --output phone-rotation.json
vault hww confirm-rotation phone-rotation.json \
  --output approved-phone-rotation.json
vault phone activate-rotation approved-phone-rotation.json
```

The rotation preserves the HWW key, creates a new phone seed and HWW-encrypted backup, sweeps the old vault cooperatively, and disables monthly spending until a fresh policy is approved.

If the phone and its backup are permanently unavailable, initialize a replacement vault, wait the real 65,535-block HWW delay, and recover directly to its address:

```bash
vault --data-dir .replacement-vault phone init
vault --data-dir .replacement-vault hww init
vault --data-dir .replacement-vault init
vault --data-dir .replacement-vault policy

# Copy the replacement vault address printed above.
vault hww recover "$REPLACEMENT_VAULT_ADDRESS"
```

This delayed recovery moves the funds to a new phone key, a new HWW key, and a new static vault address.

### Replace a lost HWW

The phone can continue using existing monthly artifacts while the recovery delay runs. Initialize a replacement vault, wait the real 61,200-block phone delay, then sweep the old vault into its address:

```bash
vault --data-dir .replacement-vault phone init
vault --data-dir .replacement-vault hww init
vault --data-dir .replacement-vault init
vault --data-dir .replacement-vault policy

# Copy the replacement vault address printed above.
vault phone recover "$REPLACEMENT_VAULT_ADDRESS"
```

The replacement vault has a fresh HWW key (and a fresh phone epoch), so the missing HWW can no longer participate. Approve a new monthly policy after the recovery confirms.

### Other cooperative sweeps

Arbitrary immediate vault sweeps retain the same explicit device boundary:

```bash
vault phone create-sweep "$DESTINATION" --output sweep.json
vault hww confirm-sweep sweep.json --output approved-sweep.json
vault phone broadcast-sweep approved-sweep.json
```
