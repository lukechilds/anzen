# Anzen mobile and browser playground

Work in progress. The screens are React Native on devices and React Native Web in the
browser. Wallet behavior is in `anzen_wallet::application`; offline approvals are in
`anzen_signer_app::application`. Consumers do not import the shared host crates.
Both device APIs execute inside WebAssembly workers in the browser. Mainnet and Mutinynet
work as a static site: public chain queries/broadcasts go directly to Electrum over WebSockets
(`wss://`), and encrypted cloud backups are simulated in browser storage. Regtest keeps the local
Rust transport and Docker node for funding, mining and time travel. No server executes signing.
The native app uses the same Electrum-over-WebSocket code, and Bitcoin Core RPC on regtest.

Read [the architecture decision](../docs/mobile-architecture.md) and
[the complete implementation checklist](../docs/mobile-implementation.md) before treating
this as a finished mobile wallet. Native device verification, complete interruption handling
and durable protocol delivery remain unfinished. Do not put real savings in this app.
The browser WASM/Dark UI verification is recorded separately in
[the current browser checklist](../docs/wasm-implementation.md).

## Run without a backend (Mutinynet or mainnet)

```sh
cd mobile
npm ci
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.126 --locked
npm run export:web
npm run preview:web
```

Open **http://localhost:8083**. It is one emulator page: on your first visit choose
**Mutinynet** (test bitcoin) or **Mainnet** (real bitcoin, asked once); after that it reopens
the network and devices you used last. Switch networks in the header; each network keeps its
own phone and signer. The wallet is created for you; open **Vault**, choose **Set up vault**,
and the signer joins over Bluetooth. **Start over** makes a fresh pair (not offered on mainnet, so
real-bitcoin keys are never replaced by accident). Both networks run entirely in the browser:
there is no capability token, local Rust service, Bitcoin node or Docker requirement.
`npm run web` on port 8081 also supports this mode when opened without a regtest link.

The whole `dist/` directory is deployable as static files at a site's root. Use HTTPS and
serve `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`; `public/_headers` is copied into the export
for hosts that support that format. Other hosts need equivalent configuration. Serve
`.wasm` as `application/wasm`, and `.mjs` as JavaScript. A plain `file://` page or a host
without isolation headers will not work. `preview:web` is only a local static-file server.

Mainnet uses `wss://electrum.lu.ke`, the only public mainnet Electrum server found that
accepts browser WebSockets. Mutinynet uses:

- `wss://mutinynet.com/electrum-websocket`
- `wss://electrum.mutinynet.arkade.sh`

Each Mutinynet wallet device randomly chooses its preferred Electrum server on opening. Connection
attempts time out after 8 seconds; unanswered requests time out after 15 seconds. A
failed operation restarts once on the other server, which becomes preferred if successful.
If every server fails, the error is shown; no partial wallet sync is committed. Discovery uses
BDK batches of 20 addresses.

Everything above the socket is shared Rust (`anzen_wallet::electrum`): server choice and
failover, JSON-RPC framing, chain checks, sync, scans and broadcasts. The browser's I/O worker
only opens the WebSocket Rust names and passes whole frames; natively, Rust opens it with
`tungstenite` and rustls. Keys never cross the transport. The
indexer sees queried addresses/history and is trusted for its chain view; this is not
full-node/SPV verification. Mutinynet's block-one hash is checked as well as genesis,
because ordinary signet shares that genesis but is a different chain.

Every vault timelock is time-based, so it lasts the same on every network: the signer's solo
path opens about **388 days** after the last renewal, emergency access after one week, and
each monthly allowance after 30 days. Mutinynet's 30-second blocks do not shorten them, and
there are no time-travel buttons here; use regtest to see them expire. First discovery searches both
address chains with a gap of 100 unused addresses; subsequent syncs scan known addresses.
Socket/HTTP adapters never replay broadcasts themselves. Rust reconciles lost broadcast
responses by querying the exact txid on failover, and may resend only the identical signed
transaction if not found. Explicit Electrum transaction rejections are surfaced directly.

### Live updates

