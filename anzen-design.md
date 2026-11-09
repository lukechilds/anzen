# Anzen: Renewable Bitcoin Vault with Bounded Mobile Spending

## Overview

Anzen combines:

- a **mobile key** `M`;
- a **hardware-wallet key** `H`;
- renewable **2-of-2 cold storage**;
- a delayed fallback for the hardware wallet alone, and none for the phone;
- a sequential, relatively timelocked monthly allowance chain;
- one cancellable, one-week-delayed emergency withdrawal per vault epoch.

The main balance normally requires both devices. The user performs an expected rollover roughly once per year, moving all funds into fresh vault outputs and restarting the hardware wallet's relative time lock.

The phone can also access a policy-defined monthly spending limit without carrying the HWW. A lower soft limit controls how much of an unlocked allowance the phone keeps hot. The same annual policy may include one larger emergency withdrawal that the phone can initiate immediately, cancel during a one-week safety window, and complete only after that delay. A new vault starts with both features disabled; their limits are introduced later through an explicit phone proposal and HWW approval. No server, custodian, or online co-signer is required.

## Designed to be hard to misuse

Anzen's operating model should remain simple even though the transaction graph underneath it is not. The wallet must guide the user through complete, high-level operations—create a policy, approve it, withdraw, revoke, renew, rotate, or recover—rather than expose raw transaction machinery and leave the user to assemble a safe sequence themselves. At every stage it should explain what happened, what protection remains active, and what action is required next.

The HWW approves a human-readable vault policy, not a stream of unrelated transaction prompts. It independently verifies the complete transaction graph implied by that policy and signs the batch in one ceremony. Once approved, the phone can safely execute or revoke the permitted behavior for the next year without asking the user to reconstruct the policy or repeatedly access cold storage.

Annual renewal should be one guided operation on the same calendar date each year. The wallet tracks the oldest live vault output, warns well before the signer's fallback opens, and guides the user through refreshing every live output and renewing the policy. Recovery and key rotation should likewise be complete workflows with safe defaults, explicit checks, and a clear destination—not collections of low-level tools that require the user to invent a recovery procedure during an emergency.

This principle applies throughout the product: monthly spending begins disabled, the HWW validates proposals independently, partial funding degrades predictably with a warning, and dangerous or incomplete state transitions are rejected. The goal is not merely to document the safe path, but to make the safe path the normal and easiest way to use Anzen.

## Vault script

Each vault output is Taproot whose **internal key is the MuSig2 (BIP327) aggregate of `M` and `H`**. Both devices spend together through the key path with one joint signature; the hardware wallet's fallback is the tree's only leaf.

Conceptually:

```text
tr(
  musig(M, H),
  and_v(v:after(T), and_v(v:older(4259839), pk(H)))
)
```

This means:

```text
M + H       can spend immediately, with one joint key-path signature
H alone     can spend after 65,535 × 512 seconds (33,553,920 s, about 388 days)
M alone     can never spend
```

Each x-only key enters aggregation as its even-y point, and the two are sorted (BIP327 `KeySort`) so both devices derive the same key. The descriptor records the aggregate as a literal x-only key, `tr(<aggregate>,and_v(v:after(T),and_v(v:older(4259839),pk(H))))`. No secret for the aggregate exists anywhere; a joint signature still needs both device keys. A cooperative spend's witness is a single 64-byte signature, about 51 vbytes per input less than a two-signature `multi_a` leaf, and looks like any single-key Taproot spend. `4259839` is the time-based BIP68 encoding: the type flag `1 << 22` plus 65,535 units of 512 seconds. The fallback leaf is the whole tree, so its control block carries no sibling hash.

`T` is an absolute locktime in Unix time, from 500,000,000 (1985) up to 1,700,000,000 (2023). Every allowed value has long passed, so `after(T)` never delays recovery: the recovery transaction sets its lock time to `T` and its inputs already carry a non-final sequence for the relative delay. `T` exists to choose the vault address without changing a key. A vanity vault grinds it on the GPU until the address starts with `vault`, which only changes the leaf hash and so the Taproot tweak; every other vault, including one rotated without `--vanity`, uses 500,000,000. The leaf is always 47 bytes, 7 more than without `after(T)`, which adds about 2 vbytes per recovery input and nothing to joint spends. The signer pairs with the phone's chosen `T`, and rotations carry the new vault's `T` explicitly.

