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

Open **http://localhost:8083**. Mutinynet is the default; choose **Dangerously enable
mainnet** for real bitcoin. **Create wallet**, open **Vault**, choose **Set up vault**, then
**Hold signer to phone**. Mainnet consent is required again after reopening the page.
Returning to the plain homepage and selecting mainnet finds the saved pair automatically;
resume it instead of creating replacement keys. Mainnet startup never overwrites that pair.
There is no capability token, local Rust service, Bitcoin node or Docker requirement.
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

Mutinynet targets 30-second blocks: the unchanged 61,200/65,535-block recovery delays
are approximately **21/23 days**, not 14/15 months. It is a test network, not a safe
annual-timelock production configuration. The one-week time-based emergency delay is
unchanged. There are no mining/time-travel buttons here. First discovery searches both
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
  Core's mempool sequence, deferring refreshes until a mining job finishes. Regtest syncs by
  walking every block since the last sync (over a minute after a 61,200-block jump), so an
  automatic refresh more than 1,000 blocks behind skips syncing and keeps the last synced
  balance (including a never-synced wallet on a long chain) and offers **Catch up**.

The vault refreshes only while it is revealed, so opening the app still never fetches
savings. The native app runs the same listener on a Rust thread; the app asks it for changes
once a second through `anzen_mobile_watch`, which never waits behind a running command.

Devices, their session IDs and simulated cloud objects survive reload in this browser
profile. **Export encrypted cloud backup** stores an encrypted object in a separate
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

After creating a device pair, the URL also records their public session IDs. Refresh the
page and choose **Resume playground** to reopen that pair while the same host is running.
Vault balances stay hidden until explicitly revealed. A host restart still creates a new
session; old browser device records are retained, not automatically attached to the new
session or overwritten. Keep an exported recovery backup rather than relying on browser
storage or the playground session as your only copy.

Each device database encrypts records with its own non-exportable WebCrypto key, including
the portable BDK JSON change-set cache (native still uses SQLite). Writes return only after
the IndexedDB transaction commits. A Web Lock prevents two tabs from writing the same
device concurrently. This is **not** hardware or OS-keychain protection: scripts running on
the same origin can use the key, and clearing browser data removes it. The simulated devices
are separate test instances, not independent security boundaries against a compromised origin.

The two screens are independent devices. Phone and signer talk by being held together, as
with a hardware wallet over NFC: when the phone needs its signer it shows **Hold your signer
to your phone**, and the playground's **Hold signer to phone** button (above the signer)
connects them. Messages cross that link as protocol JSON, never as the other device's private
storage, and each device's Rust validates them as it would a received file.

Setting up a vault is one tap. **Create wallet**, open **Vault** and choose **Set up vault**.
A new signer creates its key on that first tap; in the same exchange the phone builds the
vault, the signer accepts it and protects the phone's encrypted recovery backup, and the
backup is saved to the (simulated) cloud. Nothing in setup can move funds, so the signer
asks nothing. Anything that signs — a policy, a transfer, a recovery — shows its review on
the signer, and the phone waits for **Approve** or **Reject** there. Rejecting reports
**Declined on your signer** on the phone. If the signer approved but the phone could not
finish (for example before a timelock matures), **Cancel** keeps the approval as a **Finish**
card, so retrying never asks the signer to sign again.

### Resume a message exchange

Messages can also travel as files, the fallback for devices without a tap link. The Rust
applications retain outgoing proposals and approvals. After reopening either device, open
**Saved messages** in the private Vault tab or on the signer. Choose the message to resend;
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

After setup, reveal the vault: **Deposit** shows its address and scannable QR code to fund
with the regtest controls. Then **Set monthly allowance** sets the monthly and emergency
amounts in bitcoin, with suggestions sized to the balance. **Continue on signer**, hold the
signer to the phone and approve the plain-language summary there; the phone activates it.
Mine a confirmation. Advancing mock time also mines eleven blocks to move Bitcoin's
median-time-past; changing wall time alone does not satisfy relative timelocks.

The Vault home follows the Dark mockup: balance, Withdraw/Deposit buttons, and compact
presigned-policy rows. Tap **Monthly allowance** or **Emergency access** for their execution
and revocation controls. **Withdraw** also offers a cooperative transfer with the signer.
**Vault settings** holds backup export, signer replacement, recovery and retired-vault
monitoring. Every sheet has a close control (Escape also works on the web).

