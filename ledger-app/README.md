# Anzen Ledger app

This crate is the Ledger implementation of the Anzen cold signer: a signer that
reviews requests on the device's screen and signs them, and a deterministic
benchmark of the annual vault-policy signing workload.

## Status: proof of concept

Do not use it with real funds. What it does, on Flex and Stax:

- derives the vault key at `m/86'/coin'/100'/0/0`, where the phone app's signer
  mode keeps its key, so the same seed gives the same signer;
- accepts one vault per network and remembers it and the last approved policy;
- reviews and signs yearly policies, transfers of all vault funds, and its own
  recovery and revocation, with BIP327 MuSig2 on the secure element's own
  arithmetic.

Its limits:

- **At most 16 inputs per request.** The device holds a whole request in its
  36 KB of RAM while it reviews and signs. Every vault deposit is an input and a
  policy renewal spends them all, so a vault with more than 16 deposits cannot
  renew here. Streaming requests through the device (folding inputs into running
  hashes, building and signing one transaction at a time) would remove the limit,
  and is deliberately not planned: it would reshape the transaction builder every
  signer shares for the sake of this one device. The other signers (the phone's
  signer mode, Trezor, Passport) have memory to spare.
- **Phone rotation does not fit in memory**, for the same reason.
- **The phone's recovery backup is not implemented** (wrapping its key and
  releasing it to a replacement phone).
- **No phone app support yet.** The host side is `crates/anzen-link`: Bluetooth
  from Rust, and a client for these commands.
- It has run in the Speculos emulator and on a Flex over Bluetooth, driven by
  tests, never yet by the phone app or against a real chain.
- Nano S+, Nano X and Apex have less memory and are not sized for it.

`anzen_signer::hardware` defines the commands. The shared crates carry a few
accommodations for this device's memory, each marked "Hardware signer memory:".

### Emulator tests

`crates/anzen-link/tests/speculos.rs` plays the phone against the app in
Speculos: real requests, taps on the emulated screen, and every signature
completed and verified with the host's own code. In Ledger's dev container, with
ports 9999 and 5001 published, build for Flex and run `tools/speculos.sh`; then:

```console
cargo test -p anzen-link --test speculos -- --ignored --test-threads=1
```

`cargo ledger build flex -- --features debug` prints stack and heap high-water
marks to the emulator's log. Never load a `debug` build on a device: it prints by
semihosting. `--features measure` is safe on a device and answers the same marks
to `cargo run -p anzen-link --example ledger-memory`.

### On a device, without a cable

`cargo run -p anzen-link --example ledger-proxy` connects to a Ledger over
Bluetooth and offers it on TCP port 9998 in the proxy protocol Ledger's tools
speak. With the device unlocked on its dashboard, load the app from the dev
container (the device asks to allow an unsafe manager, then to install):

```console
LEDGER_PROXY_ADDRESS=host.docker.internal LEDGER_PROXY_PORT=9998 \
  python3 -m ledgerblue.runScript --targetId 0x33300004 --apdu --scp \
  --fileName target/flex/release/anzen-ledger.apdu
```

Open Anzen on the device, then run the same tests with a person approving:

```console
ANZEN_LEDGER=127.0.0.1:9998 ANZEN_LEDGER_HUMAN=1 \
  cargo test -p anzen-link --test speculos -- --ignored --test-threads=1 --nocapture
```

A policy is tens of signatures and takes many seconds on the secure element. The
app services the device's events between signatures; without that a real device
resets partway through a large policy, which the emulator never shows.

## Benchmark

The benchmark constructs real
Bitcoin transactions and BIP341 sighashes, derives a reserved key from the
Ledger seed, and creates real BIP340 signatures. All outpoints and amounts
belong to an isolated fake fixture; the benchmark returns only non-secret
commitments to the generated values.

The firmware depends on the platform-independent `anzen-signer` crate (`crates/anzen-signer`) and
does not depend on the Anzen CLI, hot wallet, chain backends, or filesystem
code.

## Build

Ledger's development image contains the required nightly toolchain, device
targets, SDKs, and `cargo-ledger` command. From the repository root, build for
Nano S+ with:

```console
docker run --rm \
  -v "$PWD:/app" \
  -w /app/ledger-app \
  ghcr.io/ledgerhq/ledger-app-builder/ledger-app-dev-tools:latest \
  cargo ledger build nanosplus
```

Other supported targets are `nanox`, `stax`, `flex`, and `apex_p`.

## Interactive Flex viewer

Start the Flex firmware in headless Speculos from the repository root:

```console
docker run --rm -it \
  -p 5001:5001 \
  -p 9999:9999 \
  -v "$PWD:/app" \
  -w /app/ledger-app \
  ghcr.io/ledgerhq/ledger-app-builder/ledger-app-dev-tools:latest \
  speculos --model flex --display headless --api-port 5001 --apdu-port 9999 \
  target/flex/release/anzen-ledger
```

In another terminal, start the dependency-free local viewer and open it in the
default browser:

```console
python3 ledger-app/tools/flex-viewer.py --open
```

The viewer runs at `http://127.0.0.1:5002`. Taps and swipes are translated to
Flex screen coordinates and forwarded to Speculos. Its side control forwards
Speculos's right hardware-button event. Select **Run signing benchmark** on the
Anzen home screen to run the workload locally; use the home-screen quit control
to return to the Ledger dashboard.

For accurate timing, use a physical Flex and run the host benchmark while Anzen
is open:

```sh
ledger-app/tools/run-signing-benchmark.sh --inputs 12
```

The wrapper creates an isolated Python environment and installs the pinned
Ledger USB transport automatically. The host separately measures BIP32 key
derivation, construction of every real BIP341 policy sighash, and repeated
BIP340 signing of one fixed digest. It
reports each phase and their sum on the Flex completion screen. The device does
not verify its own signatures. `--inputs` accepts `1`, `2`, or `12`; rollover
requires one HWW vault signature per vault input. Every later policy action has
one vault input and one unsigned controller input, so the HWW produces one
signature for each action during annual approval. For twelve rollover inputs,
the connector construction benchmarks 15 transactions and 26 signatures.
Speculos can exercise the same protocol for correctness, but its timing is not
representative of physical hardware:

```sh
ledger-app/tools/run-signing-benchmark.sh --speculos 127.0.0.1:9999 --inputs 12
```
