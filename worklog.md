# Implementation worklog

This file records implementation decisions and unforeseen constraints that were not fully specified in `vault-design.md`.

## 2026-08-03

- Use the current modular BDK crates (`bdk_wallet` 3.1 and `bdk_bitcoind_rpc` 0.22) rather than the deprecated monolithic `bdk` crate. The versions are pinned in `Cargo.lock` for reproducible builds.
- Use BIP86 Taproot external/change descriptors for the ordinary mobile hot wallet. The vault itself uses the custom static Taproot descriptor from the design and is tracked separately because its UTXOs require explicit script-path and batch-presigning behavior.
- Use the standard BIP341 NUMS point as the static Taproot internal key. This makes the descriptor deterministic and recoverable without extra state.
- Derive the static phone and HWW vault signing keys from a dedicated regtest-only path, separate from the phone hot-wallet account. Encryption keys are domain-separated from signing keys with HKDF-SHA256.
- Store protocol metadata as human-readable JSON and BDK hot-wallet chain state in SQLite. Device secrets and encrypted artifacts are separate files so loss/theft scenarios can remove or copy realistic subsets of state.
- The simulated HWW encrypts the phone mnemonic with a symmetric key derived from the HWW seed. A production HWW integration will need a device-defined authenticated encryption/decryption API; the MVP deliberately models the capability without claiming compatibility with an existing device.

