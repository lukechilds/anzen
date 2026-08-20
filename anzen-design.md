# Anzen: Renewable Bitcoin Vault with Bounded Mobile Spending

## Overview

Anzen combines:

- a **mobile key** `M`;
- a **hardware-wallet key** `H`;
- renewable **2-of-2 cold storage**;
- delayed single-device recovery;
- a sequential, relatively timelocked monthly allowance chain;
- one cancellable, one-week-delayed emergency withdrawal per vault epoch.

The main balance normally requires both devices. The user performs an expected rollover roughly once per year, moving all funds into fresh vault outputs and resetting the relative recovery timers.

The phone can also access a policy-defined monthly spending limit without carrying the HWW. A lower soft limit controls how much of an unlocked allowance the phone keeps hot. The same annual policy may include one larger emergency withdrawal that the phone can initiate immediately, cancel during a one-week safety window, and complete only after that delay. A new vault starts with both features disabled; their limits are introduced later through an explicit phone proposal and HWW approval. No server, custodian, or online co-signer is required.

## Designed to be hard to misuse

Anzen's operating model should remain simple even though the transaction graph underneath it is not. The wallet must guide the user through complete, high-level operations—create a policy, approve it, withdraw, revoke, renew, rotate, or recover—rather than expose raw transaction machinery and leave the user to assemble a safe sequence themselves. At every stage it should explain what happened, what protection remains active, and what action is required next.

The HWW approves a human-readable vault policy, not a stream of unrelated transaction prompts. It independently verifies the complete transaction graph implied by that policy and signs the batch in one ceremony. Once approved, the phone can safely execute or revoke the permitted behavior for the next year without asking the user to reconstruct the policy or repeatedly access cold storage.

Annual renewal should be one guided operation on the same calendar date each year. The wallet tracks the oldest live vault output, warns well before recovery paths mature, and guides the user through refreshing every live output and renewing the policy. Recovery and key rotation should likewise be complete workflows with safe defaults, explicit checks, and a clear destination—not collections of low-level tools that require the user to invent a recovery procedure during an emergency.

This principle applies throughout the product: monthly spending begins disabled, the HWW validates proposals independently, partial funding degrades predictably with a warning, and dangerous or incomplete state transitions are rejected. The goal is not merely to document the safe path, but to make the safe path the normal and easiest way to use Anzen.

## Vault script

Each vault output is Taproot with a provably unspendable **NUMS internal key**, so there is no usable key-path spend. All spending conditions are explicit Tapscript leaves.

Conceptually:

```text
tr(
  NUMS,
  {
    multi_a(2, M, H),
    {
      and_v(v:older(61200), pk(M)),
      and_v(v:older(65535), pk(H))
    }
  }
)
```

This means:

```text
M + H       can spend immediately
M alone     can spend after 61,200 blocks
H alone     can spend after 65,535 blocks
```

Assuming ten-minute blocks:

- `61,200` blocks is approximately **425 days**, or **14 months**.
- `65,535` blocks is approximately **455 days**, or **15 months**.
- The phone gets approximately a **one-month priority window** before the HWW fallback activates.

The cooperative leaf is placed at the shallowest level of the binary Taproot tree because it is expected to be used most often. The two recovery leaves are logically independent alternatives; their nesting only determines Merkle-tree depth and witness size.

The normal receive policy deliberately reuses the same `M` and `H` keys and the same Taproot address for every vault output, including rollover outputs and cold change. This sacrifices address-level privacy in favor of a simple, durable single vault address. An emergency rotation after a key compromise necessarily creates a new policy and address.

## Expected rollover schedule

The wallet should prompt the user to roll over all vault funds approximately once per year.

With the delays above, a 12-month expected rollover gives roughly:

- **two months of grace** before the phone-only path activates;
- **three months of grace** before the HWW-only path activates.

The rollover should consume every current vault UTXO plus every live policy connector. When monthly spending is enabled, it creates one allowance-chain output funding up to twelve sequential releases, one remainder at the same vault address, and one monthly connector. Emergency access adds a second connector. With both features disabled or unfunded, rollover creates only the vault remainder. Every new vault output begins its recovery timer when it confirms; replacing the connectors invalidates all unused actions from the previous epoch.

The expected calendar date is a UX reminder. The actual deadlines are based on the confirmation height of each UTXO, so the wallet must track the oldest live vault output and show an estimated recovery date.

## Why these delay values are used

Bitcoin relative timelocks are encoded through BIP68 `nSequence` semantics and enforced in script with `CHECKSEQUENCEVERIFY` / Miniscript `older()`.

