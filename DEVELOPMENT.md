# Development

This document describes the repository boundaries, dependency direction, and the workflow for Anzen's independently versioned Trezor firmware fork.

## Repository layout

```text
anzen/
├── crates/
│   ├── anzen-protocol/          `no_std` protocol rules: scripts, transaction graphs, sighashes, wire requests
│   ├── anzen-signer/            `no_std` cold signer: review and sign protocol requests, epoch state
│   ├── anzen-signer-app/        Host HWW role (CLI, mobile hardware mode, browser signer)
│   ├── anzen-wallet/            Phone role: hot wallet, policy execution, rotation and recovery
│   ├── anzen-chain/             Chain interface and the Bitcoin Core RPC backend
│   ├── anzen-common/            Shared host formats: keys, storage, backups, device messages
│   ├── anzen-storage/           Atomic private files, protected and browser storage
│   ├── anzen-cli/               `anzen` binary; composes the two roles for the simulation
│   ├── anzen-playground/        Loopback regtest host for the browser playground
│   └── anzen-integration-tests/ Cross-role, vector and real-node integration tests
├── ledger-app/                  Ledger application; depends on `anzen-signer`
├── mobile/                      React Native app, native (`mobile/rust`) and WASM (`mobile/wasm`) adapters
├── trezor-firmware/             Git submodule pointing to `lukechilds/trezor-firmware`
├── test-vectors/                Canonical serialized vault and transaction-graph fixtures
└── scripts/                     Docker, end-to-end, boundary and firmware setup helpers
```

The root repository is MIT licensed. `trezor-firmware/` is a separate GPLv3 repository and retains Trezor's upstream history and license. The submodule boundary prevents the firmware fork from being presented as part of the root MIT codebase.

## Dependency boundaries

The dependency graph is one-way. Each device role depends only on the protocol and host
support, never on the other role:

```text
anzen-cli ──────────┬──> anzen-wallet ──────┬──> anzen-chain ──┐
mobile/rust, wasm ──┤                       │                  ├──> anzen-common ──> anzen-storage
                    └──> anzen-signer-app ──┼──────────────────┘
                                            └──> anzen-signer ──> anzen-protocol
Ledger app ──────────────────────────────────────> anzen-signer ──> anzen-protocol
anzen-playground ──> anzen-wallet (public, watch-only sync)

Trezor firmware ──> independent Anzen protocol implementation
                └──> canonical protocol test vectors
```

