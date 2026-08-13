# Anzen Ledger app

This crate is the Ledger implementation of the Anzen cold signer. It currently
provides a native Ledger home screen and a deterministic benchmark of the
complete annual vault-policy signing workload. The benchmark constructs real
Bitcoin transactions and BIP341 sighashes, derives a reserved key from the
Ledger seed, and creates real BIP340 signatures. All outpoints and amounts
belong to an isolated fake fixture; generated signatures are committed to a
transcript and discarded.

The firmware depends on the platform-independent `anzen-cold-signer` crate and
does not depend on the Anzen CLI, hot wallet, chain backends, or filesystem
code.

## Build

Ledger's development image contains the required nightly toolchain, device
targets, SDKs, and `cargo-ledger` command. From the repository root, build for
Nano S+ with:

```console
docker run --rm \
  -v "$PWD:/app" \
  -w /app/ledger-app \
  ghcr.io/ledgerhq/ledger-app-builder/ledger-app-dev-tools:latest \
  cargo ledger build nanosplus
```

Other supported targets are `nanox`, `stax`, `flex`, and `apex_p`.

## Interactive Flex viewer

Start the Flex firmware in headless Speculos from the repository root:

```console
docker run --rm -it \
  -p 5001:5001 \
  -p 9999:9999 \
  -v "$PWD:/app" \
  -w /app/ledger-app \
  ghcr.io/ledgerhq/ledger-app-builder/ledger-app-dev-tools:latest \
  speculos --model flex --display headless --api-port 5001 --apdu-port 9999 \
  target/flex/release/anzen-ledger
```

In another terminal, start the dependency-free local viewer and open it in the
default browser:

```console
python3 ledger-app/tools/flex-viewer.py --open
```

The viewer runs at `http://127.0.0.1:5002`. Taps and swipes are translated to
Flex screen coordinates and forwarded to Speculos. Its side control forwards
Speculos's right hardware-button event. Select **Run signing benchmark** on the
Anzen home screen to run the workload locally; use the home-screen quit control
to return to the Ledger dashboard.

For accurate timing, use a physical Flex and run the host benchmark while Anzen
is open:

```sh
python3 ledger-app/tools/signing-benchmark.py --inputs 12
```

The host measures only the signing APDU, reports the duration on the Flex
completion screen, and does not ask the device to verify its own signatures.
`--inputs` accepts `1`, `2`, or `12`; the rollover requires one signature per
input, while every other policy transaction has one input. Speculos can
exercise the same protocol for correctness, but its timing is not
representative of physical hardware:

```sh
python3 ledger-app/tools/signing-benchmark.py --speculos 127.0.0.1:9999 --inputs 12
```