Only 16 bits are available for the relative delay value, so the maximum value is `65,535`.

In block-based mode:

```text
65,535 blocks × 10 minutes ≈ 455 days
```

This prevents directly expressing the originally desired pair of approximately 15 months for the phone and 16 months for the HWW in a single output. The chosen values are therefore close to the maximum while preserving approximately one month of phone priority.

This is a **BIP68 limitation**. It is not a limitation on the number of timelock branches in one Taproot output.

### Block-time drift

Block-based delays are approximate in wall-clock time.

If Bitcoin hashrate rises, blocks may arrive faster until the next difficulty adjustment. Over a long delay, realistic ASIC and hashrate growth may move the effective date earlier by several days. Random block-time variance can also shift it by several days.

This is why the design uses margins measured in months rather than relying on an exact calendar day. The approximately one-month phone-priority window should be treated as an estimated block interval, not a guaranteed number of wall-clock days.

### Possible soft-fork improvement

A future soft fork could extend relative-timelock range by assigning meaning to currently reserved `nSequence` bits or introducing a new relative-time primitive with a larger range.

That could allow:

- multi-year relative time locks;
- true time-based delays much longer than the current approximately 12.7-month maximum;
- a clean 15-month phone fallback and 16-month HWW fallback;
- predictable long-term relative recovery without relying on block-count approximations.

Until such a change is activated, block-based CSV is the only stateless single-output construction that gets close to the desired 15-month range.

## Revocable policy connectors

The vault script remains the only script that controls principal. Revocable policy state is represented by small 10,000-sat connector UTXOs under this fixed Taproot policy:

```text
tr(NUMS,{pk(M),pk(H)})
```

The controller reuses the fixed vault public keys. Either device can spend it alone, there is no usable key path, and no controller xpub, derivation index, or additional seed is required. All connectors in one key epoch use the same address; the particular state is identified by its outpoint.

Every presigned policy action contains two inputs:

1. a vault input whose cooperative 2-of-2 leaf is signed by `M` and `H` during annual approval; and
2. one exact connector input that remains unsigned until execution.

The signatures use Taproot `SIGHASH_DEFAULT`, so the signed vault input commits to every input, previous output, sequence, and output in the transaction—including the exact controller outpoint and the next controller output. The PSBT cannot finalize until a device signs its connector input. The phone adds that signature only when executing the action.

Revocation is constructed dynamically and spends only the current connector. It leaves principal untouched in the existing vault output and makes the presigned action invalid because the two transactions conflict on the connector outpoint. Phone revocation sends the connector value left after fees to a fresh hot-wallet change address. HWW revocation sends it to an explicitly displayed and approved destination. A revocation must never create another controller output.

Annual rollover consumes every live old connector and creates exactly one replacement for each enabled independent chain. An intermediate action rolls its connector to the same controller address; a terminal action consumes it without replacement. Phone-key rotation consumes all old-controller outputs before installing the replacement key, then a renewed policy creates connectors under the new keys.

The MVP fixes connector value and fees at 10,000 sats and 1 sat/vB. Production requires a current-feerate strategy, optional fee inputs, and RBF/CPFP support. Until revocation confirms, it can race a mature policy action that spends the same connector.

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

The monthly limit is fixed during policy activation. Both signatures on each monthly authorization commit to a transaction that releases exactly that amount to the mobile hot wallet. It is the effective hard limit and is enforced by Bitcoin signature validation of the fixed presigned transaction, not by a covenant opcode in the vault script.

The soft limit is a phone-side preference that can be changed at any time between zero and the approved monthly limit. To use a smaller soft limit, the phone broadcasts the full authorization and immediately spends its output in a child transaction that:

- keeps the chosen soft-limit amount in the mobile hot wallet; and
- sends the balance back to the static vault address, accounting for fees.

The child can also provide CPFP fee bumping for the presigned parent. The soft limit is not a security boundary: a compromised phone can keep the full monthly-limit output instead of creating the cold-return child. MVP actions are manual CLI commands; automatic broadcasting and automatic soft-limit enforcement are out of scope.

## Wallet properties

