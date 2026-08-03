# Renewable Bitcoin Vault with Bounded Mobile Spending

## Overview

This wallet combines:

- a **mobile key** `M`;
- a **hardware-wallet key** `H`;
- renewable **2-of-2 cold storage**;
- delayed single-device recovery;
- exact calendar-based monthly spending authorizations.

The main balance normally requires both devices. The user performs an expected rollover roughly once per year, moving all funds into fresh vault outputs and resetting the relative recovery timers.

The phone can also access a predefined hard spending limit each month without carrying the HWW. No server, custodian, or online co-signer is required.

## Vault script

Each vault output is Taproot with a provably unspendable **NUMS internal key**, so there is no usable key-path spend. All spending conditions are explicit Tapscript leaves.

Conceptually:

```text
tr(
  NUMS,
  {
    multi_a(2, M_i, H_i),
    {
      and_v(v:older(61200), pk(M_i)),
      and_v(v:older(65535), pk(H_i))
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

## Expected rollover schedule

The wallet should prompt the user to roll over all vault funds approximately once per year.

With the delays above, a 12-month expected rollover gives roughly:

- **two months of grace** before the phone-only path activates;
- **three months of grace** before the HWW-only path activates.

The rollover should consume every current vault UTXO and every unused monthly-spending chunk, then recreate them under fresh outputs with newly reset relative timers.

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

During each rollover, the wallet splits part of the vault into twelve independent cold chunks and creates one presigned transaction for each month.

Conceptually:

```text
Monthly chunk i
    └─ after calendar date i
         ├─ hard spending limit → mobile hot wallet
         └─ remainder           → fresh vault output
```

Each transaction:

- is signed in advance by both `M` and `H`;
- uses absolute `nLockTime` for its exact monthly date;
- is encrypted with a dedicated phone-derived encryption key;
- is stored on the phone and in encrypted cloud storage;
- can be decrypted and broadcast by the phone after its date.

Because the chunks are independent, only twelve transactions are needed.

If a month is unused, its transaction is not broadcast and the chunk remains cold. The next annual rollover spends the original chunk, permanently invalidating any retained copy of the old presigned transaction.

Loss of the presigned transactions does **not** lose bitcoin. It only removes the phone-only convenience path; the underlying chunks remain recoverable through the vault script.

Presigned transactions need a reliable CPFP fee-bumping path because their original fee is chosen in advance.

## Wallet properties

- **Phone only:** can access up to one newly authorized hard-limit chunk per month without carrying the HWW.
- **Phone + HWW:** can spend the entire balance immediately.
- **Unused monthly allowance:** remains under full vault protection until the corresponding presigned transaction is broadcast.
- **Annual rollover:** resets the recovery timers and invalidates all unused old monthly authorizations.
- **No essential transaction state:** keys plus the static descriptor are sufficient to recover the vault; presigned monthly transactions are convenience authorizations only.
- **No provider dependency:** spending limits and recovery paths require no server co-signer.
- **Safe long-lived receive policy:** relative timelocks start when each UTXO confirms, so payments to an old address do not enter an already-expired absolute policy.

One new hard-limit authorization becomes available per month. This is not a strict rolling monthly cap: several matured but unused authorizations can accumulate until rollover.

The hard limit is consensus-enforced in **satoshis**, not dollars.

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