A wallet syncs once when it opens and again whenever the chain changes for it. There is no
refresh button: if a background refresh fails, a status line under the balance shows why with
**Retry**, and a regtest wallet left far behind offers **Catch up**. Rust returns the scripts to
watch (`watch_targets`: revealed hot-wallet addresses plus five lookahead addresses per chain,
and the vault and controller addresses once paired). The listener is shared Rust with no I/O
(`anzen_wallet::watch`): the browser's I/O worker and a native thread only carry out its actions
(open, send, close, report a change). Every refresh is the ordinary Rust sync, so a missing or
spurious hint can delay or repeat a refresh but never changes wallet state.

- **Mutinynet and mainnet:** a separate Electrum WebSocket subscribes to new block headers
  and to each watched script hash. If it drops, the wallet refreshes every 30 seconds while
  it reconnects, then refreshes once more to catch anything missed.
- **Regtest:** the I/O worker asks the playground once a second for the tip and Bitcoin
  Core's mempool sequence. Regtest syncs by walking every block since the last sync, so an
  automatic refresh more than 1,000 blocks behind skips syncing and keeps the last synced
  balance (including a never-synced wallet on a long chain) and offers **Catch up**.

The vault refreshes only while it is open, so opening the app still never fetches
savings. The native app runs the same listener on a Rust thread; the app asks it for changes
once a second through `anzen_mobile_watch`, which never waits behind a running command.

Devices, their session IDs and simulated cloud objects survive reload in this browser
profile. Every new or changed recovery backup is saved automatically as an encrypted object in a separate
IndexedDB database, not in either device database. **Download backup…** saves an off-browser
copy; **Import encrypted cloud backup** loads it back into the simulation. Clearing site
data can erase both devices **and** this simulated cloud, so download and keep a separate
copy. Same-origin scripts can access browser keys; the two simulated devices do not provide
independent compromise protection. This remains experimental, not a place for real savings.

## Run the regtest browser playground

From the repository root, start the isolated regtest node and Rust transport:

```sh
docker compose -f compose.playground.yaml up -d --wait
cargo run -p anzen-playground --locked
```

In another terminal:

```sh
cd mobile
npm ci
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.126 --locked
npm run build:wasm
npm run web -- --localhost
```

The secp256k1 dependency also needs a Clang build with the WebAssembly target. On this Mac,
install Homebrew `llvm`; the build script selects `/opt/homebrew/opt/llvm/bin/clang` and
`llvm-ar`. Apple Clang does not include that target. The script selects Command Line Tools
for host build tools when full Xcode has not been configured.

Port 8081 is a loopback proxy in front of Expo on 8082. It adds the COOP/COEP isolation
headers required by the synchronous Rust worker's shared-memory I/O adapter. Do not open
8082 directly. A static deployment must serve `Cross-Origin-Opener-Policy: same-origin`
and `Cross-Origin-Embedder-Policy: require-corp` too; there is no server-signing fallback.

Open the **private localhost link printed by the Rust transport**. It contains a random
capability granting control over this test session; do not share it or put it in logs.
Each host start creates a fresh session directory beneath `.playground-data/` for encrypted
cloud objects. Device records live in separate IndexedDB databases in this browser profile,
not that directory. Existing CLI wallets are never opened. The chain persists while its
Docker container remains alive.

Opened from the host's link, the emulator offers **Regtest** alongside Mutinynet and Mainnet
and starts there. The link records the regtest devices' public IDs, so a refresh reopens the
same pair while the same host is running.
Vault balances stay hidden until you open the Vault tab, and hide again when you leave it or the app. A host restart still creates a new
session; old browser device records are retained, not automatically attached to the new
session or overwritten. Keep an exported recovery backup rather than relying on browser
storage or the playground session as your only copy.

Each device database encrypts records with its own non-exportable WebCrypto key, including
the portable BDK JSON change-set cache (native still uses SQLite). Writes return only after
the IndexedDB transaction commits. A Web Lock prevents two tabs from writing the same
device concurrently. This is **not** hardware or OS-keychain protection: scripts running on
the same origin can use the key, and clearing browser data removes it. The simulated devices
are separate test instances, not independent security boundaries against a compromised origin.