Wallet and Vault have separate receive/deposit addresses; their QR codes encode the actual
address displayed, not a sample. Wallet activity comes from the Rust wallet's transaction
graph. No sample merchants, fiat values, future payments or fabricated transaction history
are shown. A saved policy is not proof that an action remains spendable; Rust checks the
chain and Bitcoin enforces timelocks/revocations at execution.

Available screens include ordinary receive/payments, policy/renewal, monthly execution,
soft limits and revocation, emergency initiation/withdrawal/cancellation, cooperative
vault transfers, and phone-only recovery to an explicit destination. A fresh phone can
**Recover with your signer** to receive the vault's public details, then request HWW fallback
or blanket action revocation for the signer to review and sign. The fallback still requires 65,535 blocks; revocation spends both live connectors,
not the main vault balance. Neither transfer rotates keys or renews policy yet.

Use **New wallet phone** / **New signer device** and the device selectors beneath the frames
to simulate an unavailable device. Each device keeps independent state; original keys are
retained so you can switch back and test whether an attack is rejected, never copied to the
replacement. These test controls are absent in mainnet mode.

Recovery mining runs
as a background job with block-count progress and cancellation at batch boundaries.
Reloading and resuming the same host session reconnects to that job; already mined blocks
are retained if you cancel.
New playground nodes disable Bitcoin Core's `checkblockindex` debugging pass, which otherwise
repeatedly walks the entire block tree during these long regtest runs. This does not disable
transaction, script or timelock validation. All recovery delay blocks are still actually mined.
The named CLI scenarios have browser counterparts. See the verification ledger for current
combined and targeted test results; installed native-app verification remains unfinished.

### Lost-phone recovery

1. Setup already saved an encrypted recovery backup to the cloud. **Export encrypted cloud
   backup** saves the current one again, e.g. after adding a contact.
2. Choose **New wallet phone** and create it. In **Vault**, choose **Recover with your signer**,
   hold the signer to the phone and **Share vault details** on it. Then **Load latest exported
   backup** from the cloud controls (or select a saved backup after reloading).
3. **Request phone key recovery** and hold the signer to the phone. Compare the new phone
   identity on both devices and approve recovery on the signer. The old key is recovered
   separately from the replacement phone's fresh key; this step alone does not move funds.
4. **Propose restored vault rotation** and approve it on the signer. This broadcasts the vault
   transfer, revokes old connectors and renews the same reviewed monthly/emergency limits with
   a fresh backup. Mine a confirmation.
5. **Finish on signer**, hold it to the phone and select **Use the rotated vault**. Export the
   new encrypted cloud backup from the replacement phone.
6. **Review recovered wallet funds** to move the old ordinary hot-wallet balance too. Review
   its exact amount/fee and confirm; mine another confirmation. Switch to the original phone
   to check that its old vault actions can no longer spend the rotated funds.

The signer reports its **last approved** limits; being offline, it cannot know whether that
approval was activated on-chain. Rotation preserves the contacts authenticated from the old
backup. Compare their fingerprints in the rotation review and export the new backup afterwards.
If a recovered key predates contact metadata, repeat phone-key recovery before rotating; the
app refuses to guess that the contact list was empty. Contact-assisted vault sweeping has a
separate path below.
Native devices use the same Rust actions and separate OS backup sharing;
the device builds themselves remain unverified.

### Phone unavailable, with no exported backup

1. Create a fresh wallet phone, choose **Recover with your signer** in Vault, hold the
   surviving signer to it and **Share vault details**. Leave the cloud-backup field empty.
2. **Check recovery timing**. Rust reports the remaining blocks and eligible amount for
   each key. A mature phone path is not useful without the missing phone key.
3. Choose **Use this wallet’s fresh address**, or enter a verified destination, then
   **Request HWW fallback** and hold the signer to the phone. Review the destination, amount
   and fee on the signer and **Approve offline action**.
4. The phone rejects an early broadcast; **Cancel** keeps the approval. Mine the remaining
   recovery blocks in the playground, then **Finish** to submit the same approved transaction.
   The HWW path requires 65,535 blocks from each input's confirmation; advancing dates alone
   cannot unlock it. Mine a confirmation and sync the receiving wallet.

This transfers funds, not the lost phone key. HWW fallback needs no cloud backup. A newer
deposit does not postpone older savings: the review uses up to 1,000 mature inputs, or the
oldest confirmation group if none are mature yet. Check again for leftovers or later deposits.

