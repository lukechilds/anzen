# Anzen Ledger app

This crate is the Ledger implementation of the Anzen cold signer. The first
milestone only displays an interactive `Hello Anzen` / `Hello World` screen; it does
not derive keys, parse policies, or sign transactions.

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
Speculos's right hardware-button event.