The phone deliberately has no leaf of its own. The hardware wallet's seed is backed up by its maker and restores to the same key, so a lost signer does not need a path; the phone key, held on an everyday device and in a cloud backup, should never be able to drain savings. The phone's access is limited to the presigned monthly and emergency actions below.

### Joint signatures

MuSig2 takes two rounds, nonces then partial signatures. The phone signs first and the signer last: each request carries the phone's public nonce for every joint signature, and the signer returns its own nonce with its partial signature, so an approval stays one exchange. The signer derives its nonce with BIP327 `DeterministicSign` from its key, the phone's nonce, the aggregate key and the message, and keeps no state. The phone generates fresh random nonces per request and keeps each secret nonce in a single-use, device-only session until the signer answers. It verifies the signer's partial signatures before signing, and replaces the secret nonces with the finished signatures in one atomic write before any signature leaves the device. Reusing a phone nonce across two sessions would reveal the phone's key to whoever holds the signer's, so a retry is always a new request with new nonces. The whole presigned annual graph is finished during activation; no nonce outlives the exchange that created it.

The normal receive policy deliberately reuses the same `M` and `H` keys and the same Taproot address for every vault output, including rollover outputs and cold change. This sacrifices address-level privacy in favor of a simple, durable single vault address. An emergency rotation after a key compromise necessarily creates a new policy and address.

## Expected rollover schedule

The wallet should prompt the user to roll over all vault funds approximately once per year.

With the delay above, a 12-month expected rollover gives roughly **three weeks of grace** (about 23 days) before the HWW-only path opens.

The rollover should consume every current vault UTXO plus every live policy connector. When monthly spending is enabled, it creates one allowance-chain output funding up to twelve sequential releases, one remainder at the same vault address, and one monthly connector. Emergency access adds a second connector. With both features disabled or unfunded, rollover creates only the vault remainder. Every new vault output begins its time lock when it confirms; replacing the connectors invalidates all unused actions from the previous epoch.

The expected calendar date is a UX reminder. The actual deadline is per UTXO: its lock counts from the median time past of the block before the one that confirmed it, so the wallet tracks the oldest live vault output and shows the date the signer's fallback opens.

## Why this delay value is used

Bitcoin relative timelocks are encoded through BIP68 `nSequence` semantics and enforced in script with `CHECKSEQUENCEVERIFY` / Miniscript `older()`.

Only 16 bits are available for the relative delay value, so the maximum value is `65,535`. In time-based mode each unit is 512 seconds:

```text
65,535 × 512 s = 33,553,920 s ≈ 388.4 days
```

This is the longest relative delay a single output can express, and it comfortably exceeds an annual renewal. This is a **BIP68 limitation**. It is not a limitation on the number of timelock branches in one Taproot output.

### Time, not blocks

The fallback is time-based rather than block-based. Bitcoin evaluates it against median time past: a coin's lock starts at the median time past of the block before the one that confirmed it, and a spend is valid in the next block once the tip's median time past has reached that start plus the delay. This makes the delay the same on mainnet, on Mutinynet's 30-second blocks and on regtest, and immune to hashrate growth and block-time variance. Median time past trails wall-clock time by about an hour, which is negligible against a year.

### Possible soft-fork improvement

A future soft fork could extend relative-timelock range by assigning meaning to currently reserved `nSequence` bits or introducing a new relative-time primitive with a larger range. That would allow multi-year fallbacks and correspondingly longer renewal intervals. Until then, 65,535 time units is the longest stateless single-output delay.

## Revocable policy connectors

The vault script remains the only script that controls principal. Revocable policy state is represented by small 1,000-sat connector UTXOs under this fixed Taproot policy:

```text
tr(NUMS,{pk(M),pk(H)})
```

The controller reuses the fixed vault public keys. Either device can spend it alone, there is no usable key path, and no controller xpub, derivation index, or additional seed is required. All connectors in one key epoch use the same address; the particular state is identified by its outpoint.

Every presigned policy action contains two inputs:

1. a vault input carrying the joint key-path signature of `M` and `H`, completed during annual approval; and
2. one exact connector input that remains unsigned until execution.

The signature uses Taproot `SIGHASH_DEFAULT`, so the signed vault input commits to every input, previous output, sequence, and output in the transaction—including the exact controller outpoint and the next controller output. The PSBT cannot finalize until a device signs its connector input. The phone adds that signature only when executing the action.