A new phone needs no setup: its wallet is created on first open, and a short welcome explains
that it is protected by Face ID and backed up, encrypted, across the owner's Apple devices.
(That backup is the intended product behaviour; the app does not implement it yet.)

The signer is drawn as a hardware device, a dark pebble with its display at the top and the
Anzen logo beneath, not as a phone. Like a hardware wallet it shows one fact per screen:
**✕** declines, **→** steps through the pages and the last page asks for approval. Anything
that signs is approved by pressing and holding **✓**. The same React Native component will fill a phone's screen when an old phone is
put into hardware signing mode; the playground shows the device on its own.

The two devices are independent. Phone and signer talk over Bluetooth, as with a modern
hardware wallet: when the phone needs its signer it says **Approve on your signer**, and the
signer's screen shows whatever needs checking. The playground's devices are always in range.
Messages cross that link as protocol JSON, never as the other device's private storage, and
each device's Rust validates them as it would a received file.

Setting up a vault is one exchange. Open **Vault** and choose **Set up vault**.
A new signer creates its key on that first connection; in the same exchange the phone builds the
vault, the signer accepts it and protects the phone's encrypted recovery backup, and the
backup is saved to the (simulated) cloud. Nothing in setup can move funds, so the signer
asks nothing. Anything that signs — a policy, a transfer, a recovery — shows its review on
the signer, and the phone waits for **Approve** or **Reject** there. Rejecting reports
**Declined on your signer** on the phone. If the signer approved but the phone could not
finish (for example before a timelock matures), **Cancel** keeps the approval as a **Finish**
card, so retrying never asks the signer to sign again.

### Resume a message exchange

Messages can also travel as files, the fallback for devices without a Bluetooth link. The Rust
applications retain outgoing proposals and approvals. After reopening either device, open
**Saved messages** in Vault settings, or at the bottom of the signer's screen. Choose the message to resend;
the receiving device must still **Open received message** and perform its normal checks. A
proposal still needs explicit signer approval. Resending an already signed approval does not
ask the signer to sign again, and merely opening the saved-message list never broadcasts.

The list survives restarting the app while using the same device storage. It contains old as
well as recent messages, so check the actual policy/destination on the recipient: a retained
message is not proof that its policy is still current. Sharing leaves the saved copy intact.
This does not yet provide delivery acknowledgements or resume every partially completed
operation. Exported cloud backups stay separate from this signer-message channel.
Policy activation is tested across a lost broadcast reply and a reload while the broadcast
is in flight: resending the saved approval resumes the same transaction, including when it
has already confirmed. Reload alone never approves or broadcasts a policy.

### Fund and exercise a policy

After setup the Vault tab opens the vault: **Deposit** shows its address and scannable QR code to fund
with the chain controls. Then **Set monthly allowance** sets the monthly and emergency
amounts in bitcoin, with suggestions sized to the balance. **Continue on signer** and
approve the plain-language summary on the signer; the phone activates it.
Mine a confirmation. The chain controls advance mock time by a week, 31 days or 13 months;
each also mines eleven blocks to move Bitcoin's median-time-past, because changing wall time
alone does not satisfy relative timelocks.

The Vault home follows the Dark mockup: balance, Withdraw/Deposit buttons, and compact
presigned-policy rows. Tap **Monthly allowance** or **Emergency access** for their execution
and revocation controls. **Withdraw** also offers a cooperative transfer with the signer.
**Vault settings** holds the recovery backup, **Lost a device?** guidance and signer
recovery. Every sheet has a close control (Escape also works on the web).

Wallet and Vault have separate receive/deposit addresses; their QR codes encode the actual
address displayed, not a sample. Wallet activity comes from the Rust wallet's transaction
graph. No sample merchants, fiat values, future payments or fabricated transaction history
are shown. A saved policy is not proof that an action remains spendable; Rust checks the
chain and Bitcoin enforces timelocks/revocations at execution.