- `anzen-protocol` is the single definition of what Anzen transactions are. It deterministically builds the vault and controller scripts, the annual policy graph, sweeps and revocations, and their BIP341 signature messages from typed parameters. It also owns BIP327 key aggregation (`musig`), because the vault's internal key is the MuSig2 aggregate of both device keys and every implementation must derive the same one. It is `no_std`, has no `bitcoin` or `miniscript` dependency, and takes hashing and curve operations (including the `mul_add` that key aggregation needs) through a `Crypto` trait so hardware can use device primitives.
- `anzen-signer` decides everything the hardware wallet signs. Requests carry parameters, never transactions: the signer rebuilds each transaction with `anzen-protocol`, returns a review, and signs only the reviewed request. For joint vault spends it returns its half of a MuSig2 signature through `Keys::sign_cooperative`, always as the last signer with BIP327 `DeterministicSign`, so it never stores a nonce. It persists an epoch anchor so every renewed policy must conflict with or consume the previously approved one. It is `no_std`, with no filesystem, network, clock or randomness of its own.
- `anzen-signer-app` wraps the signer for host platforms: seed storage, persisted signer state, pairing, cloud backup approval and restoration. It has no wallet or chain-backend dependency.
- `anzen-wallet` owns phone keys, the BDK wallet, encrypted policy artifacts, policy execution, phone restoration and rotation, and the coordination of the HWW's delayed recovery (its time-lock clock reads each coin's start time through the chain backend's `median_time_past`). It produces signer requests and checks signer approvals against the transactions it built from the same protocol code. Its `cosigning` module holds the phone's MuSig2 nonce sessions (`phone/cosigning/`): fresh nonces for every request, bound to the exact messages they may sign, and replaced by the finished signatures in one atomic write when the signer's partial signatures arrive. Never put this directory in a backup or sync it between devices.
- `anzen-chain` defines the chain interface, including the block median time past that time-based relative locks are judged by, and contains the Bitcoin Core RPC backend (regtest). Electrum over WebSocket, the transport for Mutinynet and mainnet on every platform, lives in `anzen-wallet` (`electrum`), together with the live-update listener (`watch`); each platform supplies only the socket. `anzen-common` holds shared host formats (device keys, configuration, the HWW-wrapped cloud backup, device messages) and the host MuSig2 code (`musig`, built on the pure-Rust `musig2` crate with `k256`) that the phone, the browser and the software signer share. `anzen-storage` provides atomic private file writes and protected storage.
- `ledger-app` is an independently built Ledger firmware crate and directly consumes `anzen-signer`. It is a proof of concept: it signs requests of at most 16 inputs, because it holds each request in memory whole (see its README for why that is left as it is). `anzen-link` is its host side: Bluetooth to a Ledger from Rust, and a client for the commands in `anzen_signer::hardware`. The shared crates' few accommodations for that device's memory are each marked "Hardware signer memory:".
- `trezor-firmware` cannot consume the Rust crate through Trezor's normal firmware architecture. It implements the same protocol independently and proves compatibility against the same deterministic vectors. Production code should live under `core/src/apps/anzen/`, with protocol messages in `common/protob/messages-anzen.proto`, Python host support in `python/src/trezorlib/anzen.py`, and device tests in `tests/device_tests/anzen/`.

`scripts/check-boundaries.sh` enforces these boundaries from the resolved dependency graph: it
builds `anzen-protocol` and `anzen-signer` for a bare-metal target, allowlists the signer's entire
dependency tree, and denies wallet, chain and network crates to the signer side. Hardware
implementations must never gain network access or depend on phone-wallet implementation code.

`test-vectors/policy-graphs.json` pins the protocol builder byte-for-byte across valid and rejected
policies, and `anzen-protocol`'s tests compare its scripts, addresses, transactions and sighashes
against `rust-bitcoin` and `miniscript`, check its key aggregation against the independent `musig2`
crate, and verify a complete two-party MuSig2 signature under the vault output key. `test-vectors/vault-output-graph.json` is generated by the
`vault_output_vectors` integration test and read back by `anzen-protocol`. After an intentional
policy change, regenerate both with `ANZEN_UPDATE_VECTORS=1`:

```bash
ANZEN_UPDATE_VECTORS=1 cargo test -p anzen-protocol --test policy_graphs
ANZEN_UPDATE_VECTORS=1 cargo test -p anzen-integration-tests --test vault_output_vectors
```

## Root Rust development

The React Native app and two-device web playground are documented in
[`mobile/README.md`](mobile/README.md). See the
[mobile architecture decision](docs/mobile-architecture.md) for Rust-owned business logic,
role-specific consumer APIs and native storage/transport boundaries. Native Rust is an
independently built workspace, so adding a mobile target does not change CLI Docker builds.