Revocation is constructed dynamically and spends only the current connector. It leaves principal untouched in the existing vault output and makes the presigned action invalid because the two transactions conflict on the connector outpoint. Phone revocation sends the connector value left after fees to a fresh hot-wallet change address. HWW revocation sends it to an explicitly displayed and approved destination. A revocation must never create another controller output.

Annual rollover consumes every live old connector and creates exactly one replacement for each enabled independent chain. An intermediate action rolls its connector to the same controller address; a terminal action consumes it without replacement. Phone-key rotation consumes all old-controller outputs before installing the replacement key, then a renewed policy creates connectors under the new keys.

The MVP fixes connector value at 1,000 sats and fees at 1 sat/vB. A standalone 137-vbyte revocation therefore leaves 863 sats of non-dust P2TR change. Production requires a current-feerate strategy, optional fee inputs, and RBF/CPFP support. Until revocation confirms, it can race a mature policy action that spends the same connector.

## Monthly mobile spending

Vault initialization creates only the static cold-storage policy. Its monthly limit is zero, so there are no monthly authorization PSBTs or monthly connector. The phone later proposes a limit, including zero to disable spending, and the HWW confirms it through the signing protocol described below.

When a positive monthly policy is activated, rollover creates one allowance-chain UTXO, one remainder UTXO, and one connector. The chain contains enough value for up to twelve releases and their fees. At any moment only one allowance-chain output and one matching connector are live on-chain.

Each non-final authorization spends both live outputs after the vault input's relative delay, sends exactly the approved limit to a fresh hot-wallet address, and creates the next smaller vault output plus a replacement connector. The final authorization sends the last limit to the hot wallet and consumes both chains. For `N` steps and connector value `C`, the initial allowance-chain value is:

```text
N × monthly limit
+ all N authorization fees
- C (the final connector contributes its value to the last transaction)
```

The remainder receives every satoshi not needed for allowances, connector outputs, or rollover fees. If all twelve steps do not fit alongside a non-dust remainder and any configured emergency reserve, the wallet warns the user and chooses the largest fundable count. If zero steps fit, rollover and any fundable emergency package still proceed. A zero monthly limit creates no allowance chain or monthly connector.

Conceptually:

```text
Annual rollover
    ├─ allowance vault output, step 1 ─┐
    ├─ monthly connector, state 1 ─────┴─ authorization after ~30 days
    │                                      ├─ monthly limit → hot wallet
    │                                      ├─ smaller vault output → step 2
    │                                      └─ connector state 2
    └─ vault remainder

Phone or HWW revocation:
    connector state N → ordinary wallet change
    vault output N     → remains untouched
```

Each authorization is version 2 with zero `nLockTime`; its vault input uses a time-based BIP68 `nSequence` of at least 30 days, while its controller input is replaceable and has no delay. The HWW validates the complete chain and signs only the vault inputs during the annual ceremony. The phone encrypts each incomplete PSBT individually and signs the active controller input when the user executes it.

BIP68 encodes time in 512-second units. The smallest representable delay of at least 30 days is 5,063 units, or **2,592,256 seconds**—30 days plus 4 minutes 16 seconds. Bitcoin evaluates this against median-time-past. Step 1 starts when rollover confirms; every later delay starts only when the preceding authorization confirms. Waiting without executing a step does not mature descendants.

Revoking step `i` spends its controller and makes the current authorization impossible. Every later authorization also becomes impossible because its vault and connector outpoints would have been created by the current transaction. Revocation is therefore whole-chain. A full policy stores at most twelve monthly authorization PSBTs, not twelve authorization/revocation pairs.

Unused allowance value remains protected by the vault script. A later annual rollover consumes the live vault outputs and connector, invalidating retained copies of every old policy PSBT. Losing the artifacts removes convenience but cannot lose bitcoin; keys plus the static descriptor remain sufficient for vault recovery.

## Emergency access

An annual policy may reserve a fixed amount for one emergency withdrawal during that vault epoch. Zero disables it. A positive limit adds one independent connector and exactly two presigned mixed-input PSBTs:

1. **Trigger:** spends the vault remainder plus the emergency connector and creates a staging vault output, vault change, and a successor connector.
2. **Withdrawal:** spends the staging output plus that successor connector to a fresh hot-wallet address after a time-based BIP68 delay of at least one week.

Conceptually:

