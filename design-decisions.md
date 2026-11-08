# Design decisions

This file records the product rationale and implementation trade-offs that are useful to contributors but are not required to follow the CLI examples in the README.

## Product model and threat model

Anzen is intended to avoid the usual self-custody trade-off between security, usability, and trustlessness. Singlesig hot and hardware wallets are simple and sovereign but make one key a catastrophic point of failure. Conventional multivendor multisig raises the compromise threshold without adding a provider, but also adds devices, backups, locations, and recovery procedures that users must manage correctly. Collaborative custody can hide that complexity, but gives a provider a signing role and makes its continued availability and behavior part of the security model.

Anzen instead combines two independent device keys with one Bitcoin-enforced fallback: the hardware wallet alone, after a time lock of about 388 days. In effect it is a hardware wallet with a second factor. The phone adds a required second signature, a bounded monthly allowance and an emergency withdrawal, but never a path of its own to the savings. The comparison with 2-of-3 multisig is specifically about compromise resistance: an attacker must obtain two independent device keys to spend immediately. The constructions are not identical. Anzen replaces a third key and its operational burden with that delayed hardware-wallet path and an expected annual renewal.

The phone is both a normal hot wallet and one vault cosigner. The HWW is the independent cold signer. This supports a checking-and-savings mental model: the hot balance is available for ordinary use, while the vault policy controls how cold savings may move into it.

## Why only the hardware wallet has a fallback

A vault needs a way out if a device is permanently lost, but the two devices are not alike. A hardware wallet's seed is already backed up through its maker's own recovery process, and restoring it yields the same key, so a lost signer is not a lost key: the user restores it and carries on, and the vault does not change. The phone key lives on an everyday device and in its cloud backup, both exposed to theft, malware and account compromise.

The fallback therefore runs one way. The vault has two spending paths: both devices together through the Taproot key path, or the hardware wallet alone, the tree's only leaf, after the longest relative time lock Bitcoin allows. The phone alone never reaches the savings, so a stolen or compromised phone is never a single point of theft. At most it gets the monthly allowance and the emergency withdrawal, both fixed by the approved policy, delayed, and revocable by the signer.

Giving each device its own eventual path would mean staggering the delays so the honest device wins the race after one key leaks, and it would let the weaker phone key, or anyone holding its cloud backup, eventually drain the vault. With one path there is no race to win and nothing to stagger.

The cost is the hardware wallet's own: losing the signer together with its backup, and the phone, leaves the savings locked for good. Losing the phone and its cloud backup leaves the signer able to recover alone once the time lock passes. A thief with the signer, and its PIN, must wait out the same time lock, and any renewal in that window moves every coin to fresh outputs and restarts the clock; rotating the phone key does the same.

## Why the time lock is time-based and about 388 days

BIP68 gives a relative lock 16 bits, so its longest time-based value is 65,535 units of 512 seconds: 33,553,920 seconds, about 388 days, written `older(4259839)` (the time-type flag plus 65,535). A time-based lock means the same on mainnet, Mutinynet's 30-second blocks and regtest, and does not drift with hashrate. It counts from the median time past of the block before the one that confirmed the coin, and a spend fits in the next block once the tip's median time past has passed it.

The delay is longer than the expected annual renewal interval. A normal renewal refreshes every live output about three weeks before the fallback opens, so it stays dormant during ordinary operation and matters only when renewal stops. The exact semantics are specified in `anzen-design.md`.

## Why the joint path is a MuSig2 key spend

The vault's internal key is the MuSig2 (BIP327) aggregate of the phone and hardware wallet keys, so a cooperative spend is one ordinary 64-byte Taproot key-path signature produced by both devices together. The alternative, a `multi_a(2,phone,hww)` leaf, needs two signatures, the 70-byte script and a control block on chain. The key path saves about 51 vbytes per cooperative vault input, which every rollover, allowance authorization, emergency transaction, sweep and rotation pays, and cooperative spends look like any single-key Taproot spend rather than revealing the vault's scripts. The time-locked hardware-wallet leaf stays in the tree, so the fallback is unchanged.

Security is the same 2-of-2: no secret for the aggregate key exists anywhere, and a joint signature needs both device keys. Key aggregation is part of the protocol, not a device detail, because every implementation must derive the same address; it is checked against an independent MuSig2 implementation in tests.

## Why the signer signs last and how the phone handles nonces

MuSig2 has two rounds: exchange nonces, then partial signatures. With two signers the hardware wallet can do both of its steps at once if it signs last: the phone's request carries its public nonces, and the signer answers with its own nonce and partial signature. Setup and approvals therefore stay a single exchange, including for QR-only devices. The signer uses BIP327 `DeterministicSign`, deriving its nonce from its key, the phone's nonce, the aggregate key and the message. It keeps no nonce state, which suits devices that are bad at keeping state safely: the same request yields the same answer, and any change yields a different nonce.

