# Development

This document describes the repository boundaries, dependency direction, and the workflow for Anzen's independently versioned Trezor firmware fork.

## Repository layout

```text
anzen/
├── src/
│   ├── core/             Shared protocol, policy, transaction, and storage logic
│   ├── hot_wallet/       Phone-side keys, wallet state, policy execution, and recovery
│   ├── cold_wallet/      Reference HWW role used by the CLI
│   └── main.rs           CLI orchestration and chain interaction
├── cold-signer/          Small `no_std` crate for embedded cold-signing logic
├── ledger-app/           Ledger application; depends on `cold-signer`
├── trezor-firmware/      Git submodule pointing to `lukechilds/trezor-firmware`
├── test-vectors/         Canonical serialized vault and transaction-graph fixtures
├── tests/                Rust architecture, CLI, and real-node integration tests
└── scripts/              Docker, end-to-end, and firmware setup helpers
```

The root repository is MIT licensed. `trezor-firmware/` is a separate GPLv3 repository and retains Trezor's upstream history and license. The submodule boundary prevents the firmware fork from being presented as part of the root MIT codebase.

## Dependency boundaries

The desktop/reference implementation has a deliberately one-way dependency graph:

```text
                         ┌── hot_wallet ──┐
Anzen CLI / future apps ─┤                ├──> core
                         └── cold_wallet ─┘

Ledger app ──> cold-signer

Trezor firmware ──> independent Anzen protocol implementation
                └──> canonical protocol test vectors
```

- `core` contains shared serialized objects, key derivation, Miniscript policy construction, PSBT construction and validation, encryption formats, storage types, and chain interfaces. It does not depend on either device implementation.
- `hot_wallet` owns phone keys, the BDK wallet, encrypted policy artifacts, phone-side execution, and phone recovery and rotation operations.
- `cold_wallet` is the reference implementation of the hardware-wallet role used by the CLI. It has no hot-wallet or chain-backend dependency.
- `cold-signer` is the small, platform-independent `no_std` surface intended for embedded hardware-wallet applications. It currently contains the deterministic signing benchmark; protocol parsing and validation should move here as those interfaces stabilize.
- `ledger-app` is an independently built Ledger firmware crate and directly consumes `cold-signer`.
- `trezor-firmware` cannot consume the Rust crate through Trezor's normal firmware architecture. It implements the same protocol independently and proves compatibility against the same deterministic vectors. Production code should live under `core/src/apps/anzen/`, with protocol messages in `common/protob/messages-anzen.proto`, Python host support in `python/src/trezorlib/anzen.py`, and device tests in `tests/device_tests/anzen/`.

The tests in `tests/architecture.rs` enforce the root Rust dependency boundaries. Hardware implementations must never gain network access or depend on phone-wallet implementation code.

## Root Rust development

The normal Rust checks cover the root package and `cold-signer` workspace member:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --all-targets --locked
```

The Ledger application uses a separate embedded target and workspace:

```bash
cd ledger-app
cargo ledger build flex
```

The Docker integration and end-to-end commands are documented in the main README. The Trezor submodule is excluded from the root Docker build context so local firmware checkouts do not invalidate or enlarge CLI images.

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
- Taproot scripts, control blocks, and output keys;
- every input, output, amount, fee, sequence, and timelock;
- conflicting authorization/revocation and emergency-withdrawal/cancellation transactions;
- all BIP341 signature messages; and
- rejection of altered or incomplete policy packages.

The firmware repositories may copy a versioned fixture when they need to test independently, but the copied fixture should record its originating Anzen commit and be checked against the canonical version during integration.