Available screens include ordinary receive/payments, policy/renewal, monthly execution,
soft limits and revocation, emergency initiation/withdrawal/cancellation and cooperative
vault transfers. The phone alone can never move savings: it is a second factor with a
bounded monthly allowance and emergency access. A fresh phone can **Recover with your
signer** to receive the vault's public details, then request HWW fallback or blanket action
revocation for the signer to review and sign. The fallback opens only about 388 days after
the vault was last renewed; revocation spends both live connectors, not the main vault
balance. Neither transfer rotates keys or renews policy yet.

Use **New phone** / **New signer** beneath the devices to simulate losing one: the new device
takes its place, with fresh keys. The playground keeps the replaced device's state, never
copied to the replacement, so the browser tests can bring it back and check an attack is
rejected. These test controls are absent in mainnet mode.

**+13 months** moves the chain clock past the signer's time lock, as if a year's renewal
had been missed. The named CLI scenarios have browser counterparts. See the verification
ledger for current combined and targeted test results; installed native-app verification
remains unfinished.

### Lost-phone recovery

1. Setup saved an encrypted recovery backup to the cloud, and every change saves it again.
2. Choose **New phone**. In **Vault**, choose **Recover with your signer**,
   then **Share vault details** on the signer. Then **Load latest exported
   backup** from the cloud controls (or select a saved backup after reloading).
3. **Request phone key recovery**. Compare the new phone
   identity on both devices and approve recovery on the signer. The old key is recovered
   separately from the replacement phone's fresh key; this step alone does not move funds.
4. **Propose restored vault rotation** and approve it on the signer. This broadcasts the vault
   transfer, revokes old connectors and renews the same reviewed monthly/emergency limits with
   a fresh backup. Mine a confirmation.
5. **Finish on signer** and select **Use the rotated vault** on it. Export the
   new encrypted cloud backup from the replacement phone.
6. **Review recovered wallet funds** to move the old ordinary hot-wallet balance too. Review
   its exact amount/fee and confirm; mine another confirmation. Switch to the original phone
   to check that its old vault actions can no longer spend the rotated funds.

The signer reports its **last approved** limits; being offline, it cannot know whether that
approval was activated on-chain. The rotation's new backup is saved automatically.
Native devices use the same Rust actions and separate OS backup sharing;
the device builds themselves remain unverified.

### Phone unavailable, with no exported backup

1. Create a fresh wallet phone, choose **Recover with your signer** in Vault, then
   **Share vault details** on the surviving signer. Leave the cloud-backup field empty.
2. **Check recovery timing**. Rust reports how many days remain before the signer can move
   the savings alone, and the amount already eligible.
3. Choose **Use this wallet’s fresh address**, or enter a verified destination, then
   **Request HWW fallback**. Review the destination, amount
   and fee on the signer and **Approve offline action**.
4. The phone rejects an early broadcast; **Cancel** keeps the approval. Use **+13 months**,
   then **Finish** to submit the same approved transaction. Bitcoin accepts it only once the
   chain's median time past is 65,535 × 512 seconds (about 388 days) past each input's
   confirmation. Mine a confirmation and sync the receiving wallet.

This transfers funds, not the lost phone key. HWW fallback needs no cloud backup. A newer
deposit does not postpone older savings: the review uses up to 1,000 mature inputs, or the
oldest confirmation group if none are mature yet. Check again for leftovers or later deposits.

### Test compromise scenarios

The device selectors retain old test devices so their keys and presigned actions can be
exercised after recovery; the replacement never receives their private files.

- **Stolen phone:** use its ordinary wallet to make a payment. The phone has no way to move
  savings alone; reject its cooperative sweep on the legitimate signer. Follow lost-phone
  recovery on a separate new phone, rotate, and recover the remaining ordinary wallet funds.
  Returning to the stolen phone shows that its old policy actions cannot spend the moved funds.
  Already-confirmed stolen hot funds remain with the attacker.
- **Cloud-only attacker:** give unrelated new devices the old encrypted backup and public
  recovery-vault message. The unrelated signer cannot approve restoration or an HWW fallback.
- **Both devices compromised:** both keys can approve an immediate cooperative vault
  transfer. The signer's time lock does not protect against compromise of both devices.