```text
vault remainder + trigger connector
    └─ emergency trigger
         ├─ staged amount under vault script ─┐
         ├─ withdrawal connector ─────────────┴─ withdraw after ~1 week → hot wallet
         └─ remaining cold change under vault script

Phone or HWW cancellation:
    withdrawal connector → ordinary wallet change
    staged vault output   → remains untouched
```

The staging output deliberately uses the existing vault script. The withdrawal's version-2 transaction and time-based `nSequence` commit to the one-week rule; Bitcoin rejects it until the delay elapses. BIP68's smallest representable delay of at least seven days is 605,184 seconds—seven days plus 6 minutes 24 seconds.

The HWW validates the source outpoints, exact amount, destination, delay, change, connector transitions, and fees, then signs the trigger and withdrawal vault inputs during the same one-prompt annual ceremony. Their controller inputs remain unsigned. The phone stores both incomplete PSBTs encrypted and completes the selected action with the live controller key.

Cancellation is not presigned and does not spend the staging output. It dynamically spends only the withdrawal connector, leaving the staged bitcoin under the normal vault policy and invalidating the withdrawal. Only one trigger can exist per epoch because it commits to the unique rollover remainder and initial emergency connector. A fresh emergency chain is created only by a later rollover.

The emergency reserve is funded before the allowance chain. If the balance can fund the emergency amount but fewer than twelve allowance steps, Anzen preserves the exact emergency amount and creates the largest fundable sequential allowance chain. If it cannot fund the emergency amount, its fees, connectors, and non-dust cold change, construction fails rather than silently weakening the approved limit.

### Monthly and soft spending limits

The monthly limit is fixed during policy activation. The joint signature on each monthly authorization commits to a transaction that releases exactly that amount to the mobile hot wallet. It is the effective hard limit and is enforced by Bitcoin signature validation of the fixed presigned transaction, not by a covenant opcode in the vault script.

The soft limit is a phone-side preference that can be changed at any time between zero and the approved monthly limit. To use a smaller soft limit, the phone broadcasts the full authorization and immediately spends its output in a child transaction that:

- keeps the chosen soft-limit amount in the mobile hot wallet; and
- sends the balance back to the static vault address, accounting for fees.

The child can also provide CPFP fee bumping for the presigned parent. The soft limit is not a security boundary: a compromised phone can keep the full monthly-limit output instead of creating the cold-return child. MVP actions are manual CLI commands; automatic broadcasting and automatic soft-limit enforcement are out of scope.

## Wallet properties

- **Phone only:** can release the next monthly-limit amount after each sequential relative delay without carrying the HWW.
- **Phone-only revocation:** can spend the live monthly connector, leaving principal in place while invalidating the current and every later authorization.
- **Phone-only emergency access:** can start one fixed emergency withdrawal per epoch, cancel it during the one-week window, or complete it after the delay without carrying the HWW.
- **HWW-only policy revocation:** can spend every live connector to an explicitly verified destination, invalidating all phone-held policy actions without moving vault principal.
- **Phone + HWW:** can spend the entire balance immediately with one joint MuSig2 signature.
- **Unused monthly allowances:** remain under full vault protection inside one live allowance-chain output.
- **Phone alone, beyond that:** cannot move savings. Every other spend needs the HWW.
- **HWW alone:** can sweep the vault after its time lock of about 388 days.
- **Annual rollover:** consumes every old connector, restarts the HWW's time lock, and creates only the controller state required by the renewed policy.
- **No essential transaction state:** keys plus the static descriptor are sufficient to recover the vault; presigned policy transactions are convenience authorizations only.
- **No provider dependency:** spending limits and the fallback require no server co-signer.
- **Safe long-lived receive policy:** relative timelocks start when each UTXO confirms, so payments to an old address do not enter an already-expired absolute policy.

The chain enforces spacing rather than calendar dates: the next monthly-limit authorization becomes available roughly 30 days after the preceding chain output confirms. Unused authorizations cannot accumulate, because a later hop does not exist until the current one executes.

The monthly limit is transaction-enforced in **satoshis**, not dollars.

## MVP implementation scope

The initial implementation is a Rust CLI built with BDK and run by default against a Bitcoin Core regtest node. The CLI and node are orchestrated with Docker. Bitcoin Core RPC and Electrum are selectable independently of the configured regtest or mainnet network, and every connection verifies that the backend is on the expected chain. An experimental mainnet mode is available only when `--dangerously-enable-mainnet` is used during initialization and repeated on every later invocation. This mode does not make the MVP production-safe: software HWW keys, printed mnemonics, fixed fees, public-server privacy/trust, and crash consistency remain unresolved. Signet, a graphical mobile application, and integration with a physical hardware wallet are out of scope.

