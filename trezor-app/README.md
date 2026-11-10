# Anzen for Trezor

The Anzen cold signer as a Trezor app: the phone sends a request, the Trezor shows what it does, and only an approval on the device signs it. Every decision is `anzen_signer::Signer`'s, the code every other Anzen signer runs. This crate supplies the Trezor's key and its screen.

## Status: proof of concept

It runs, end to end, in Trezor's emulator of a Safe 7 (`T3W1`), driven by the same Rust code the phone app uses. It has not run on a device.

Trezor's apps ("modular apps", loaded by the host each time it connects) are still in development: the SDK, the build tool and the firmware side live in an unmerged pull request. This app is built against our fork of that work, the `anzen-apps` branch of the `trezor-firmware` submodule, which adds three things:

- **The private key of a path in the app's manifest** (`crypto::get_private_key`). Trezor Core has no BIP340 or MuSig2 call, so an app can only sign for a vault by doing the arithmetic itself, with the key. Core releases a key only for a curve and a path the app's manifest declares, as it does for every other key operation. For this app that is `m/86'/0'/100'/0/0` and `m/86'/1'/100'/0/0`, the signer's vault key on mainnet and on the test networks, and nothing else in the wallet.
- **Apps in the emulator on Apple silicon.** Upstream links and loads emulator apps for Linux only.
- **Apps that live outside the firmware repository**, which this one does.

A Trezor running stock firmware cannot run this app.

### What the app keeps

Nothing. A Trezor app has no storage, so what the signer must remember (the vault it accepted, the last policy it approved, a rotation it approved) goes back to the phone under the signer's seal, and the phone hands it back with the next request (`anzen_signer::hardware::stateless`). The seal is HMAC-SHA256 under a key derived from the vault key. The phone can lose that state or withhold it, and never change it. It can present an older one: a seal proves the signer once held the state, not that it is the latest.

Because any state it seals at setup starts with no approved policy, the Trezor asks the person to confirm a vault's address when it is set up, where the Ledger app accepts silently and refuses a second vault.

### Limits

- A request may spend at most 64 inputs (`stateless::MAX_INPUTS`). The request, the transactions rebuilt from it and the answer are in the app's memory together, and on a Safe 7 the app's code, stack and heap share 384 KiB. The number is a guess at a safe one, not a measurement: the emulator gives an app all the memory it wants.
- Signing is pure Rust (`k256`), constant time but not hardened against someone probing the chip, and unaccelerated. Its speed on a device is unknown. The app reports progress after every signature, which also keeps Core from stopping it as unresponsive.
- Like the Ledger app, it cannot yet protect the phone's recovery backup.

## Layout

| Path | What it is |
| --- | --- |
| `src/main.rs` | The requests: check what the phone carried, review, sign, seal what to remember. |
| `src/screen.rs` | The review screen and the signing progress. |
| `tools/build.sh` | Builds the app: the emulator's image, the Safe 7's, or both. |
| `tools/emulator.sh` | Starts the emulator with a wallet loaded. |
| `tools/seed-emulator.py` | Loads a wallet into a running emulator (debug link). |
| `../crates/anzen-signer/src/hardware/stateless.rs` | The protocol, shared with the host. |
| `../crates/anzen-link/src/trezor/` | The host: Trezor's encrypted link, pairing, loading the app, the signer client. |

## Building and running it in the emulator

Everything Trezor-side runs in the firmware's Nix shell. Once, from `../trezor-firmware`:

```sh
git fetch origin && git switch anzen-apps    # this repository still pins the older `anzen` branch
git submodule update --init --recursive
nix-shell --run "uv sync"
```

Build the emulator with app support and the debug link (about three minutes the first time):

```sh
cd ../trezor-firmware
nix-shell --run "source .venv/bin/activate && xtask build firmware -m t3w1 -e --apps -p test-live"
```

Start it. This loads Trezor's public test wallet (twelve times `all`), and stays in the foreground:

```sh
tools/emulator.sh            # port 31324
```

Build the app for the emulator, from this directory:

```sh
tools/build.sh emulator
```

Then play the phone against it. These tests pair, load the app, set up a vault, approve policies, transfers, the signer's own actions and a phone rotation, reject, and tamper with the carried state, reading the Trezor's screen and pressing its buttons through the emulator's debug link:

```sh
cd ..
ANZEN_TREZOR_PORT=31324 cargo test -p anzen-link --test trezor -- --ignored
ANZEN_TEST_INPUTS=64 ANZEN_TREZOR_PORT=31324 cargo test -p anzen-link --test trezor full_policy -- --ignored
```

To look at the emulated screen, or press its buttons by hand:

```sh
cargo run -p anzen-link --example trezor-screen -- --port 31324            # what it shows
cargo run -p anzen-link --example trezor-screen -- --port 31324 approve   # or: reject
```

A test that is killed half way leaves the emulator waiting on a dead connection for about a minute. Restart the emulator rather than wait.

## Building for a device

```sh
tools/build.sh device
```

This writes the Safe 7 image and, beside it, what a Trezor needs to accept it: a proof that the app is in a signed set of apps, and that set's signature (a "root packet"), signed here with Trezor's development keys. Only firmware built with those keys accepts it, which means our fork built for development, on a Safe 7 that has been unlocked to run unofficial firmware:

```sh
cd ../trezor-firmware
nix-shell --run "source .venv/bin/activate && xtask build firmware -m t3w1 --apps"
```

The phone app carries the device image inside it and uploads it when it connects (see `mobile/rust/build.rs`), so build the app before building the phone app.

None of this section has been tried on hardware.

## Protocol

`anzen_signer::hardware::stateless` is the reference, for both ends. In short: a request is one message to the app (an id and bytes) and one answer. `INFO` and `IDENTITY` ask who the signer is; `RUN + operation` carries what the phone held for the signer and the operation's request, in postcard, and answers with the signatures and what to hold next. A body longer than 4096 bytes travels in pieces, each acknowledged, because Trezor's link carries about 8.7 kB in one message.