The normal Rust checks cover every workspace crate:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
./scripts/check-boundaries.sh
cargo test --workspace --all-targets --locked
cargo test -p anzen-wallet --all-features --lib --locked
```

The real-node tests are ignored by default and run in Docker by `./scripts/run-tests.sh`, or
directly against a regtest node with, for example,
`cargo test -p anzen-integration-tests --test regtest_ceremony -- --ignored --test-threads=1`.
The recovery test reaches the HWW's ~388-day delay by advancing Bitcoin Core's mock time: eleven
blocks stamped with one time move the median time past exactly to it. It checks that Core rejects
the HWW's sweep as non-BIP68-final one second early and accepts the same sweep on time.

The Ledger application uses a separate embedded target and workspace:

```bash
cd ledger-app
cargo ledger build flex
```

The Docker integration and end-to-end commands are documented in the main README. The Trezor submodule is excluded from the root Docker build context so local firmware checkouts do not invalidate or enlarge CLI images.

## Phone rotation recovery state

Phone rotation sweeps the old phone's spendable hot-wallet balance as well as rotating the vault.
Before any broadcast, it retains the old and replacement key material and encrypted cloud backups
under `history/rotation-<vault-sweep-txid>/`; the replacement state is in its `replacement/` directory.
The old wallet database and policy artifacts are archived there when the vault sweep succeeds.
Treat this directory as sensitive wallet data, not disposable policy output: it keeps interrupted
rotations, late deposits, and immature regtest coinbase outputs recoverable. Old-key recovery
material is intentionally not deleted automatically after broadcasting a sweep.

## Policy activation durability

Each policy is staged under `phone/transactions/<rollover-txid>/`, including its approved package,
signed rollover, encrypted future transactions, and candidate schedule. The active `phone/schedule.json`
is atomically replaced only after the chain backend accepts the rollover; failed broadcasts leave the
previous epoch intact. If broadcasting succeeds but a local write fails, retry activation using that
epoch's `approved-policy.json`, encrypted with the phone key and accepted directly by
`phone activate-policy`. Backends accept retries of an already-known transaction, while the
phone refuses to reactivate an epoch it has already superseded. State files use private temporary
files and a flushed atomic rename rather than truncating the active file in place.

Before staging or funding a policy, the phone verifies the signer's partial signature on every vault
input, completes and verifies each joint signature, and finalizes each vault input on a temporary copy
to check its satisfaction metadata. Completing records the finished signatures in the phone's signing
session, so a retry of the same approval reuses them without touching a nonce. Future
controller inputs remain unsigned. Phone rotation runs the same complete renewal check before
broadcasting its hot-wallet sweep, controller revocation, or vault sweep.

## Hot-wallet address recovery

The first Electrum sync of a new or restored database discovers both receive and change chains,
using a 100-unused-address stop gap to accommodate addresses reserved by policy proposals. This is
a bounded discovery scan, not a promise to find arbitrary gaps larger than 100 addresses. Previously
revealed addresses are always synced too, even when they lie beyond that gap. Successful discovery
is recorded in SQLite only after the wallet update is persisted; failed scans retry after reopening.
Phone restoration calls `HotWallet::request_full_scan` even if an old database survives.

## Hardware signing benchmark

The Ledger and Trezor benchmarks still model the earlier script-path signing workload, not the MuSig2 key path; updating them to key-path messages and partial signing is pending. They reconstruct the same deterministic annual policy graph as the reference implementation: a 2.1 BTC fixture, a twelve-input rollover, twelve sequential 0.1 BTC allowance authorizations, and one 0.5 BTC emergency trigger and withdrawal. The rollover creates separate monthly and emergency connectors. Every later action has one vault input and one unsigned connector input; the benchmark hashes the complete two-input transaction but signs only the vault input, matching annual HWW approval.

The resulting workload is 15 transactions and 26 HWW signatures: twelve rollover-input signatures plus one for each of fourteen future actions. Dynamic revocation and cancellation are deliberately excluded because either device constructs and signs those controller-only transactions only when requested. `anzen-signer`'s benchmark checks every BIP341 sighash against `rust-bitcoin` and pins the controller script, first and last sighashes, and aggregate digest. Trezor independently pins those same values, preventing the two firmware implementations from silently benchmarking different graphs.

Build Ledger Flex with the command above. From a configured Trezor Nix environment, run its focused compatibility test with:

```bash
make -C trezor-firmware/core test \
  TESTOPTS=test_apps.homescreen.anzen_benchmark.py
```

## The Trezor firmware submodule

`.gitmodules` records two pieces of routing information:

```ini
[submodule "trezor-firmware"]
    path = trezor-firmware
    url = git@github.com:lukechilds/trezor-firmware.git
    branch = anzen
