# Renewable Bitcoin Vault with Bounded Mobile Spending

## Overview

This wallet combines:

- a **mobile key** `M`;
- a **hardware-wallet key** `H`;
- renewable **2-of-2 cold storage**;
- delayed single-device recovery;
- exact calendar-based monthly spending authorizations.

The main balance normally requires both devices. The user performs an expected rollover roughly once per year, moving all funds into fresh vault outputs and resetting the relative recovery timers.

The phone can also access a policy-defined monthly spending limit without carrying the HWW. A lower soft limit controls how much of an unlocked allowance the phone keeps hot. A new vault starts with monthly spending disabled; the limit is introduced later through an explicit phone proposal and HWW approval. No server, custodian, or online co-signer is required.

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

The rollover should consume every current vault UTXO and every unused monthly-spending chunk, then recreate them as new outputs to the same vault address with newly reset relative timers.

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

## Monthly mobile spending

Vault initialization creates only the static cold-storage policy. Its monthly limit is zero, so there are no presigned monthly transactions. The phone later proposes a limit, including zero to disable spending, and the HWW confirms it through the signing protocol described below.

When a positive monthly policy is activated, the wallet divides the entire post-fee vault balance equally across twelve independent cold chunks and creates a pair of presigned transactions for each month. Each authorization releases the approved monthly limit and returns that chunk's remainder to the vault address. If the balance cannot fund twelve chunks that can each release the full limit plus fees, the wallet warns the user, chooses the largest fundable chunk count below twelve, and creates authorizations only for those earliest consecutive months. The entire post-fee balance is still divided equally across the selected chunks; there is no separate main-vault remainder output. Any indivisible satoshi remainder is distributed deterministically across the earliest chunks. Activating a zero-limit policy instead creates one cold rollover output and no monthly pairs.

Conceptually:

```text
Monthly chunk i
    ├─ monthly authorization, after 00:00 UTC on day 1 of month i
    │    ├─ monthly limit       → mobile hot wallet
    │    └─ remainder           → vault address
    └─ immediate revocation
         └─ all value, less fee      → vault address
```

Each monthly authorization transaction:

- is signed in advance by both `M` and `H`;
- uses absolute timestamp `nLockTime` for `00:00 UTC` on the first day of its calendar month;
- sends exactly the approved monthly-limit amount to a fresh address from the mobile hot wallet;
- returns any cold remainder to the static vault address;
- is encrypted individually with a dedicated phone-derived encryption key;
- is stored on the phone and in encrypted cloud storage;
- can be decrypted and broadcast by the phone after its date.

Timestamp locktimes are evaluated using Bitcoin median-time-past. `00:00 UTC` is therefore the earliest policy time, not a promise of inclusion at exactly that wall-clock instant.

Each monthly chunk also has a conflicting presigned revocation transaction. It:

- is signed in advance by both `M` and `H` during the same signing ceremony;
- has no monthly absolute timelock and can be broadcast from the phone immediately;
- returns the chunk to the static vault address, less its transaction fee;
- is encrypted individually with the same dedicated phone-derived encryption key used for monthly transaction storage;
- invalidates the corresponding monthly authorization once it confirms because both transactions spend the same chunk.

The phone should revoke before the authorization matures. Until the revocation confirms, the two transactions remain conflicting alternatives; revoking after maturity can become a fee and confirmation race.

Because the chunks are independent, at most twelve authorization transactions and twelve matching revocation transactions are needed.

If a month is unused, neither transaction needs to be broadcast and the chunk remains cold. The next annual rollover spends the original chunk, permanently invalidating retained copies of both presigned transactions.

Loss of the presigned transactions does **not** lose bitcoin. It only removes the phone-only convenience path; the underlying chunks remain recoverable through the vault script.

Presigned transactions need a reliable CPFP fee-bumping path because their original fee is chosen in advance. The MVP uses a fixed fee rate of 1 sat/vB on regtest. A production design must also ensure that a phone-broadcast revocation has a phone-available fee-bumping path; returning every spendable output directly to the 2-of-2 vault would otherwise prevent immediate phone-only CPFP. The MVP implementation should carry an explicit code `TODO` at the revocation construction/broadcast boundary so this is not mistaken for a production-safe fee strategy.

### Monthly and soft spending limits

The monthly limit is fixed during policy activation. Both signatures on each monthly authorization commit to a transaction that releases exactly that amount to the mobile hot wallet. It is the effective hard limit and is enforced by Bitcoin signature validation of the fixed presigned transaction, not by a covenant opcode in the vault script.

The soft limit is a phone-side preference that can be changed at any time between zero and the approved monthly limit. To use a smaller soft limit, the phone broadcasts the full authorization and immediately spends its output in a child transaction that:

- keeps the chosen soft-limit amount in the mobile hot wallet; and
- sends the balance back to the static vault address, accounting for fees.

The child can also provide CPFP fee bumping for the presigned parent. The soft limit is not a security boundary: a compromised phone can keep the full monthly-limit output instead of creating the cold-return child. MVP actions are manual CLI commands; automatic broadcasting and automatic soft-limit enforcement are out of scope.

## Wallet properties