The cost is that the phone, signing first, must hold a secret nonce from its request until the signer answers, and a MuSig2 secret nonce must sign exactly once. Three signatures on one phone nonce with different signer nonces reveal the phone's vault key. The realistic attacker is someone holding the signer's key, the one party able to produce answers the phone accepts, so a nonce bug would let a signer thief drain the vault immediately instead of waiting out the time lock. The phone therefore treats pending nonces as one-time secrets:

- every request gets fresh random nonces, and a retry is a new request, never a resend;
- a signing session signs only the exact messages it was started for;
- the signer's partial signatures are verified before the phone signs anything;
- finishing replaces the secret nonces with the finished signatures in one atomic write before any signature leaves the phone, so a crash either releases nothing or leaves only finished signatures for retries to reuse;
- sessions live in device-only storage, never in the cloud backup or an exported message, so a restore cannot put a pending nonce on a second device.

The phone finishes the whole presigned annual graph during activation, so no nonce is held for months. "Finish later" retries reuse finished signatures, not nonces. The phone key alone still cannot reach the savings, so these rules protect against the signer-thief case specifically.

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

The current protocol reserves 1,000 sats in each live connector. The annual rollover consumes every old connector and every live vault UTXO, then creates exactly one new connector for each enabled independent chain: one for monthly allowances and one for emergency access. Intermediate monthly authorizations and the emergency trigger reproduce a 1,000-sat connector at the same address for the next action. The final monthly authorization and emergency withdrawal consume their connector without replacing it. At the MVP's fixed 1 sat/vB fee rate, a standalone 137-vbyte revocation returns 863 sats—comfortably above the 330-sat P2TR dust threshold. Production can add an ordinary wallet fee input when higher feerates would otherwise make the change dust.

Phone revocation constructs a controller-only transaction dynamically and sends the connector value left after fees to a fresh hot-wallet change address. HWW revocation likewise spends one or all live connectors, but sends the remainder to a destination that the hardware wallet explicitly displays and approves. Revocation never sends change back to the controller address: doing so would accidentally create new authorization state. Phone-key rotation consumes every old-key controller before installing the replacement key.

This construction reduces a full annual policy from 28 presigned PSBTs to 15: one rollover, twelve monthly authorizations, one emergency trigger, and one emergency withdrawal. Revocation and emergency cancellation are dynamic controller spends and require no presigned vault transaction. It also reduces the HWW workload for a twelve-input rollover from 39 vault signatures to 26, because controller inputs are signed only at execution or revocation.

The connector reserve and fixed 1 sat/vB fee are MVP parameters, not production fee policy. A production implementation needs current-feerate selection plus explicit fee inputs and RBF/CPFP handling for both execution and revocation. Until a revocation confirms, it can still race a now-valid action spending the same connector; the wallet should revoke early and fee competitively.

## Why the vault address is static

Every ordinary vault output reuses the same keys, descriptor, and address until a key rotation. This deliberately trades address-level privacy for a durable receive address, simpler backup and recovery, and easier verification. Annual rollovers already link the vault's UTXOs, so rotating addresses without rotating keys would add operational complexity with limited privacy benefit. Because cooperative spends use the MuSig2 key path, they do not reveal the vault's scripts or mark the address as an Anzen vault. A key rotation creates a new descriptor and address.

## Why rotation uses separate revocation, sweep, and rollover transactions

Phone-key rotation first uses a dynamic controller-only transaction to consume all old-key connectors and return their remainder to replacement-phone change. It then sweeps every old-policy vault UTXO into one output under the new keys. If programmable policy is active, the ordinary rollover transaction splits that output into an allowance-chain output, a remainder, and fresh connectors under the replacement controller policy. This gives the transactions clear responsibilities: the first transaction revokes old policy authority without moving principal, the second authorizes leaving the old vault, and the third lets the new keys authorize the renewed policy and its presigned children.

The separation also lets rotation use the same validation path whether monthly and emergency features are enabled or disabled, and lets renewed policy reuse the normal rollover machinery. It is not a Bitcoin requirement. A future format could combine connector revocation with the cooperative sweep and make that sweep create the policy outputs directly, saving transactions and unconfirmed ancestors at the cost of coupling rotation validation to the policy layout. Because the MVP broadcasts these state transitions separately, production rotation must persist and rebroadcast each accepted transaction crash-safely before installing replacement state.

## Why vault setup is a single exchange

Setup needs three things from the signer: its public key, its acceptance of the vault built from both keys, and its wrap of the phone's recovery-backup key. Once the phone has the signer's key, the other two are one request and one response, so setup is a single exchange over the Bluetooth link, with Bitkey's simplicity as the model. The signer asks nothing during setup: no funds can move before a policy exists, the backup request is sealed by the phone key of the vault being accepted, and the authenticated Bluetooth pairing (a code confirmed on both screens the first time) establishes which signer the phone is talking to. This replaces comparing vault addresses across a file transfer, which protected a channel the user could not see.