```

The Anzen repository itself records an exact firmware commit, not a moving branch reference. The `branch = anzen` setting is used only by explicit remote-update commands. This means an existing Anzen commit always resolves to the same firmware source even after newer firmware commits are pushed.

### Initializing it

Normal CLI contributors do not need the firmware checkout. Firmware contributors can initialize the fork and all of Trezor's nested vendor submodules with:

```bash
./scripts/setup-trezor.sh
```

The same setup can be performed manually:

```bash
git submodule update --init trezor-firmware
git -C trezor-firmware submodule update --init --recursive
git -C trezor-firmware remote add upstream https://github.com/trezor/trezor-firmware.git
```

The final command is needed only once. The setup script checks before adding the remote.

After an ordinary clone or `git submodule update`, Git checks out the exact pinned firmware commit in detached-HEAD state. That is correct for building and testing. Switch to the fork's development branch before editing:

```bash
git -C trezor-firmware switch anzen
git -C trezor-firmware pull --ff-only origin anzen
```

If the remote branch has advanced beyond the commit pinned by Anzen, switching and pulling will intentionally make the parent repository report `trezor-firmware` as modified.

### Committing firmware changes

Firmware source and the parent pointer require separate commits. Always publish the firmware commit before publishing an Anzen commit that points to it:

```bash
git -C trezor-firmware status --short
git -C trezor-firmware add <firmware-paths>
git -C trezor-firmware commit -m "feat(core): describe the firmware change"
git -C trezor-firmware push origin anzen

git add trezor-firmware
git commit -m "Update Trezor firmware"
git push origin main
```

The parent repository only stores the submodule's commit ID. A root `git add trezor-firmware` cannot capture uncommitted firmware files, and a root push does not normally push the firmware branch. Keeping the two operations explicit avoids publishing a parent pointer whose commit is not available from the fork.

Inspect the current relationship with:

```bash
git submodule status trezor-firmware
git -C trezor-firmware status --short --branch
git diff --submodule=log
```

### Pulling a newer fork commit

To move Anzen to the current tip of the configured `anzen` firmware branch:

```bash
git submodule update --remote --merge trezor-firmware
git diff --submodule=log
git add trezor-firmware
git commit -m "Update Trezor firmware"
```

Do not run remote submodule updates in ordinary reproducible builds. CI and releases should build the exact commit already pinned by the parent repository.

### Updating from Trezor upstream

The fork uses these remotes:

```text
origin    git@github.com:lukechilds/trezor-firmware.git
upstream  https://github.com/trezor/trezor-firmware.git
```

Published firmware commits are security-review artifacts, so do not rewrite the `anzen` branch after Anzen has pinned it. Merge a reviewed upstream release or create a new branch from its signed release tag, run the firmware tests, push the result, and then update the parent pointer:

```bash
git -C trezor-firmware fetch upstream --tags
git -C trezor-firmware switch anzen
git -C trezor-firmware merge <reviewed-upstream-tag>
```

Keep Anzen-specific commits small and separate generated-file updates from handwritten protocol or UI changes where practical. Trezor commits generated bindings and QSTR files; regenerate them with Trezor's own commands rather than editing generated output manually.

## Protocol compatibility

The protocol specification and deterministic fixtures in the Anzen repository are canonical. Hardware-wallet implementations should independently reconstruct and validate the complete policy transaction graph before signing it. Compatibility tests should cover at least:

- policy serialization and versioning;
- MuSig2 key aggregation, Taproot scripts, control blocks, and output keys;
- every input, output, amount, fee, sequence, and timelock;
- vault/controller input roles, connector continuity, and dynamic revocation/cancellation spends;
- all BIP341 signature messages, and the signer's MuSig2 partial signatures; and
- rejection of altered or incomplete policy packages.

The firmware repositories may copy a versioned fixture when they need to test independently, but the copied fixture should record its originating Anzen commit and be checked against the canonical version during integration.