Run these three browser scenarios with `npm run test:e2e -- compromise.spec.ts`.

With **both devices lost**, restore the signer from the backup its maker gave you: it is the
same key, so after the time lock it can move the savings alone. If that backup is lost too,
unrelated new keys cannot decrypt the old exported backup or spend its vault, even after the
time lock opens. To reproduce this, create fresh phone/signer devices, import only the old
public recovery-vault message and encrypted backup, then try phone-key recovery and HWW
fallback before and after **+13 months**. Time never creates a missing private key. The
dedicated case is `npm run test:e2e -- --grep "signer’s own backup"`.

### Forgotten annual renewal

1. Set and approve a policy. Open **Signer recovery and revocation** and **Check recovery
   timing**: the signer's solo path opens in about 388 days.
2. Use **+13 months**. The signer could now move the savings alone; the phone still cannot.
3. With both legitimate devices still available, **Renew for another year**. Review and approve
   the unchanged limits on the signer, activate on the phone, and mine a confirmation.
4. **Check recovery timing** again. The new outputs restart the time lock at about 388 days,
   and an HWW fallback is again refused as not mature.

The dedicated browser case is selected with `npm run test:e2e -- --grep "missed renewal"`.

### Lost-signer recovery

A vault never changes signer. Restore a lost signer from the backup its maker gave you: it
comes back as the same key, so the vault, its policy and the phone are unchanged. **Lost a
device?** in Vault settings explains this. Anzen does not hold or restore signer seeds.

Mainnet never uses the Rust host, even when the page was opened from its link: it runs in the
browser against real Electrum servers, so the host has no mainnet option. Chain controls
exist only on regtest, and the node-control endpoint independently refuses a
non-regtest node. Browser keys stay in the device's browser-local WASM/storage workers.
Browser mainnet mode remains experimental and is not a production wallet.

## Native builds

Native builds link `mobile/rust` directly; they do not call the playground HTTP server.
Expo Go cannot contain this custom Rust library. These build entry points are implemented
but **iOS/Android app builds have not yet been verified on this machine**: Xcode requires
its license to be reviewed, and an Android SDK/NDK has not been installed.

After installing/reviewing the platform tools:

```sh
cd mobile
npm run build:rust:ios
npm run ios
```

Or, with `ANZEN_ANDROID_NDK` pointing to your installed NDK:

```sh
cd mobile
npm run build:rust:android
npm run android
```

The iOS script prepares ARM64 device and Apple Silicon simulator libraries. Android
prepares ARM64 device and x86-64 emulator libraries (Android 8 / API 26 and newer). These
targets are shared in `native-targets.json`; the app is not packaged for unsupported
32-bit Android devices. The Rust build requires an already installed NDK r27 or newer,
adds 16 KB alignment flags and checks the Rust library before packaging. The final
APK and actual 16 KB device behavior still require verification. Select wallet or hardware-wallet
mode on launch. Regtest RPC must be reachable from the physical device; the Docker
node is intentionally bound to host loopback by default. Mainnet connects directly
to Electrum. A second phone is not equivalent to a hardware secure element.

You can check generated native configuration without compiling/installing an app:

```sh
npm run check:native-config
```

This creates a temporary Expo project and prints its path for inspection. It checks Android
ABI/API settings, disabled automatic backup, iOS deployment target (on macOS), and native
module discovery. It does not overwrite the project's native directories, install SDKs or
prove that the Rust bridge runs on a device.

On this Mac, iOS verification has two current prerequisites: accepting Xcode's license, and
using the Expo SDK 57 patches/scene support required by Xcode 27. npm's configured seven-day
release-age policy currently prevents installing those recommended patches. It has not been
disabled; the current dependency set remains installable. Android additionally needs a JDK,
SDK and NDK. Native device support is not yet verified.

Native builds do not have the Bluetooth link yet, so the app falls
back to exchanging JSON files:

1. On the sender, use **Send to signer** or **Send to wallet**. Choose how to transfer the file from the OS share sheet.
2. Save/receive the file on the other device. Choose **Receive a file or message** →
   **Choose protocol message file**. Pasting protocol JSON is also supported; never paste seeds.
