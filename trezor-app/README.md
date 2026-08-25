# Anzen Trezor app

This standalone app is the Trezor implementation of the Anzen cold signer. It
lives in the Anzen repository and depends only on the platform-independent
`anzen-cold-signer` crate plus Trezor's modular-app SDK from the pinned
`trezor-firmware` submodule. It does not depend on the Anzen CLI, hot wallet,
chain backends, or filesystem code.

The current app implements the deterministic annual-policy signing benchmark.
It constructs the same 12-input rollover, monthly allowance chain, emergency
access chain, 15 transactions, and 26 BIP341 signing jobs used by the Ledger
benchmark. Fake outpoints and amounts isolate the workload from real funds.
The device derives a reserved BIP340 key from its seed and creates real Schnorr
signatures, but the benchmark returns only non-secret commitments and timings.

## Build

Initialize the pinned firmware/SDK dependency from the repository root:

```sh
git submodule update --init --recursive trezor-firmware
```

Then enter Trezor's reproducible Nix environment and build the external app:

```sh
cd trezor-firmware
nix-shell
cd ../trezor-app
cargo xtask build --model t3w1 --lang en
```

The loadable app, Merkle proof, and development root packet are written to
`trezor-app/target/artifacts/t3w1/`.

## Run on a development Safe 7

The device must be running the app-capable development firmware pinned by this
repository. Never use that unsafe firmware or its seed for real funds.

With the device connected and unlocked, run from the repository root:

```sh
trezor-app/tools/run-benchmark.sh
```

Approve the annual policy on the device. Both the Trezor and the terminal show
the key-derivation, transaction-graph, 26-signature, and complete-workload
timings.

## Tests

Inside the Nix shell:

```sh
cd trezor-app
cargo xtask unit-tests --model t3w1 --lang en
cargo xtask fmt-check
```

For an end-to-end run, first build app-enabled T3W1 emulator firmware and the
emulator app artifact:

```sh
cd trezor-firmware
xtask build firmware --model T3W1 --apps --emulator --pyopt false --debug-link

cd ../trezor-app
cargo xtask build --model t3w1 --lang en --emulator
```

Then start the emulator from the repository root and let the debug link approve
the same policy screens a user sees:

```sh
trezor-firmware/core/emu.py \
  --headless \
  --temporary-profile \
  --executable trezor-firmware/core/build-xtask/release/unix \
  --command trezor-app/tools/emulator-test.py
```

This validates the complete app protocol and signing workload. Its timings are
host-CPU timings; use a physical development device for representative results.
