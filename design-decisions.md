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

## Why monthly allowances form one sequential chain

The annual rollover creates one allowance-chain output rather than twelve independently spendable calendar outputs. Each authorization waits for a time-based BIP68 delay of at least 30 days, releases the fixed limit, and creates the next smaller chain output. The next delay cannot begin until that authorization confirms, so unused allowances do not accumulate and the phone cannot release several missed months at once.

The chain is deliberately all-or-nothing from its current hop onward. Revoking its one live authorization also makes every descendant impossible because those descendants depend on transaction outputs that the current authorization would have created. Individual future allowances cannot be revoked selectively, but one device action can cancel all remaining mobile access for the epoch.

Relative delays also avoid embedding calendar dates and absolute timestamps into the annual policy. “Monthly” means a minimum 30-day cadence, encoded as 5,063 BIP68 time units (2,592,256 seconds), rather than the first day of each calendar month. This makes the security property relative to actual on-chain confirmation while keeping the transaction graph independent of wall-clock policy creation time.

## Why revocation uses connector outputs

If an authorization and its revocation were both fully presigned against the vault UTXO, revoking would require moving the entire remaining allowance or emergency balance on-chain. Anzen instead gives every independently revocable policy chain a small **connector UTXO**. A policy action is a two-input transaction:

```text
presigned 2-of-2 vault input + live 1-of-2 controller input
```

The annual ceremony signs only the vault input. The controller input deliberately remains unsigned and prevents the PSBT from being finalized. When the user executes the action, the phone signs that one input and broadcasts the completed transaction. A revocation spends only the same connector, so the vault principal remains in its existing vault output while the presigned action becomes permanently invalid.

The controller policy is fixed for one key epoch and reuses the same vault public keys:

```text
tr(NUMS,{pk(phone),pk(hww)})
```

Either device can therefore revoke independently. Reusing the two fixed device keys avoids controller xpubs, controller derivation state, and connector-index keys. A connector's identity is its outpoint, not a derived key or address: every connector in the epoch uses the same controller address, and every presigned action commits to one exact outpoint with Taproot `SIGHASH_DEFAULT`.

The current protocol reserves 10,000 sats in each live connector. The annual rollover consumes every old connector and every live vault UTXO, then creates exactly one new connector for each enabled independent chain: one for monthly allowances and one for emergency access. Intermediate monthly authorizations and the emergency trigger reproduce a 10,000-sat connector at the same address for the next action. The final monthly authorization and emergency withdrawal consume their connector without replacing it.

Phone revocation constructs a controller-only transaction dynamically and sends the connector value left after fees to a fresh hot-wallet change address. HWW revocation likewise spends one or all live connectors, but sends the remainder to a destination that the hardware wallet explicitly displays and approves. Revocation never sends change back to the controller address: doing so would accidentally create new authorization state. Phone-key rotation consumes every old-key controller before installing the replacement key.

This construction reduces a full annual policy from 28 presigned PSBTs to 15: one rollover, twelve monthly authorizations, one emergency trigger, and one emergency withdrawal. Revocation and emergency cancellation are dynamic controller spends and require no presigned vault transaction. It also reduces the HWW workload for a twelve-input rollover from 39 vault signatures to 26, because controller inputs are signed only at execution or revocation.

The connector reserve and fixed 1 sat/vB fee are MVP parameters, not production fee policy. A production implementation needs current-feerate selection plus explicit fee inputs and RBF/CPFP handling for both execution and revocation. Until a revocation confirms, it can still race a now-valid action spending the same connector; the wallet should revoke early and fee competitively.

## Why the vault address is static

Every ordinary vault output reuses the same keys, descriptor, and address until a key rotation. This deliberately trades address-level privacy for a durable receive address, simpler backup and recovery, and easier verification. Annual rollovers already link the vault's UTXOs, so rotating addresses without rotating keys would add operational complexity with limited privacy benefit. A key rotation creates a new descriptor and address.

## Why rotation uses separate revocation, sweep, and rollover transactions

Phone-key rotation first uses a dynamic controller-only transaction to consume all old-key connectors and return their remainder to replacement-phone change. It then sweeps every old-policy vault UTXO into one output under the new keys. If programmable policy is active, the ordinary rollover transaction splits that output into an allowance-chain output, a remainder, and fresh connectors under the replacement controller policy. This gives the transactions clear responsibilities: the first transaction revokes old policy authority without moving principal, the second authorizes leaving the old vault, and the third lets the new keys authorize the renewed policy and its presigned children.

The separation also lets rotation use the same validation path whether monthly and emergency features are enabled or disabled, and lets renewed policy reuse the normal rollover machinery. It is not a Bitcoin requirement. A future format could combine connector revocation with the cooperative sweep and make that sweep create the policy outputs directly, saving transactions and unconfirmed ancestors at the cost of coupling rotation validation to the policy layout. Because the MVP broadcasts these state transitions separately, production rotation must persist and rebroadcast each accepted transaction crash-safely before installing replacement state.

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