3. Return to Vault if it was hidden while switching apps, and explicitly **Open received
   message**. Import alone does not validate, approve, sign or broadcast. Review the Rust
   validation results before approving a signing operation.
4. A closed share sheet does not prove delivery. The outgoing message stays available to
   retry; **Saved messages** can resend retained Rust outbox copies after an app restart.

Natively the recovery backup is offered as a file whenever it changes. Save that file
independently from the signer and verify that the saved copy exists—the app cannot
confirm delivery.
On a replacement wallet-mode phone, use **Receive a file or message** → **Choose encrypted
backup file**, then open the appropriate recovery action in Vault. Signer mode does not offer
backup-file import. Do not store the backup on the signer or in a shared account accessible
to it; the OS chooser cannot prevent an incorrectly selected destination.

Files have an 8 MiB UTF-8 read limit. The OS picker may cache the selected file before that
check; exported copies also remain in app cache to allow the recipient to read them. These
cache copies are not backups. Import never modifies/deletes the original selected file, and
the app does not expose its private wallet directory through file sharing.

Pure adapter tests and native JavaScript/Hermes bundle exports pass. The actual native
picker, share sheet and Rust bridge still need installed-device verification. Incoming files
are not yet persisted across restarts. Animated QR exchange and complete interruption
recovery also remain unfinished.

## Verified checks

The Wallet home shows only the ordinary spending balance and its actual BDK transaction
history. No fiat conversion or merchant labels are fabricated when those sources are absent.

The following run the real Rust implementations, not mocked JavaScript wallet logic:

```sh
# Repository root
cargo test -p anzen-integration-tests --test mobile_application --locked
./scripts/check-boundaries.sh
cargo build -p anzen-playground --locked
node crates/anzen-playground/test.mjs
cargo test --manifest-path mobile/rust/Cargo.toml --locked

# mobile/; Docker must be available (browser tests can start/reuse the Expo server)
npm run typecheck
npm run test:unit
npm run export:web
npm run test:static
npm run test:e2e
npm run test:native:regtest

# Or just the native policy and payment review test
npm run test:native:regtest -- encrypted_native_devices
```

`test:static` serves production assets with no Rust host or Docker. It covers browser-local
pairing/sync, encrypted backup export/recovery after reload, consent and network mismatch,
using a deterministic local Electrum WebSocket fixture. Read-only public server smoke tests are
opt-in: `ANZEN_LIVE_ELECTRUM=1 npm run test:static`. They create empty temporary wallets and
never broadcast. Rust transport and listener tests: `cargo test -p anzen-wallet --lib electrum
watch`, plus `cargo test -p anzen-integration-tests --test electrum_recovery --test electrum_watch`.

`test:unit` checks the thin file courier, browser-worker lifecycle, the WebSocket frame pipe and native-build configuration
without a browser, native device or Bitcoin node; it does not mock wallet business logic or
replace the Rust integration tests. Browser tests separately exercise real IndexedDB/WebCrypto
failures, cross-tab locking, interrupted activation and all wallet scenarios against Rust WASM.

The native and browser regtest runners each create and remove their own Docker project on a
free loopback port; neither reuses nor resets the interactive playground chain. The browser
runner builds the current WASM module and Rust host before testing. Select a test with `npm run test:e2e --
--grep "signer reviews"`.

Playwright needs a Chromium installation (`npx playwright install chromium`). A locally
installed compatible Chromium can be selected with `ANZEN_CHROMIUM_PATH`. Host-side Rust
tests exercise encrypted native state and the C ABI, but do not prove that the Swift/JNI
adapters work on a device.

On this Mac, ordinary host Rust checks currently use
`DEVELOPER_DIR=/Library/Developer/CommandLineTools` until the updated Xcode license is
reviewed. That workaround does not provide the missing iOS SDK build environment.

`npm audit` currently reports an upstream Expo/Xcode build-tool dependency advisory for
`uuid` (GHSA-w5hq-g745-h8pq). npm's suggested forced downgrade is not an appropriate fix;
track the upstream tooling update before release.