- **Phone only:** can release the next monthly-limit amount after each sequential relative delay without carrying the HWW.
- **Phone-only revocation:** can spend the live monthly connector, leaving principal in place while invalidating the current and every later authorization.
- **Phone-only emergency access:** can start one fixed emergency withdrawal per epoch, cancel it during the one-week window, or complete it after the delay without carrying the HWW.
- **HWW-only policy revocation:** can spend every live connector to an explicitly verified destination, invalidating all phone-held policy actions without moving vault principal.
- **Phone + HWW:** can spend the entire balance immediately.
- **Unused monthly allowances:** remain under full vault protection inside one live allowance-chain output.
- **Annual rollover:** consumes every old connector, resets the recovery timers, and creates only the controller state required by the renewed policy.
- **No essential transaction state:** keys plus the static descriptor are sufficient to recover the vault; presigned policy transactions are convenience authorizations only.
- **No provider dependency:** spending limits and recovery paths require no server co-signer.
- **Safe long-lived receive policy:** relative timelocks start when each UTXO confirms, so payments to an old address do not enter an already-expired absolute policy.

The chain enforces spacing rather than calendar dates: the next monthly-limit authorization becomes available roughly 30 days after the preceding chain output confirms. Unused authorizations cannot accumulate, because a later hop does not exist until the current one executes.

The monthly limit is transaction-enforced in **satoshis**, not dollars.

## MVP implementation scope

The initial implementation is a Rust CLI built with BDK and run by default against a Bitcoin Core regtest node. The CLI and node are orchestrated with Docker. Bitcoin Core RPC and Electrum are selectable independently of the configured regtest or mainnet network, and every connection verifies that the backend is on the expected chain. An experimental mainnet mode is available only when `--dangerously-enable-mainnet` is used during initialization and repeated on every later invocation. This mode does not make the MVP production-safe: software HWW keys, printed mnemonics, fixed fees, public-server privacy/trust, and crash consistency remain unresolved. Signet, a graphical mobile application, and integration with a physical hardware wallet are out of scope.

For the MVP:

- `M` and `H` are software keys managed by separate simulated phone and HWW components;
- hot-wallet receiving and change use normal external and internal address derivation;
- the phone mnemonic and cold-storage descriptor are encrypted together with a random symmetric key and stored locally as a stand-in for cloud storage;
- the symmetric key is authenticated-encrypted to an HWW-derived key and may also be independently OpenPGP-encrypted to each configured recovery friend's public key; the complete friend-wrapper manifest is authenticated with that symmetric key before it can be reused during rotation, and every friend is a 1-of-N recovery recipient rather than part of a threshold scheme;
- every incomplete monthly and emergency action PSBT is separately encrypted to a phone-derived encryption key and stored as an independent local ciphertext;
- all ordinary transactions use a fixed fee rate of 1 sat/vB;
- policy setup, rollover, signing, revocation, allowance use, emergency initiation/withdrawal/cancellation, soft-limit return, recovery, and sweeping are explicit CLI actions rather than automated behavior.

Device operations are separated in the CLI. Phone actions live under `anzen phone *`; HWW actions live under `anzen hww *`. Vault initialization is likewise explicit: initialize both devices in either order, then run `anzen init`. The final command prints only the cold-storage descriptor and public policy details; hot-wallet descriptors remain internal.

An optional `anzen phone init --vanity` mode requires the HWW to be initialized first because the final address commits to both device keys. It searches the phone's final non-hardened vault-key derivation index across all available CPU threads until the combined address begins with `bc1pvault` on mainnet or `bcrt1pvault` on regtest. The mnemonic and winning index together reproduce the phone key, so the index is persisted in device state and included in authenticated encrypted recovery payloads. An existing funded vault can use `anzen phone rotate-key --vanity` to grind a replacement phone key against its current HWW key and feed the result through the ordinary HWW-approved cooperative rotation. That migration moves every old UTXO on-chain, preserves the active monthly and emergency policy, and reuses a matching pending vanity key if proposal construction is retried. The canonical Miniscript policy remains the source of truth; an optimized address encoder used during grinding is tested byte-for-byte against its scripts and resulting address.

The implementation library is split into three public modules. `core` contains shared policy, PSBT, cryptographic, protocol, storage-format, and chain primitives and does not depend on a device implementation. `hot_wallet` contains phone/BDK behavior and depends only on `core`. `cold_wallet` contains the minimal HWW validation, backup, and signing surface and likewise depends only on `core`; it has no hot-wallet or network-client dependency. The CLI performs chain scanning/broadcast and composes these APIs into commands. This boundary is intended to let mobile apps reuse `hot_wallet`, hardware signing apps or firmware reuse the smaller `cold_wallet` surface, and reviewers audit the cold path without BDK/mobile implementation details.

The vault-policy protocol has three stages:

