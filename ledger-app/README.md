# Anzen Ledger app

This crate is the Ledger implementation of the Anzen cold signer. The first
milestone only displays an interactive `Anzen` / `Hello world` screen; it does
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