- **Phone only:** can access up to one newly authorized monthly-limit chunk per month without carrying the HWW.
- **Phone-only revocation:** can invalidate a future monthly authorization by broadcasting its presigned revocation transaction before the authorization matures.
- **Phone + HWW:** can spend the entire balance immediately.
- **Unused monthly allowance:** remains under full vault protection until the corresponding presigned transaction is broadcast.
- **Annual rollover:** resets the recovery timers and invalidates all unused old monthly authorizations.
- **No essential transaction state:** keys plus the static descriptor are sufficient to recover the vault; presigned monthly transactions are convenience authorizations only.
- **No provider dependency:** spending limits and recovery paths require no server co-signer.
- **Safe long-lived receive policy:** relative timelocks start when each UTXO confirms, so payments to an old address do not enter an already-expired absolute policy.

One new monthly-limit authorization becomes available per month. This is not a strict rolling monthly cap: several matured but unused authorizations can accumulate until rollover.

The monthly limit is transaction-enforced in **satoshis**, not dollars.

## MVP implementation scope

The initial implementation is a Rust CLI built with BDK and run against a Bitcoin Core regtest node. The CLI and node are orchestrated with Docker. Mainnet, signet, a graphical mobile application, and integration with a physical hardware wallet are out of scope.

For the MVP:

- `M` and `H` are software keys managed by separate simulated phone and HWW components;
- hot-wallet receiving and change use normal external and internal address derivation;
- the phone seed backup is encrypted to a backup/decryption key controlled by the simulated HWW and stored locally as a stand-in for cloud storage;
- every finalized authorization and revocation transaction is separately encrypted to a phone-derived encryption key and stored as an independent local ciphertext;
- all ordinary transactions use a fixed fee rate of 1 sat/vB;
- policy setup, rollover, signing, revocation, allowance use, soft-limit return, recovery, and sweeping are explicit CLI actions rather than automated behavior.

Device operations are separated in the CLI. Phone actions live under `vault phone *`; HWW actions live under `vault hww *`. Vault initialization is likewise explicit: `vault phone init`, `vault hww init`, then `vault init`. The final command prints only the cold-storage descriptor and public policy details; hot-wallet descriptors remain internal.

The monthly-policy protocol has three stages:

1. `vault phone set-policy --monthly-limit SATS --output PROPOSAL.json` constructs the rollover and monthly PSBTs, signs the phone side, and emits a portable JSON policy object.
2. `vault hww confirm-policy PROPOSAL.json --output APPROVED.json` presents the complete high-level policy once, obtains one approval, independently validates every PSBT against the manifest, and signs the complete batch without per-transaction prompts.
3. `vault phone activate-policy APPROVED.json` verifies both approvals, broadcasts the rollover, and stores the individually encrypted monthly artifacts.

The JSON interchange embeds PSBTs plus a versioned policy/batch manifest, so the simulated devices do not share an implicit signing workspace. Phone backup restoration, cooperative sweeping, and phone-key rotation use the same explicit JSON handoff model.

In addition to focused automated tests, the repository should provide a serial end-to-end terminal demonstration funded with 2 BTC and configured with a 0.1 BTC monthly limit. It should print human-readable seeds and public keys (regtest only), policies, timelocks, Miniscript, addresses, transaction IDs, balances, presigned transaction details, simulated calendar and block advancement, successful and revoked allowances, on-time and forgotten annual rollover, recovery actions, and the loss/theft scenarios described below.

The demonstration should derive its schedule from the actual UTC date when the test starts, with the first authorization on the next first day of a calendar month. It should still use a real Bitcoin Core regtest node. The test harness may use regtest mock time and on-demand block generation to advance the chain monotonically, mining until median-time-past is strictly later than the authorization locktime before testing a successful broadcast. Block-based recovery paths must be exercised by mining their required block counts rather than by changing mock time.

## Loss and theft scenarios

- **Lost phone**
  - Recover the encrypted mobile-key backup from cloud storage using HWW decryption.
  - Rotate to a fresh mobile key and recreate the monthly authorization schedule.

- **Stolen phone or extracted mobile key**
  - The attacker can steal the current hot balance and use any matured monthly authorizations.
  - The attacker cannot spend the main vault before the phone fallback matures.
  - Recover the mobile key using the HWW and immediately sweep all vault chunks to fresh keys, invalidating future authorizations.

- **Lost HWW**
  - The phone can continue using existing monthly authorizations.
  - After approximately 61,200 blocks, the phone can recover each vault UTXO alone.

- **Stolen HWW with extracted key**
  - The attacker cannot spend immediately.
  - The phone fallback activates first.
  - The legitimate phone sweeps funds during the approximately one-month priority window before the HWW-only path becomes valid.

- **Lost phone and unavailable cloud backup**
  - Funds remain locked until the HWW-only fallback matures.
  - The surviving HWW can then recover the vault alone.

- **Both devices lost**
  - Recovery depends on optional social recovery of the encrypted mobile-key backup.
  - Without social recovery, simultaneous permanent loss of both devices is unrecoverable.

- **Cloud account compromised**
  - The attacker obtains encrypted backup material but still needs the HWW decryption capability or an authorized social-recovery path.

- **Rollover forgotten**
  - The phone-only path eventually activates, followed by the HWW-only path.
  - Any previously extracted key becomes dangerous at its corresponding maturity.
  - The wallet must provide escalating warnings based on the oldest live UTXO.

- **Both mobile and HWW keys compromised**
  - The attacker can satisfy the immediate 2-of-2 path and steal the full balance.

## Summary

The design provides renewable 2-of-2 cold storage, bounded calendar-scheduled mobile liquidity, and eventual recovery from either device loss using only existing Bitcoin consensus rules.

Its central trade-off is operational: users must complete an annual rollover before the delayed single-key paths activate. In exchange, they get strong cold-storage security without carrying the HWW for routine monthly spending and without relying on a third-party co-signer.