### Test compromise scenarios

The device selectors retain old test devices so their keys and presigned actions can be
exercised after recovery; the replacement never receives their private files.

- **Stolen phone:** use its ordinary wallet to make a payment. An early phone-only cold
  transfer fails; reject a cooperative sweep on the legitimate signer. Follow lost-phone
  recovery on a separate new phone, rotate, and recover the remaining ordinary wallet funds.
  Returning to the stolen phone shows that its old policy actions cannot spend the moved funds.
  Already-confirmed stolen hot funds remain with the attacker.
- **Cloud-only attacker:** give unrelated new devices the old encrypted backup and public
  recovery-vault message. The unrelated signer cannot approve restoration or an HWW fallback.
- **Both devices compromised:** both keys can approve an immediate cooperative vault
  transfer. The recovery delays do not protect against compromise of both devices.

Run these three browser scenarios with `npm run test:e2e -- compromise.spec.ts`.

With **both devices lost and no enrolled contact**, unrelated new keys cannot decrypt the
old exported backup or spend its vault, even after both recovery delays expire. To reproduce
this, create fresh phone/signer devices, import only the old public recovery-vault message and
encrypted backup, then try phone-key recovery and HWW fallback before and after mining the
displayed HWW delay. Neither delay creates a missing private key. The dedicated case is
`npm run test:e2e -- --grep "both devices lost without"`.

### Forgotten annual renewal

1. Set and approve a policy, then use **Advance 31 days** to pass the annual renewal date.
   Open **Signer recovery and revocation** and **Check recovery timing**. Calendar advancement
   alone does not mature either block-based recovery path.
2. Mine the displayed phone delay. It reaches zero while the signer's later path remains
   locked for another 4,335 blocks. Mine those blocks too: both keys can now recover.
3. With both legitimate devices still available, **Renew current policy**. Review and approve
   the unchanged limits on the signer, activate on the phone, and mine a confirmation.
4. **Check recovery timing** again. The new outputs restart both delays; immediate monthly,
   phone-recovery and HWW-recovery attempts remain locked by their new timelocks.

The dedicated browser case is selected with `npm run test:e2e -- --grep "forgotten rollover"`.

### Lost-signer recovery

1. Keep the surviving wallet phone and choose **New signer device**. Already approved monthly
   withdrawals can still be executed by the phone while the old signer is unavailable.
2. In **Vault settings**, choose **Replace lost signer**, then **Connect new signer** and hold
   the new signer to the phone. The replacement screen shows the new vault address, the
   selected amount versus the total, and the delay for that group. Younger deposits do not
   postpone older savings.
3. **Propose signer replacement** and hold the new signer to the phone. Check both device
   identities and the preserved monthly/emergency limits, then **Approve signer replacement**.
4. The phone rejects early activation without broadcasting anything; **Cancel** keeps the
   approval as a **Finish** card. Use recovery mining to reach the required block height, then
   **Finish**. Changing calendar time alone is insufficient.
5. Mine a confirmation. **Finish on signer**, hold it to the phone and select **Use the
   replacement vault**.
   Export the new encrypted cloud backup. The phone key and ordinary wallet remain unchanged;
   the new vault's policy and recovery delays restart on confirmation.
6. Check **Retired vaults** for funds left at the old address. **Review old vault funds**,
   check the destination/amount/fee, and **Confirm previous vault transfer** once that group
   is mature. Mine confirmation and **Sync vault**; repeat for any younger group. Cancel a
   review before submission with **Cancel previous vault transfer**.

The new signer signs only its new policy, never the old vault's recovery input. Neither its
approval nor the app can bypass Bitcoin's 61,200-block phone delay. Contacts in the phone's
stored backup are retained: compare their fingerprints on the new signer and export the new
backup once replacement is complete.

The old address remains tracked on this phone even when empty, and is rescanned by vault
syncs; there is no background warning service yet. Avoid new payments to it. The retired-address
history is not yet included in cloud recovery onto another phone. Moving these deposits back
to the current vault preserves the limits but does not rewrite its existing presigned policy.

To simulate a stolen signer, keep its test-device entry. During the phone-priority window,
switch to that old signer, share its public recovery vault, and request an HWW fallback from
the phone: it is still too early. Return to the legitimate new signer and complete replacement.
After recovering any younger groups, advance past the old HWW deadlines and check its old
address again; it has no remaining inputs to recover. All approvals remain explicit.

### Enroll a recovery contact