Everything that signs remains a separate, explicit review on the signer. The spending policy is chosen after the first deposit because its presigned transactions spend real vault coins, and the signer shows it as a plain monthly amount and an emergency amount before approval. Messages are the same versioned objects over any link; a file exchange remains the fallback where no Bluetooth link exists.

## Trust and interoperability

Anzen's security-critical behavior must remain enforceable without an Anzen company, server, update channel, or online cosigner. Chain backends provide data and broadcast transactions but cannot change a valid transaction or script. Cloud storage holds only encrypted recovery material and individually encrypted policy artifacts.

The phone's cloud backup is wrapped to the hardware wallet alone, so a compromised cloud account reveals nothing without the signer, and even a decrypted phone key cannot reach the savings by itself. There are no recovery contacts or other optional trust expansions. Recovering a lost signer is its maker's existing seed backup, which Anzen deliberately does not duplicate.

The protocol is intentionally split between hot-wallet and cold-wallet roles rather than tied to the reference applications. Independent mobile, desktop, and hardware-wallet implementations should be able to exchange the same versioned policy and recovery objects and validate each other without vendor infrastructure.

## Library architecture

The Rust workspace has two consumer-facing device crates over a small protocol core:

```text
Wallet UI  ──> anzen-wallet ─────┬──> anzen-chain, anzen-common, anzen-storage
Signer UI  ──> anzen-signer-app ─┤
                                 └──> anzen-signer ──> anzen-protocol
Ledger app ──> anzen-signer
```

- `anzen-protocol` is the one deterministic definition of the vault scripts and MuSig2 key aggregation, the annual policy graph, sweeps, revocations and their signature messages. It is `no_std` and builds every transaction from typed parameters, so the phone and every signer derive byte-identical transactions from the same request.
- `anzen-signer` is the auditable hardware-wallet surface: it rebuilds each request with `anzen-protocol`, reports what it does, signs only what was reviewed (as the last, stateless MuSig2 signer for joint signatures), and keeps an epoch anchor that forces every renewal to conflict with or consume the previously approved policy. It is `no_std` and has no BDK wallet, Electrum, Bitcoin Core, filesystem or clock dependency.
- `anzen-wallet` owns phone keys, the BDK hot wallet, the phone's single-use MuSig2 signing sessions, encrypted monthly/emergency policy transactions, restoring a lost phone from its cloud backup, and phone-key rotation. Future iOS and Android apps build on this API.
- `anzen-signer-app` hosts `anzen-signer` for the CLI, the mobile hardware-wallet mode and the browser signer: seed storage, persisted signer state, backups and restoration. It has no wallet or chain dependency.
- `anzen-common` and `anzen-storage` hold shared host formats (keys, configuration, encrypted backups, device messages) and atomic private storage. They exist to avoid duplication, not as APIs that wallet UIs must understand; each device crate re-exports the shared types its consumers need.

React Native is a presentation layer over these APIs, not a second implementation of the wallet. Policy validation, transaction construction, signing, balances, action eligibility, recovery, and durable workflow state belong to Rust. The same boundary applies to the native app and the React Native Web playground. See [Mobile application architecture](docs/mobile-architecture.md).

The CLI composes only the two device crates. Chain scanning and broadcasting for HWW recovery stay outside the cold signer. `scripts/check-boundaries.sh` enforces the crate boundaries from the resolved dependency graph.

## Chain backends

Bitcoin Core RPC and Electrum both implement the shared chain interface. Electrum is always Electrum over WebSockets (`wss://`), through one shared Rust client in `anzen_wallet::electrum` used by the CLI, the native app and the browser; a platform supplies only the socket. Mutinynet uses two public servers with failover, and mainnet uses `wss://electrum.lu.ke`. Regtest uses Bitcoin Core RPC. The CLI defaults to RPC on regtest and Electrum on mainnet, while `--chain-backend` explicitly selects either implementation. RPC defaults to the conventional local ports; `--rpc-url` and `--electrum-url` (a `wss://` URL) override those endpoints.

Both implementations verify the connected network before scanning or broadcasting. Public Electrum servers are an availability convenience, not a privacy or trust boundary; production users should prefer their own backend.

## Continuous integration

GitHub Actions runs formatting, Clippy, unit/CLI tests, the real Bitcoin Core integration suite, and every isolated end-to-end test on pushes to `main` and pull requests. A preparation job reads test names dynamically from `./scripts/run-e2e.sh --list`, then a matrix assigns every test to a separate GitHub-hosted worker so the long end-to-end tests run concurrently. A final aggregate check passes only when preparation and every matrix worker pass.

The quality job restores Cargo registry and `target/` data with the GitHub Actions cache. Docker jobs build through Buildx with separate GHA-backed `test` and `runtime` cache scopes. The E2E preparation job builds the runtime image once and uploads it as a short-lived workflow artifact; every matrix worker loads that exact image and tells Compose not to rebuild it. The Dockerfile compiles dependencies before copying application source, keeping dependency layers reusable when Rust code changes.
