# Design decisions

This file records the product rationale and implementation trade-offs that are useful to contributors but are not required to follow the CLI examples in the README.

## Product model and threat model

Anzen is intended to avoid the usual self-custody trade-off between security, usability, and trustlessness. Singlesig hot and hardware wallets are simple and sovereign but make one key a catastrophic point of failure. Conventional multivendor multisig raises the compromise threshold without adding a provider, but also adds devices, backups, locations, and recovery procedures that users must manage correctly. Collaborative custody can hide that complexity, but gives a provider a signing role and makes its continued availability and behavior part of the security model.

Anzen instead combines two independent device keys with Bitcoin-enforced recovery paths. The comparison with 2-of-3 multisig is specifically about compromise resistance: an attacker must obtain two independent device keys to spend immediately. The constructions are not identical. Anzen replaces a third key and its operational burden with staggered, delayed recovery paths and an expected annual renewal.

The phone is both a normal hot wallet and one vault cosigner. The HWW is the independent cold signer. This supports a checking-and-savings mental model: the hot balance is available for ordinary use, while the vault policy controls how cold savings may move into it.

## Why recovery paths are staggered

Either device must eventually be able to recover from permanent loss of the other, but allowing both single-key paths at the same time would create a race with no honest-party advantage after one key is extracted. The phone path therefore matures first. If the HWW key is compromised, the legitimate phone gets approximately one month in which it can sweep every live vault output before the HWW-only path becomes valid.

The delays are intentionally longer than the expected annual renewal interval. A normal rollover refreshes every live output before either fallback matures, so delayed recovery remains dormant during ordinary operation. The exact delay selection, BIP68 constraints, and block-time uncertainty are specified in `anzen-design.md`.

## Why the HWW approves a policy

Showing dozens of unrelated PSBT prompts would make the transaction graph impossible for a person to audit and would train users to approve opaque data. The user-facing security primitive is therefore a high-level annual policy: vault balance, monthly allowance, emergency amount and delay, recovery keys, and renewal date.

Internally, the HWW still validates and signs transactions. It independently reconstructs the transaction graph implied by the proposal, verifies every input, output, amount, timelock, key, and conflict, and signs the complete batch only after one human approval. The distinction is about what the user meaningfully authorizes, not about hiding validation inside the phone.

## Why presigned transactions are used

Bitcoin does not currently provide the covenant primitives needed to express Anzen's complete annual spending policy directly in one reusable script. Presigned transactions commit to the permitted outputs and amounts using ordinary Bitcoin signatures and timelocks, so the construction works under current consensus rules without a server, soft fork, or new opcode.

Presigned policy artifacts are permissions, not essential custody state. Losing every artifact removes the convenient monthly and emergency actions but does not remove any vault-script recovery path. Keys plus the static descriptor remain sufficient to recover the underlying outputs.

## Why the vault address is static

Every ordinary vault output reuses the same keys, descriptor, and address until a key rotation. This deliberately trades address-level privacy for a durable receive address, simpler backup and recovery, and easier verification. Annual rollovers already link the vault's UTXOs, so rotating addresses without rotating keys would add operational complexity with limited privacy benefit. A key rotation creates a new descriptor and address.

## Why rotation and policy rollover are separate transactions

Phone-key rotation first sweeps every old-policy UTXO into one output under the new keys. If programmable policy is active, the ordinary rollover transaction then splits that output into monthly chunks and a remainder. This gives the two transactions clear responsibilities: the old keys authorize leaving the old policy, while the new keys authorize the renewed policy and its presigned children.

The separation also lets rotation use the same validation path whether monthly and emergency features are enabled or disabled, and lets renewed policy reuse the normal rollover machinery. It is not a Bitcoin requirement. A future format could make the cooperative rotation create the policy outputs directly, saving one transaction and one unconfirmed ancestor at the cost of coupling rotation validation to the policy layout.

## Trust and interoperability

Anzen's security-critical behavior must remain enforceable without an Anzen company, server, update channel, or online cosigner. Chain backends provide data and broadcast transactions but cannot change a valid transaction or script. Cloud storage holds only encrypted recovery material and individually encrypted policy artifacts.

Social recovery is an explicit optional trust expansion: each configured friend receives eventual phone-key recovery capability, but still cannot bypass the phone path's on-chain delay. It is not part of the default custody threshold.

The protocol is intentionally split between hot-wallet and cold-wallet roles rather than tied to the reference applications. Independent mobile, desktop, and hardware-wallet implementations should be able to exchange the same versioned policy and recovery objects and validate each other without vendor infrastructure.

## Library architecture

The Rust library has three public modules with a one-way dependency boundary:

```text
CLI / future apps
├── hot_wallet ──┐
├── cold_wallet ─┼──> core
└── core ────────┘
```

- `hot_wallet` owns phone keys, the BDK hot wallet, encrypted monthly/emergency policy transactions, phone recovery, and phone-key rotation. Future iOS and Android apps should build on this API.
- `cold_wallet` owns the deliberately small HWW surface: backup encryption/decryption, complete policy review and signing, cooperative-sweep approval, offline HWW recovery signing, and rotation approval. It imports only `core` and has no BDK wallet, Electrum, Bitcoin Core, or `hot_wallet` dependency.
- `core` contains shared serialized protocol objects, key derivation, Miniscript policy construction, PSBT construction and validation, authenticated encryption/OpenPGP recovery envelopes, storage formats, and chain backend interfaces. It has no dependency on either device implementation.

The Anzen CLI composes these APIs. `anzen phone *` dispatches only through `hot_wallet` and `core`; `anzen hww *` dispatches only through `cold_wallet` and `core`. Chain scanning and broadcasting for HWW recovery remain in the CLI, keeping the cold signer offline. Architecture tests enforce these boundaries.

## Chain backends

Bitcoin Core RPC and Electrum both implement the shared chain interface and can be used on regtest or mainnet. The CLI defaults to RPC on regtest and Electrum on mainnet for compatibility, while `--chain-backend` explicitly selects either implementation. RPC defaults to the conventional local ports; Electrum defaults to a local regtest server or a failover list of public mainnet TLS servers. `--rpc-url` and `--electrum-url` override those endpoints.

Both implementations verify the connected network before scanning or broadcasting. Public Electrum servers are an availability convenience, not a privacy or trust boundary; production users should prefer their own backend.

## Continuous integration

GitHub Actions runs formatting, Clippy, unit/CLI tests, the real Bitcoin Core integration suite, and every isolated end-to-end test on pushes to `main` and pull requests. A preparation job reads test names dynamically from `./scripts/run-e2e.sh --list`, then a matrix assigns every test to a separate GitHub-hosted worker so the long recovery-delay tests run concurrently. A final aggregate check passes only when preparation and every matrix worker pass.

The quality job restores Cargo registry and `target/` data with the GitHub Actions cache. Docker jobs build through Buildx with separate GHA-backed `test` and `runtime` cache scopes. The E2E preparation job builds the runtime image once and uploads it as a short-lived workflow artifact; every matrix worker loads that exact image and tells Compose not to rebuild it. The Dockerfile compiles dependencies before copying application source, keeping dependency layers reusable when Rust code changes.