1. `anzen phone set-policy --monthly-limit SATS --emergency-access-limit SATS --output PROPOSAL.json` constructs the rollover, sequential allowance authorizations, and optional two-action emergency package. It signs the phone side of every vault input and emits a portable JSON policy object; action controller inputs remain unsigned.
2. `anzen hww confirm-policy PROPOSAL.json --output APPROVED.json` presents the complete high-level policy once, independently validates every PSBT and connector transition, and signs the HWW side of every vault input without per-transaction prompts.
3. `anzen phone activate-policy APPROVED.json` verifies both approvals, broadcasts the fully signed rollover, and stores every incomplete monthly and emergency action PSBT as an individually encrypted phone artifact. Later phone actions sign the one live controller input and broadcast only the selected action.

The JSON interchange embeds PSBTs plus a versioned policy/batch manifest, so the simulated devices do not share an implicit signing workspace. Phone backup restoration, cooperative sweeping, and phone-key rotation use the same explicit JSON handoff model.

In addition to focused automated tests, the repository should provide isolated end-to-end terminal tests funded with 2 BTC and configured with a 0.1 BTC monthly limit. They should print human-readable seeds and public keys (regtest only), policies, timelocks, Miniscript, addresses, transaction IDs, balances, connector state, presigned transaction details, simulated time and block advancement, sequential allowance execution, phone and HWW revocation, successful and cancelled emergency access, on-time and forgotten annual rollover, recovery actions, and the loss/theft scenarios described below.

The demonstration should use the real BIP68 30-day delay for each allowance hop on a real Bitcoin Core regtest node. The test harness may use regtest mock time and on-demand block generation to advance the chain monotonically, but it must show that step 1 is relative to rollover confirmation, step 2 is relative to step 1 confirmation, and a live-hop revocation invalidates later descendants. Block-based recovery paths must be exercised by mining their required block counts rather than by changing mock time.

## Loss and theft scenarios

- **Lost, broken, or stolen locked phone**
  - A stolen device that does not yield its key is equivalent to a lost device for vault security.
  - Recover the encrypted mobile-key backup from cloud storage using HWW decryption.
  - Rotate to a fresh mobile key while preserving the active monthly and emergency-access limits. The rotation proposal chains a replacement annual rollover and policy schedule to the emergency sweep, and the HWW signs both under one high-level rotation approval.

- **Extracted mobile key**
  - The attacker can steal the current hot balance and use the currently matured allowance authorization.
  - The attacker cannot spend the main vault before the phone fallback matures.
  - Recover the mobile key using the HWW and immediately sweep every vault output to fresh keys, invalidating the old allowance chain while replacing it with an equivalent schedule encrypted to the new phone key.

- **Lost, broken, or stolen locked HWW**
  - A stolen device that does not yield its key is equivalent to a lost device for vault security.
  - The phone can continue using the existing allowance chain.
  - After approximately 61,200 blocks, the phone can recover each vault UTXO alone.

- **Extracted HWW key**
  - The attacker cannot spend immediately.
  - The phone fallback activates first.
  - The legitimate phone sweeps funds during the approximately one-month priority window before the HWW-only path becomes valid.

- **Lost phone and unavailable cloud backup**
  - Funds remain locked until the HWW-only fallback matures.
  - The surviving HWW can then recover the vault alone.

- **Both devices lost**
  - Recovery depends on social recovery having been configured before the loss.
  - Any configured friend can use their OpenPGP private key to unwrap the symmetric backup key and authenticate/decrypt the phone mnemonic plus descriptor.
  - The recovered phone key still cannot bypass the 61,200-block phone-recovery delay. After that path matures, `anzen social emergency-access` can sweep directly to replacement keys without recreating either lost device.
  - Without social recovery, simultaneous permanent loss of both devices is unrecoverable.

- **Cloud account compromised**
  - The attacker obtains encrypted payload and key wrappers but still needs the HWW decryption capability or one configured friend's OpenPGP private key.
  - A recovery friend is trusted with eventual phone-key capability and can spend matured monthly artifacts or use the delayed phone-recovery path; social recovery is therefore a deliberate 1-of-N trust expansion.

- **Rollover forgotten**
  - The phone-only path eventually activates, followed by the HWW-only path.
  - Any previously extracted key becomes dangerous at its corresponding maturity.
  - The wallet must provide escalating warnings based on the oldest live UTXO.

- **Both mobile and HWW keys compromised**
  - The attacker can satisfy the immediate 2-of-2 path and steal the full balance.

## Summary

The design provides renewable 2-of-2 cold storage, bounded sequential mobile liquidity, and eventual recovery from either device loss using only existing Bitcoin consensus rules.

Its central trade-off is operational: users must complete an annual rollover before the delayed single-key paths activate. In exchange, they get strong cold-storage security without carrying the HWW for routine monthly spending and without relying on a third-party co-signer.
