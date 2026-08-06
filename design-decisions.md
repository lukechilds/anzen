# Design decisions

This file records implementation details that are useful to contributors but are not required to follow the CLI examples in the README.

## Library architecture

The Rust library has three public modules with a one-way dependency boundary:

```text
CLI / future apps
├── hot_wallet ──┐
├── cold_wallet ─┼──> core
└── core ────────┘
```

- `hot_wallet` owns phone keys, the BDK hot wallet, encrypted monthly transactions, phone recovery, and phone-key rotation. Future iOS and Android apps should build on this API.
- `cold_wallet` owns the deliberately small HWW surface: backup encryption/decryption, complete policy review and signing, cooperative-sweep approval, offline HWW recovery signing, and rotation approval. It imports only `core` and has no BDK wallet, Electrum, Bitcoin Core, or `hot_wallet` dependency.
- `core` contains shared serialized protocol objects, key derivation, Miniscript policy construction, PSBT construction and validation, authenticated encryption/OpenPGP recovery envelopes, storage formats, and chain backend interfaces. It has no dependency on either device implementation.

The Anzen CLI composes these APIs. `anzen phone *` dispatches only through `hot_wallet` and `core`; `anzen hww *` dispatches only through `cold_wallet` and `core`. Chain scanning and broadcasting for HWW recovery remain in the CLI, keeping the cold signer offline. Architecture tests enforce these boundaries.

## Chain backends

Bitcoin Core RPC and Electrum both implement the shared chain interface and can be used on regtest or mainnet. The CLI defaults to RPC on regtest and Electrum on mainnet for compatibility, while `--chain-backend` explicitly selects either implementation. RPC defaults to the conventional local ports; Electrum defaults to a local regtest server or a failover list of public mainnet TLS servers. `--rpc-url` and `--electrum-url` override those endpoints.

Both implementations verify the connected network before scanning or broadcasting. Public Electrum servers are an availability convenience, not a privacy or trust boundary; production users should prefer their own backend.

## Continuous integration

GitHub Actions runs formatting, Clippy, unit/CLI tests, the real Bitcoin Core integration suite, and every isolated end-to-end test on pushes to `main` and pull requests. A preparation job reads test names dynamically from `./scripts/run-e2e.sh --list`, then a matrix assigns every test to a separate GitHub-hosted worker so the long recovery-delay tests run concurrently. A final aggregate check passes only when preparation and every matrix worker pass. Superseded runs on the same branch are cancelled.

The quality job restores Cargo registry and `target/` data with the GitHub Actions cache. Docker jobs build through Buildx with separate GHA-backed `test` and `runtime` cache scopes. The E2E preparation job builds the runtime image once and uploads it as a short-lived workflow artifact; every matrix worker loads that exact image and tells Compose not to rebuild it. The Dockerfile compiles dependencies before copying application source, keeping dependency layers reusable when Rust code changes.
