# Vault CLI MVP

[![CI](https://github.com/lukechilds/vault/actions/workflows/ci.yml/badge.svg)](https://github.com/lukechilds/vault/actions/workflows/ci.yml)

This repository contains a regtest-only Rust/BDK implementation of the renewable Bitcoin vault described in [vault-design.md](vault-design.md). It uses a real Bitcoin Core node for policy validation, transaction relay, calendar locktimes, and block-based recovery.

## Use the CLI manually

Start Core, then define the `vault` shell helper. The helper itself has no output. The Compose volume preserves vault state and JSON handoff files between invocations:

```console
$ docker compose --profile manual up -d bitcoind
[+] Running 1/1
 ✔ Container vault-bitcoind-1  Healthy

$ vault() { docker compose run --rm cli "$@"; }
```

Everything is hard-wired to regtest and 1 sat/vB. Mnemonics are intentionally printed by the simulated devices for demonstration; this is not production key handling. Output below is representative: generated mnemonics, keys, addresses, and transaction IDs change on every run. The examples assume the vault has confirmed regtest funds; `./scripts/run-e2e.sh monthly-spend` demonstrates funding 2 BTC from a freshly mined hot wallet.

### Create a vault

Initialize each simulated device separately, then combine their public keys into the static cold-storage policy:

```console
$ vault phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: <generated phone mnemonic>
Phone vault key: <phone public key>

$ vault hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: <generated HWW mnemonic>
HWW vault key: <HWW public key>
Phone backup encrypted for the HWW

$ vault init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(<NUMS key>,{multi_a(2,<phone key>,<HWW key>),{...}})
Vault address: bcrt1p<generated vault address>
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ vault policy
Cold storage descriptor: tr(<NUMS key>,{multi_a(2,<phone key>,<HWW key>),{...}})
Vault address: bcrt1p<generated vault address>
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled
```

The new vault starts with monthly spending disabled. `vault init` prints the static cold-storage descriptor, vault address, and recovery delays, but does not create or sign a monthly spending policy.

### Set or replace the monthly policy

The phone proposes the policy and signs its side of every PSBT. The HWW independently validates the high-level policy, asks for one approval, and signs the complete batch. The phone verifies the approved JSON, broadcasts the rollover, and stores each authorization and revocation as an individually encrypted artifact:

```console
$ vault phone set-policy --monthly-limit 10000000 --output policy.json
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(<vault descriptor>)
Vault address: bcrt1p<vault address>
Monthly limit: 10000000 sats
Fee rate: 1 sat/vB
Total input: 200000000 sats
Monthly pairs: 12
Rollover txid: <rollover txid>
Rollover fee: 635 sats
Phone signed PSBTs: 25
Phone-signed policy proposal: policy.json

$ vault hww confirm-policy policy.json --output approved-policy.json
SIMULATED HWW — ONE HIGH-LEVEL POLICY APPROVAL
PHONE POLICY PROPOSAL
Cold storage descriptor: tr(<vault descriptor>)
Vault address: bcrt1p<vault address>
Monthly limit: 10000000 sats
Fee rate: 1 sat/vB
Total input: 200000000 sats
Monthly pairs: 12
Rollover txid: <rollover txid>
Rollover fee: 635 sats
Phone signed PSBTs: 25
Type `approve` to confirm the complete monthly policy: approve
HWW validated and signed all 25 PSBTs after one approval
HWW-approved policy: approved-policy.json

$ vault phone activate-policy approved-policy.json
Rollover broadcast: <rollover txid>
Active monthly limit: 10000000 sats
Encrypted monthly transaction pairs: 12

$ vault policy
Cold storage descriptor: tr(<vault descriptor>)
Vault address: bcrt1p<vault address>
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly limit: 10000000 sats
Presigned monthly transaction pairs: 12
```

`10000000` sats is 0.1 BTC. Set `--monthly-limit 0` through the same three-step protocol to disable monthly authorizations while still rolling all funds into cold storage. Policy JSON may also be piped with `--output -`; file handoff is clearer for the interactive HWW approval.

### Execute a monthly spend

The month is the calendar month recorded in the active schedule. An authorization becomes valid once Bitcoin median-time-past is beyond 00:00 UTC on its first day:

```console
$ vault phone authorize 2026-09
Broadcast Authorization for 2026-09: <authorization txid>
```

To keep only a 0.01 BTC soft limit from a 0.1 BTC authorization, immediately return the difference to cold storage:

```console
$ vault phone apply-soft-limit 2026-09 --limit 1000000
Soft limit applied for 2026-09: retained at most 1000000 sats hot; cold-return txid=<cold-return txid>
```

The signed monthly limit is the security boundary. The adjustable soft limit is a phone-side action and may be any value from zero through the signed monthly limit.

### Revoke a future monthly spend

Before an authorization matures, the phone can broadcast its conflicting presigned revocation without the HWW:

```console
$ vault phone revoke 2026-10
Broadcast Revocation for 2026-10: <revocation txid>
```

Once the revocation confirms, the corresponding authorization can no longer spend that monthly chunk.

### Replace a lost phone

If the encrypted cloud backup survives, the HWW decrypts it into a portable recovery object. After installing that object on the replacement phone, rotate immediately to a fresh phone key and vault address:

```console
$ vault hww decrypt-phone-backup \
  .vault-data/cloud/phone-seed-backup.json \
  --output phone-recovery.json
Decrypted phone recovery package: phone-recovery.json

$ vault phone restore phone-recovery.json
Phone key restored from HWW recovery package
Recovered phone mnemonic: <recovered phone mnemonic>

$ vault phone rotate-key --output phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: <new phone public key>
New cold storage descriptor: tr(<new vault descriptor>)
New vault address: bcrt1p<new vault address>
Inputs: 12
Sent: <sats moved to the new vault>
Fee: <fee> sats (1 sat/vB)
Phone-key rotation proposal: phone-rotation.json

$ vault hww confirm-rotation phone-rotation.json \
  --output approved-phone-rotation.json
PHONE-KEY ROTATION
New phone vault key: <new phone public key>
New cold storage descriptor: tr(<new vault descriptor>)
New vault address: bcrt1p<new vault address>
Inputs: 12
Sent: <sats moved to the new vault>
Fee: <fee> sats (1 sat/vB)
Type `approve` to confirm the phone-key rotation: approve
HWW validated and signed the phone-key rotation
HWW-approved phone-key rotation: approved-phone-rotation.json

$ vault phone activate-rotation approved-phone-rotation.json
Emergency phone-key rotation broadcast: <rotation txid>
Old vault address: bcrt1p<old vault address>
New vault address: bcrt1p<new vault address>
New phone mnemonic: <new phone mnemonic>
Monthly spending: disabled until a new policy is approved
```

The rotation preserves the HWW key, creates a new phone seed and HWW-encrypted backup, sweeps the old vault cooperatively, and disables monthly spending until a fresh policy is approved.

If the phone and its backup are permanently unavailable, initialize a replacement vault, wait the real 65,535-block HWW delay, and recover directly to its address:

```console
$ vault --data-dir .replacement-vault phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: <replacement phone mnemonic>
Phone vault key: <replacement phone public key>

$ vault --data-dir .replacement-vault hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: <replacement HWW mnemonic>
HWW vault key: <replacement HWW public key>
Phone backup encrypted for the HWW

$ vault --data-dir .replacement-vault init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(<replacement vault descriptor>)
Vault address: bcrt1p<replacement vault address>
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ vault --data-dir .replacement-vault policy
Cold storage descriptor: tr(<replacement vault descriptor>)
Vault address: bcrt1p<replacement vault address>
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

# Copy the replacement vault address printed above.
$ vault hww recover "$REPLACEMENT_VAULT_ADDRESS"
HWW recovery sweep broadcast: <recovery txid>
Inputs: <mature vault UTXO count>
Sent: <recovered sats>
Fee: <fee> sats (1 sat/vB)
```

This delayed recovery moves the funds to a new phone key, a new HWW key, and a new static vault address.

### Replace a lost HWW

The phone can continue using existing monthly artifacts while the recovery delay runs. Initialize a replacement vault, wait the real 61,200-block phone delay, then sweep the old vault into its address:

```console
$ vault --data-dir .replacement-vault phone init
Simulated phone initialized (REGTEST ONLY)
Phone mnemonic: <replacement phone mnemonic>
Phone vault key: <replacement phone public key>

$ vault --data-dir .replacement-vault hww init
Simulated HWW initialized (REGTEST ONLY)
HWW mnemonic: <replacement HWW mnemonic>
HWW vault key: <replacement HWW public key>
Phone backup encrypted for the HWW

$ vault --data-dir .replacement-vault init
Vault initialized (REGTEST ONLY)
Cold storage descriptor: tr(<replacement vault descriptor>)
Vault address: bcrt1p<replacement vault address>
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

$ vault --data-dir .replacement-vault policy
Cold storage descriptor: tr(<replacement vault descriptor>)
Vault address: bcrt1p<replacement vault address>
Phone recovery: 61,200 blocks (~14 months)
HWW recovery:   65,535 blocks (~15 months)
Monthly spending: disabled

# Copy the replacement vault address printed above.
$ vault phone recover "$REPLACEMENT_VAULT_ADDRESS"
Phone recovery sweep broadcast: <recovery txid>
Inputs: <mature vault UTXO count>
Sent: <recovered sats>
Fee: <fee> sats (1 sat/vB)
```

The replacement vault has a fresh HWW key (and a fresh phone epoch), so the missing HWW can no longer participate. Approve a new monthly policy after the recovery confirms.

### Other cooperative sweeps

Arbitrary immediate vault sweeps retain the same explicit device boundary:

```console
$ vault phone create-sweep "$DESTINATION" --output sweep.json
COOPERATIVE VAULT SWEEP
Destination: bcrt1p<destination address>
Inputs: <vault UTXO count>
Sent: <sats sent>
Fee: <fee> sats (1 sat/vB)
Phone signed: true
Phone-signed cooperative sweep: sweep.json

$ vault hww confirm-sweep sweep.json --output approved-sweep.json
COOPERATIVE VAULT SWEEP
Destination: bcrt1p<destination address>
Inputs: <vault UTXO count>
Sent: <sats sent>
Fee: <fee> sats (1 sat/vB)
Phone signed: true
Type `approve` to confirm the cooperative sweep: approve
HWW validated and signed the cooperative sweep
HWW-approved cooperative sweep: approved-sweep.json

$ vault phone broadcast-sweep approved-sweep.json
Cooperative vault sweep broadcast: <sweep txid>
Inputs: <vault UTXO count>
Sent: <sats sent>
Fee: <fee> sats (1 sat/vB)
```

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

## Run all tests

```bash
./scripts/run-tests.sh
```

This runs unit and CLI tests, focused real-node integration tests, and the slow recovery integration test. For fast local development without Docker:

```bash
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

## Continuous integration

GitHub Actions runs formatting, Clippy, unit/CLI tests, the real Bitcoin Core integration suite, and every isolated end-to-end test on pushes to `main` and on every pull request. The workflow can also be started manually. End-to-end tests run serially through `./scripts/run-e2e.sh all`; superseded runs on the same branch are cancelled because the exact recovery-delay tests take a while.

The quality job restores Cargo registry and `target/` data with the GitHub Actions cache. Docker jobs build through Buildx with separate GHA-backed `test` and `runtime` cache scopes, load the resulting images into the runner, and tell the Compose scripts to use those images without rebuilding. The Dockerfile compiles dependencies before copying application source, so dependency layers remain reusable when Rust code changes.