For the MVP:

- `M` and `H` are software keys managed by separate simulated phone and HWW components;
- hot-wallet receiving and change use normal external and internal address derivation;
- the phone mnemonic and cold-storage descriptor are encrypted together with a random symmetric key and stored locally as a stand-in for cloud storage;
- the symmetric key is authenticated-encrypted to an HWW-derived key, so only the signer can unwrap the backup;
- every incomplete monthly and emergency action PSBT is separately encrypted to a phone-derived encryption key and stored as an independent local ciphertext;
- the phone's pending MuSig2 nonces live in single-use local signing sessions that never enter the backup;
- all ordinary transactions use a fixed fee rate of 1 sat/vB;
- policy setup, rollover, signing, revocation, allowance use, emergency initiation/withdrawal/cancellation, soft-limit return, recovery, and sweeping are explicit CLI actions rather than automated behavior.

Device operations are separated in the CLI. Phone actions live under `anzen phone *`; HWW actions live under `anzen hww *`. Vault initialization is likewise explicit: initialize both devices in either order, then run `anzen init`. The final command prints only the cold-storage descriptor and public policy details; hot-wallet descriptors remain internal.

An optional `anzen phone init --vanity` mode requires the HWW to be initialized first because the final address commits to both device keys. It searches the phone's final non-hardened vault-key derivation index until the combined address begins with `bc1pvault` on mainnet, `tb1pvault` on Mutinynet or `bcrt1pvault` on regtest. The search runs on the GPU by default (a wgpu compute shader given only public data: the vault xpub, the HWW key and the leaf hash) or, with `--cpu`, across all available CPU threads using the library code; either way the winning index is re-derived with the library code before it is used. The mnemonic and winning index together reproduce the phone key, so the index is persisted in device state and included in authenticated encrypted recovery payloads. An existing funded vault can use `anzen phone rotate-key --vanity` to grind a replacement phone key against its current HWW key and feed the result through the ordinary HWW-approved cooperative rotation. That migration moves every old UTXO on-chain, preserves the active monthly and emergency policy, and reuses a matching pending vanity key if proposal construction is retried. Grinding derives each candidate address with the protocol's own script construction, including MuSig2 key aggregation, and is tested against the canonical descriptor and its address.

The implementation library is split into three public modules. `core` contains shared policy, PSBT, cryptographic, protocol, storage-format, and chain primitives and does not depend on a device implementation. `hot_wallet` contains phone/BDK behavior and depends only on `core`. `cold_wallet` contains the minimal HWW validation, backup, and signing surface and likewise depends only on `core`; it has no hot-wallet or network-client dependency. The CLI performs chain scanning/broadcast and composes these APIs into commands. This boundary is intended to let mobile apps reuse `hot_wallet`, hardware signing apps or firmware reuse the smaller `cold_wallet` surface, and reviewers audit the cold path without BDK/mobile implementation details.

The vault-policy protocol has three stages:

1. `anzen phone set-policy --monthly-limit SATS --emergency-access-limit SATS --output PROPOSAL.json` constructs the rollover, sequential allowance authorizations, and optional two-action emergency package, starts a signing session with a fresh phone nonce for every vault input, and emits a portable JSON policy object; action controller inputs remain unsigned.
2. `anzen hww confirm-policy PROPOSAL.json --output APPROVED.json` presents the complete high-level policy once, independently rebuilds every transaction and connector transition, and returns its nonce and partial signature for every vault input without per-transaction prompts.
3. `anzen phone activate-policy APPROVED.json` verifies the signer's partial signatures, finishes every joint signature from its session, broadcasts the fully signed rollover, and stores every incomplete monthly and emergency action PSBT as an individually encrypted phone artifact. Later phone actions sign the one live controller input and broadcast only the selected action.

The JSON interchange embeds PSBTs plus a versioned policy/batch manifest, so the simulated devices do not share an implicit signing workspace. Phone backup restoration, cooperative sweeping, and phone-key rotation use the same explicit JSON handoff model.