1. In the regtest controls, choose **Add recovery contact device**. On that third phone,
   **Create wallet**, then **Be a recovery contact**. Enter a name and create the contact key.
2. **Share contact public key**, then **Send contact public key**. The owner's phone receives
   only the public certificate and fingerprint; the private OpenPGP key stays on the contact.
3. On the owner's phone, open the message, compare the fingerprint with the contact, and
   **Propose recovery contact** and hold the signer to the phone.
4. Compare the fingerprint on the signer and **Approve recovery backup**, then **Export
   encrypted cloud backup** separately.

The contact's device survives page reload/resume, and recreating a backup preserves its contacts.
Any one configured contact can recover the phone key with the backup; this is not threshold
sharing. Both phone rotation and signer replacement retain contacts and create a fresh backup;
export it after each rotation. Do not rely on this prototype as your only social-recovery route.

### Recover after losing both vault devices

1. Enroll a contact and export the updated backup as above. Choose **New wallet phone** and
   **New signer device** to make both original devices unavailable. **Create wallet** on the
   fresh phone; the replacement signer is not used for this recovery.
2. On the contact, **Share contact public key** and **Send contact public key**. On the fresh
   phone, **Open received message** and load the latest exported cloud backup using the
   playground's cloud controls (or paste it into the recovery screen).
3. **Request contact recovery**, **Send to contact**, then **Open received message** on the
   contact. Independently compare the new phone's identity before **Approve contact recovery**.
   **Send recovery approval**, then open it on the new phone. The old vault address is recovered
   from the backup without either original device or a separate configuration export.
4. **Check recovered vault balance and delay**, then **Use new wallet address** or enter a
   destination. **Review recovered vault transfer** shows the exact amount, fee and inputs.
   **Confirm recovered vault transfer** refuses to sign before the Bitcoin delay has expired.
5. Use recovery mining to reach the required height, retry confirmation, and mine a confirmation.
  Mature vault funds arrive at the destination. Newer deposits may need a later recovery sweep.
6. **Review recovered wallet funds** separately to recover the old ordinary hot-wallet balance.
   It has no vault CSV delay. Check the exact amount, fee and this phone's fresh destination,
   then **Confirm recovered wallet transfer** and mine confirmation. Sync the Wallet tab to
   see the received balance. **Cancel recovered wallet transfer** works before submission;
   it cannot undo a transaction whose submission has started. Repeat for later deposits.

After reload, open **Recover with a contact**, then **Check stored contact recovery**. The
fresh phone's identity stays independent from the restored key. The cloud backup contains the
key and descriptor, not the old spending-policy history: the UI reports that history as
unavailable, and this transfer does not renew the policy. Ordinary hot-wallet funds can be
recovered immediately; contact approval cannot bypass the 61,200-block delay on vault funds.

The legacy host-backed mainnet option (when using a private regtest-style host link) requires
**both** the browser's explicit danger action and starting the Rust host with
`--dangerously-enable-mainnet`. It uses real Electrum servers and
new isolated keys. Mining/funding controls are hidden for mainnet; the node-control
endpoint itself independently refuses a non-regtest node. Browser keys stay in the device's
browser-local WASM/storage workers; only public chain information or signed transactions
cross the chain proxy. Browser mainnet mode remains experimental and is not a production wallet.

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

Native builds do not have a tap link yet (NFC or Bluetooth is the next step), so the app falls
back to exchanging JSON files:

1. On the sender, use **Send to signer**, **Send to wallet**, or the corresponding contact
   action. Choose how to transfer the file from the OS share sheet.
2. Save/receive the file on the other device. Choose **Receive a file or message** →
   **Choose protocol message file**. Pasting protocol JSON is also supported; never paste seeds.
3. Return to Vault if it was hidden while switching apps, and explicitly **Open received
   message**. Import alone does not validate, approve, sign or broadcast. Review the Rust
   validation results before approving a signing operation.
4. A closed share sheet does not prove delivery. The outgoing message stays available to
   retry; **Saved messages** can resend retained Rust outbox copies after an app restart.

**Export encrypted cloud backup** is a separate action. Save that file independently from
the signer/contact and verify that the saved copy exists—the app cannot confirm delivery.
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

Wallet **Settings** contains **Be a recovery contact** and **Recover with a contact**.
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

# Or just the independent contact/ordinary-wallet recovery native test
npm run test:native:regtest -- contact_recovers_hot_funds
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