In addition to focused automated tests, the repository should provide isolated end-to-end terminal tests funded with 2 BTC and configured with a 0.1 BTC monthly limit. They should print human-readable seeds and public keys (regtest only), policies, timelocks, Miniscript, addresses, transaction IDs, balances, connector state, presigned transaction details, simulated time and block advancement, sequential allowance execution, phone and HWW revocation, successful and cancelled emergency access, on-time and forgotten annual rollover, recovery actions, and the loss/theft scenarios described below.

The demonstration should use the real BIP68 30-day delay for each allowance hop on a real Bitcoin Core regtest node. The test harness may use regtest mock time and on-demand block generation to advance the chain monotonically, but it must show that step 1 is relative to rollover confirmation, step 2 is relative to step 1 confirmation, and a live-hop revocation invalidates later descendants. The HWW's time-based fallback is exercised the same way: mock time moves forward and eleven blocks are mined so median time past reaches the boundary, and the test shows Bitcoin Core rejecting the sweep one second before it opens and accepting the same signed transaction at the boundary. The protocol's key aggregation is checked against an independent MuSig2 implementation, and joint key-path signatures are spent through the same Bitcoin Core node in the rollover, rotation and sweep tests.

The hardware-wallet performance benchmark shared by the Ledger app and the Trezor port still models the earlier script-path signing workload. Measuring on-device MuSig2 partial signing, which needs point arithmetic on each device, is follow-up work.

## Loss and theft scenarios

- **Lost, broken, or stolen locked phone**
  - A stolen device that does not yield its key is equivalent to a lost device for vault security.
  - Recover the encrypted mobile-key backup from cloud storage using HWW decryption.
  - Rotate to a fresh mobile key while preserving the active monthly and emergency-access limits. The rotation proposal chains a replacement annual rollover and policy schedule to the emergency sweep, and the HWW signs both under one high-level rotation approval.

- **Extracted mobile key**
  - The attacker can steal the current hot balance and use the currently matured allowance authorization or start the emergency withdrawal.
  - The attacker can never spend the main vault: the phone has no path of its own.
  - The HWW can cancel the emergency withdrawal and revoke every live action. Recover the mobile key using the HWW and sweep every vault output to fresh keys, invalidating the old allowance chain while replacing it with an equivalent schedule encrypted to the new phone key.

- **Lost, broken, or stolen locked HWW**
  - A stolen device that does not yield its key is equivalent to a lost device for vault security.
  - Restore the HWW from its maker's seed backup. It restores the same key, so the vault, policy and phone are unchanged.
  - The phone can continue using the existing allowance chain meanwhile.

- **Extracted HWW key**
  - The attacker cannot spend immediately; the HWW-only path is at least about 388 days away.
  - Any renewal before then moves every vault output and restarts the time lock. To retire the key, the owner sweeps with both devices to a new vault set up with a fresh signer.

- **Lost phone and unavailable cloud backup**
  - Funds remain locked until the HWW-only fallback opens, about 388 days after the last renewal.
  - The surviving HWW can then recover the vault alone, coordinated by a new phone or computer that supplies only public chain data.

- **Lost phone and lost HWW**
  - If the HWW's seed backup survives, restore the HWW, then restore the phone key from the cloud backup with it.
  - If the cloud backup is also gone, the restored HWW recovers the vault alone once its time lock opens.
  - If the HWW's seed backup is also lost, the savings are unrecoverable, as with any hardware wallet.

- **Cloud account compromised**
  - The attacker obtains the encrypted payload and its HWW-wrapped key, and still needs the HWW to decrypt it.
  - Even a decrypted phone key is no more than an extracted mobile key: bounded spending, never the savings.

- **Rollover forgotten**
  - Nothing opens for the phone. Only the HWW-only path opens, about 388 days after the oldest live output's confirmation.
  - A previously extracted HWW key becomes dangerous at that point.
  - The wallet must provide escalating warnings based on the oldest live UTXO. A late renewal with both devices restarts the time lock.

- **Both mobile and HWW keys compromised**
  - The attacker can satisfy the immediate 2-of-2 path and steal the full balance.

## Summary

The design provides renewable 2-of-2 cold storage spent with one joint MuSig2 signature, bounded sequential mobile liquidity, recovery from a lost phone with the signer, and a delayed hardware-wallet-only fallback, using only existing Bitcoin consensus rules. It is a hardware wallet with a second factor: the phone can never be the single point of theft.

Its central trade-off is operational: users must complete an annual rollover before the hardware wallet's solo path opens. In exchange, they get strong cold-storage security without carrying the HWW for routine monthly spending and without relying on a third-party co-signer.
