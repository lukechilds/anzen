# Native app and browser playground

## Decision: device libraries own the application, React Native presents it

Anzen's mobile app uses React Native and Expo. Its browser playground renders the same
screens with React Native Web. The Dark UI keeps separate Wallet and Vault contexts and a
stable Wallet / Vault bottom navigation.

The wallet UI depends only on `anzen-wallet`. The hardware-wallet-mode UI depends only on
`anzen-signer-app`. Neither imports the shared host crates directly; shared public types are
re-exported through the appropriate device crate. The app shell may choose a device role, but it
does not implement either role's rules.

Each role is a separate crate. `anzen-signer-app` wraps the `no_std` `anzen-signer`, which is the
same signing code the Ledger app runs, and has no wallet, chain or network dependency.
`scripts/check-boundaries.sh` enforces this from the resolved Cargo dependency graph.

| Layer | Owns | Must not own |
| --- | --- | --- |
| `anzen-wallet` | Phone keys, wallet synchronization, balances, transaction history, proposals, broadcast, policy execution/revocation, renewal, rotation, recovery, durable operation state | Cold signing or reading another device's private storage |
| `anzen-signer-app` / `anzen-signer` | Independent policy/transaction validation, review information, explicit approval, signing, cold-device recovery and backup operations | Network synchronization, broadcasting, phone wallet logic |
| `anzen-protocol`, `anzen-common`, `anzen-storage` | Shared protocol rules, scripts, transactions, cryptography, formats, reusable persistence mechanisms | UI state or dependencies on device implementations |
| React Native | Navigation, text/amount formatting, input collection, accessibility, animation, progress/error presentation | Computing spending eligibility, signing, policy enforcement, recovery rules or durable wallet state |
| Native adapters | OS-protected storage, file/QR exchange, device permissions, biometric prompts and transport | Deciding what a wallet may sign or spend |
| Playground host | Public chain queries, signed transaction broadcast, encrypted cloud objects, regtest controls | Device command execution, private wallet storage, approvals or signing |

Rust returns typed snapshots, review data, available actions, rejection reasons and progress.
UI input is untrusted: all amounts, destinations, timelocks, packages and approval transitions
must be validated again by Rust. Disabling a button is never an authorization check.
Cryptographic workloads and chain scans run away from the native UI thread.

## One app, two independently stored roles

On first use, choose Wallet or Hardware wallet mode. The latter can run on a second phone;
it must have its own key and storage. Switching modes never copies keys between roles and
must not silently reinitialize, erase, or replace a funded device.

A general-purpose phone used as a signer is not equivalent to a secure-element hardware
wallet. It is a separate signing device, with the security properties of its OS and storage.
Using both roles on the same physical phone does not provide independent compromise domains.
These limitations must be visible in setup, not hidden behind the phrase "hardware wallet".

Setup is one exchange over the Bluetooth link between phone and signer (in-page in the
playground), modelled on Bitkey's simplicity. In one exchange the signer
returns its public identity (a new signer creates its key then), the phone builds the vault from
its key and the signer's public key and sends it with a sealed backup-key request, and the
signer independently reconstructs the expected script/address, checks its own key, accepts the
vault and wraps the phone's recovery-backup key (`setup_vault` / `accept_setup`). Bluetooth
pairing authenticates the channel: on devices the first connection must confirm a code shown
on both screens, as hardware wallets do (the playground, with both devices in one page, skips
it). There is no address comparison or approval prompt: nothing in setup can move funds. A
paired signer refuses a different vault, and a
received package cannot silently change the accepted key, network or vault; changes require
their own review flow. Repeating an interrupted setup with the same signer resends the same
vault.

Signing is two-phase: import and validate a request, then explicitly approve the exact
reviewed request. Merely receiving a message must never sign. Response packages are validated
by the wallet before activation/broadcast. Rejection, retries, interruption, duplicates and
stale requests are first-class states. No approval is inferred from a successful transport.

The versioned message format is independent of transport. Over the link, the phone sends a
request and the signer answers; an answer that signs waits for the user's approval on the
signer, and disconnecting abandons it. The browser playground connects its two visible
app instances this way. Physical devices use the same format through file/QR
exchange; large presigning packages require files or a chunked/animated transport rather
than assuming they fit in one QR. Private backup material requires authenticated encryption;
ordinary pairing/policy messages are not an excuse to expose a seed.

## Real wallet behavior in the browser

The playground runs a wallet instance and a hardware-wallet-mode instance side by side.
Each instance runs the same Rust device APIs as native, compiled to WebAssembly. Each device
has a WASM worker and a sibling I/O worker connected by a private MessageChannel. Rust waits
on a bounded shared-memory mailbox while the I/O worker commits IndexedDB writes or fetches
public chain data; the React thread stays responsive. No HTTP endpoint can execute a device
command. Native still uses its direct bindings, filesystem/SQLite and OS-managed protection.

Browser records use per-device AES-GCM encryption and path-bound authentication. The wrapping
key is a non-exportable WebCrypto CryptoKey persisted in that device's IndexedDB database.
It is usable by same-origin scripts: this is not secure-element or Keychain protection.
Same-device writes are serialized with a Web Lock; failed transactions must not acknowledge
success. Portable BDK ChangeSets replace SQLite only for WASM. The host reconstructs an
in-memory, watch-only wallet from public descriptors/history to perform regtest chain synchronization.
It has no device seeds, device filesystem or signer/application dispatch.

The browser requires cross-origin isolation (COOP/COEP). The supplied web launcher applies
those headers through a loopback proxy. No isolation or storage failure can switch signing
back to the server. See [WASM implementation gates](wasm-implementation.md).

Host-backed links select regtest. A separate control panel can fund a chosen address, mine
confirmation blocks, and advance mock time by a week, 31 days or 13 months (at most 400 days
per step). All transactions and consensus checks use a real Bitcoin Core node. Every vault
timelock is time-based, so each time step also mines eleven blocks at the new time: relative
locks are judged against median time past, which wall time alone does not move.

The local host binds to loopback, restricts origins and authenticates requests with
per-session capabilities. Device-local workers enforce wallet/signer roles; the offline
signer cannot use the chain adapter. Regtest controls
must never be available against a mainnet backend. A remotely hosted playground is a
different trust/deployment model and must not accidentally expose local wallet keys.

Opening the static site without a host capability selects Mutinynet; mainnet requires
explicit consent on each reopening. Mutinynet uses Electrum over secure WebSockets:
`wss://mutinynet.com/electrum-websocket` and `wss://electrum.mutinynet.arkade.sh`.
Rust (`anzen_wallet::electrum`, shared with the native app and CLI) chooses and randomizes
servers, frames JSON-RPC and runs `electrum-client::RawClient` with BDK's Electrum adapter
in the device worker. The I/O worker only opens the WebSocket Rust names, enforces
connection and read timeouts, and passes whole frames. Whole failed queries retry once
against the other server from the original public state; successful fallback is
preferred for that device until reload. Partial scans never update persisted state.
Broadcast failover checks the exact txid and may resend only the same signed bytes;
explicit transaction rejections are not retried. Every new connection checks genesis
and Mutinynet block one before any wallet query or broadcast.

Mainnet uses the same Electrum client over `wss://electrum.lu.ke`. Live updates use a
second, keyless WASM instance in the wallet I/O worker running the shared listener
(`anzen_wallet::watch`); it sees only public script hashes and socket events, has no device
storage bridge, and is never created by a cold signer. No transport needs an Anzen proxy.

The browser workspace vendors the unchanged `bdk_electrum` 0.23.2 Rust sources with
a manifest-only compatibility adjustment: SOCKS support is enabled only off WASM.
Upstream unconditionally enables a Unix/Windows UDP implementation. The patch is
isolated to `mobile/wasm`; native builds retain the crates.io dependency. See
`mobile/wasm/vendor/bdk_electrum/ANZEN.md` for provenance and removal conditions.
Public transport and encrypted-object persistence are adapters, not JavaScript wallet rules.

Static cloud simulation uses a separate IndexedDB database with content-addressed encrypted
objects, independent of device replacement. Object-write acknowledgement requires transaction
commit. Download/import supports keeping an actual external copy; browser-local "cloud" is
not off-device recovery, and clearing site data can destroy it along with the wallet.

Mutinynet stores the explicit name `mutinynet`; Bitcoin's `Network::Signet` supplies its
address encoding. Its genesis and pinned block-one hash identify the deployment. Ordinary
signet is not supported. Its [30-second blocks](https://github.com/MutinyWallet/mutiny-net/blob/master/README.md)
do not shorten any vault timelock: all are time-based, so the signer's ~388-day path is the
same as on mainnet. The public indexer remains trusted for chain data;
the checkpoint check is not full-node validation. The history rate limit is respected with
cross-device request pacing. No real-fund broadcasts are part of automated verification.

Static hosting still needs HTTPS and COOP/COEP headers; no dynamic Anzen backend is required.
The app never discovers/imports this repository's existing mainnet state. Native continues
using Electrum; regtest remains the controllable real-node environment for all time-travel
scenarios. See [static-browser implementation](static-browser-implementation.md).

## Native builds

The iOS and Android apps link Rust locally through thin native bindings; they must not
require the developer's playground server to operate. JavaScript sees public view models
and explicit operations, not private-key objects. Native secret persistence must be
protected by the platform's secure storage and tested across restart/restore. The current
CLI's plaintext simulation files are not a production mobile storage design.

Use Expo development builds for custom Rust/native integration, not an assumption that
stock Expo Go contains those modules. The initial UI can be previewed without native
signing, but a preview is not evidence that the native wallet works.

## Verification

Keep existing Rust/vector/CLI/regtest suites. Add library tests for isolated device stores,
public-only pairing, tampered package rejection, review/approval binding and idempotent
activation. Add architecture tests prohibiting wallet UI/transport imports of `core` and
cross-role calls.

Run browser end-to-end tests through the real two-instance UI and device-local Rust WASM, covering the
same scenarios as `scripts/e2e-demo.sh`. Fixtures are useful for visual states, not a
substitute for exercising real wallet behavior. Also verify native iOS/Android builds,
native Rust invocation, device exchange, persistence, privacy-on-background, biometrics
and lifecycle behavior. Passing browser tests cannot establish those native properties.

## References

- [Expo web support](https://docs.expo.dev/workflow/web/)
- [Adding custom native code to Expo](https://docs.expo.dev/workflow/customizing/)
- [UniFFI native Rust bindings](https://mozilla.github.io/uniffi-rs/) (an alternative adapter)

## Current native adapter implementation

The first adapter uses a small, ownership-documented C ABI around the two Rust application
facades (`mobile/rust`), called by a local Expo Swift/Kotlin module. JavaScript sends JSON
commands and receives view data or opaque protocol packages. The native layer selects
the role, serializes calls and supplies platform capabilities; it never decides fees,
coin selection, policies, recovery maturity, or transaction construction. Its signer arm
does not construct a chain backend. The separate native Cargo workspace keeps embedded
and CLI builds independent of mobile toolchains.

iOS stores a random, per-role storage key in Keychain using
`WhenUnlockedThisDeviceOnly`; Android uses an AndroidKeyStore AES-GCM wrapping key for
the random storage key. Neither adapter passes the storage key to JavaScript. Rust's
synchronous protected-storage scope authenticates/encrypts JSON under the device root
with ChaCha20-Poly1305. Calls fail closed on a missing/wrong key, malformed ciphertext,
or unexpected plaintext. No automatic key replacement is attempted for existing state.

The BDK SQLite cache contains public descriptors/history, not xprvs (covered by a native
test); OS app-private file protection covers that metadata. iOS device state is excluded
from backup and gets complete file protection; Android uses credential-encrypted
`noBackupFilesDir`. This is not a claim of full forensic resistance, hardware-backed
Bitcoin signing, biometric transaction authorization or anti-rollback storage. Rooted
devices and a compromised unlocked app remain outside this protection boundary.

Host tests verify encryption, reopen/address-index persistence, role separation, the C ABI,
real regtest policy approval/activation, exact payment review, and idempotent payment
confirmation. Platform adapters have only syntax/autolinking checks so far: iOS and
Android build/device verification remains a required gate, not an assumed success.

### Native build targets are explicit

`mobile/native-targets.json` is the common build contract for the Rust script, Expo
configuration and native modules. Android currently supports ARM64 devices and x86-64
emulators, with API 26 as the minimum; unsupported 32-bit ABIs must not appear in an APK
without a corresponding Rust library. The Expo plugin sets the generated Gradle properties,
preserves a higher existing Android minimum, and the native module refuses unsupported ABIs.
iOS uses ARM64 device/simulator libraries and a consistent 16.4 deployment target.

The Rust build resolves an existing, executable NDK toolchain before building. It uses an
explicit artifact directory so a global `CARGO_TARGET_DIR` cannot make it package an older
library from somewhere else. Both Android libraries receive 16 KB maximum/common page-size
linker flags. Before copying the Rust libraries into the native module, the script checks
their LOAD alignment and RELRO end with `llvm-readelf`. This follows
[Android's 16 KB guidance](https://developer.android.com/guide/practices/page-sizes);
final APK ZIP alignment, third-party libraries and actual 16 KB device execution still need
verification after the SDK is available. Header-fixture tests are not compiled-device proof.

`npm run check:native-config` generates actual Expo projects in a temporary copy, without
installing dependencies, invoking Gradle/CocoaPods or altering `mobile/ios` and `mobile/android`.
It verifies the generated target settings, disabled Android automatic backup and native-module
discovery. Private Rust state already lives in Android's no-backup directory; backups are
separate, explicitly exported encrypted files. The app also declares the iOS local-network
purpose used when connecting to a regtest node, and includes Expo SystemUI for Android's
requested dark appearance. None of this moves wallet rules out of Rust.

The current Mac has Xcode 27 and an unaccepted SDK license. Expo documents an additional
[SDK 57.0.23+ scene-support requirement](https://docs.expo.dev/versions/latest/sdk/build-properties/#pluginconfigtypeios)
for Xcode 27. The recommended patches are presently excluded by this machine's seven-day npm
release-age policy. That policy was not bypassed: the installable versions and matching lockfile
are retained. Before claiming an iOS device build, resolve the license and that SDK compatibility
gate, then enable the official scene-support plugin and verify the generated/build output.

## Confidential backup exchange

The device-separated backup ceremony never sends the phone seed **or its ciphertext** to
the signer. The phone validates its seed against the vault and encrypts the recovery payload
with a random symmetric key. Only that random key and a commitment to the backup cross the
device channel. After review, the signer wraps the key using its HWW secret and round-trip
checks the wrapping; the phone combines that response with its locally retained payload
to create the existing cloud-envelope format.

This distinction preserves device independence: the signer alone, even with its full
key-wrapping transcript, lacks the encrypted recovery payload. Its review file contains
only public configuration and a request hash, not the request or cloud backup. Approval
requires resupplying the exact request matching that hash. Stale responses cannot replace a
newer phone backup draft. Both UIs receive ciphertext or public review fields, never seeds.
Only the signer can unwrap the backup key; there are no other recipients. A rotation creates
a fresh backup, which the user must export separately after activation.

The channel uses libsecp256k1 ECDH on the existing vault keys normalized to even Y, followed
by the existing HKDF/XChaCha20-Poly1305 implementation. Direction, both identities, vault
address and operation purpose are domain-bound. Pairing still requires an independent
identity/address comparison. This static-key channel is not forward secret and does not
provide third-party-verifiable message signatures; it is for confidential exchange between
the paired devices, not a replacement for Bitcoin signature checks. It must not be reused
to transfer seeds: recorded messages plus a later stolen signer would then compromise both
keys. Key rotation and recovery onto a fresh physical device require additional explicit
protocol steps, including rotating away from any restored/previously disclosed key.

### Restore and rotate a lost phone

The cloud simulation stores opaque encrypted backups outside both device directories. Its
administrator capability is distinct from the wallet and signer capabilities. On native
devices, backup export uses a separate OS share action, never the signer-message channel.

A fresh phone imports the public recovery vault and the cloud envelope. It sends the signer
only the HWW-wrapped encryption key, the backup commitment, and its fresh public identity.
The user compares that identity on both screens and separately approves recovery. The signer
unwraps the encryption key and encrypts it to the replacement phone. It cannot authenticate
the absent cloud payload; the phone authenticates/decrypts it and validates its descriptor.
Neither seed plaintext nor seed ciphertext is sent to the signer. The recovered old key is
encrypted under the replacement phone's seed and stored separately; its fresh key is unchanged.

The signer retains signed policy packages so a lost phone does not erase its spending policy.
The recovery bundle explicitly says **last approved**, not **active**: an offline signer has
no proof that an approval was ever broadcast. Both libraries validate the package and its
signatures. Rotation preserves those reviewed monthly/emergency limits, and the signer shows
the limits again alongside the new key/address and funded withdrawal count.

`anzen-wallet` builds the rotation using the recovered old key and its own new key. `anzen-signer-app`
validates public previous transactions, input values/scripts, duplicates, the new fixed vault
script, the policy-preserving transaction chain, and the phone signatures. The signer receives
no replacement mnemonic, hot-wallet database, or seed ciphertext. Its stored review contains a
request hash and public configuration; approval requires the exact request again.

Approval signs the vault transfer and renewed policy, revokes live old connectors to the new
vault, and wraps the new backup's encryption key. The phone validates the returned package,
saves the approved request and backup before broadcasting, then activates the new vault and
schedule. Receipts make retries idempotent; a lost broadcast response retries the exact signed
transaction. The signer switches its paired vault only after a separate user confirmation;
re-delivering that completion cannot overwrite a subsequently approved policy.

Ordinary hot-wallet funds have a separate exact-amount/fee review and confirmation. The old
key scans both wallet keychains, then sends its spendable balance to a freshly derived address
owned by the replacement phone. Immature coins and later deposits can be recovered in a later
scan. This step does not require the signer. The recovered key is retained encrypted.

### Retaining outgoing protocol messages

The application dispatchers retain successful outgoing proposals, approvals and rotation
completion messages in a device-local Rust outbox before returning them to the UI. The caller
cannot insert arbitrary messages. `outbox` lists message kinds and IDs; `read_outbox` returns
a selected copy. Both are offline operations, exposed through each device facade. Native
records use the same OS-key-backed encrypted storage as the other application records.

Each record binds its version, device identity, network, message kind and body into its ID.
An identical output is deduplicated; changing a body or copying a record to a different device
fails validation. Exported cloud backups are deliberately excluded: the ordinary signer
channel must not receive the encrypted phone-seed payload. Backup ceremony messages contain
the existing key-wrapping protocol only.

Reading or sharing a saved message never signs, adopts a configuration, or broadcasts. An old
proposal still requires the recipient's normal validation and explicit approval; a saved
approval still requires the wallet's normal activation checks. Sharing does not delete the
copy or claim it was received. Saved-message UI belongs in the private vault/signer context,
not the public spending-wallet screen.

This is a durable resend mechanism, not a transaction spanning every file write and broadcast.
A crash between an operation's own persisted result and its outbox write still requires the
operation's existing retry/review path. Delivery acknowledgements, durable incoming reviews,
history/cleanup and recovery of every partial-signing interruption remain work in progress.

### Native file exchange is a platform adapter, not wallet logic

Native devices share JSON **files**, using the OS share sheet, and receive them through an
explicit document picker. This avoids relying on another application's text-message limit
for a presigned policy package. Pasting a protocol message remains an alternative. These
adapters do not call wallet commands: importing only populates an inbox; the user must open
the message, let the appropriate Rust facade validate it, and separately approve any signing.
The consumer still does not import `core` or interpret Bitcoin policy in TypeScript.

`fileExchange.ts` checks the transport envelope, bounds UTF-8 input to 8 MiB and keeps
protocol messages separate from opaque encrypted backups. Rust remains responsible for
cryptographic validation, protocol state, amounts, timelocks and authorization. The reader
uses bounded chunks, checks actual bytes even when provider metadata is missing or wrong,
and closes its handle on success or failure. A structurally accepted file is not an
authenticated message or a verified backup.

`nativeFileExchange.ts` supplies the Expo file-picker, filesystem and share-sheet calls.
It opens selected files read-only. The picker makes a readable cache copy before the JS
size check; the read limit does **not** bound that OS/provider copy's disk usage. Export
filenames contain no wallet identifiers, cannot come from imported JSON, and never overwrite
another export. Only explicit message/backup copies are shared, never the native wallet
storage directory. Broad Documents sharing and iCloud container entitlements are not enabled.

The native share API provides no delivery receipt or reliable cancellation distinction.
Closing its chooser therefore returns `offered`, keeps the outgoing message available, and
does not claim a backup was saved. A retained Rust outbox copy can be resent after reopening
the app. Cache copies are not immediately deleted because another app may read them lazily;
they can remain until cache eviction and are not durable backups. Selected originals are
never modified or deleted. Incoming files are currently in-memory UI state, not a durable
workflow journal.

Backup export/import is a separate wallet-mode action. It must not send the phone-seed
ciphertext to the signer. The user still chooses the recipient/storage
destination, so the OS share sheet cannot enforce that separation: keep the backup in an
independent location, not on the signer or in storage accessible to a stolen signer. Sharing
the same cloud account across both devices can defeat this intended independence.

Pure adapter tests cover framing, cancellation, failed/retried exports, byte limits, short
reads and malformed UTF-8. iOS/Android Hermes bundles and native module discovery are checked
separately. Neither proves a working document provider, share sheet or Rust bridge on a
physical phone; installed-device exchange and interruption checks remain required.

API references: [Expo DocumentPicker](https://docs.expo.dev/versions/latest/sdk/document-picker/),
[FileSystem](https://docs.expo.dev/versions/latest/sdk/filesystem/) and
[Sharing](https://docs.expo.dev/versions/latest/sdk/sharing/).

### A lost signer is restored, not replaced

A vault never changes signer. A lost signer is restored from the backup its hardware-wallet
maker provides, which yields the same key, so the vault, its policy and the phone are
unchanged. Anzen holds no signer seed and has no signer-replacement protocol. The **Lost a
device?** sheet explains this alongside lost-phone recovery and the signer's solo path.

## Offline fallback and controller revocation

The signer can export its public vault configuration to a replacement phone. The phone
stores that as a separate watch-only recovery source, without replacing its own key or vault.
It scans the source and packages input outpoints with their complete previous transactions.
No original phone key or seed backup is needed for this coordination.

The vault is `tr(NUMS,{multi_a(2,phone,hww),and_v(v:older(4259839),pk(hww))})`: both
devices, or the signer alone once the longest BIP68 time lock, 65,535 × 512 seconds (about
388 days), has passed since a coin confirmed. Each annual renewal moves every coin and so
restarts it. The phone has no path of its own.

The hot facade exposes a recovery snapshot for that path: the delay, the median time at which
the oldest coin's lock opens, the seconds remaining and the mature amount. BIP68 counts from
the median time past of the block before each coin's, so Rust reads those times from the
chain backend. The UI displays the remaining days; it does not calculate Bitcoin maturity.
HWW fallback selects mature inputs, or the oldest confirmation cohort before maturity,
bounded to 1,000 inputs with higher-value inputs first. New deposits cannot postpone older
savings, and callers can review another batch for leftovers.

The cold library checks transaction IDs, input values/scripts, duplicates, network and the
accepted vault. It reconstructs the transaction and presents its destination, amount, fee
and action before a separate hash-bound approval. HWW fallback carries the time-based CSV
sequence `0x40FFFF`. The offline signer does **not** claim to know the time or unspent
status; the wallet preflights maturity and Bitcoin ultimately enforces it.

Blanket revocation spends the live connectors in one transaction and returns their change
to the reviewed destination. It does not move the main vault savings. Confirmation must
win against a competing policy action; an unconfirmed revocation is not a guarantee.
The wallet reconstructs the approved transaction, imports only its signatures, verifies
them and broadcasts. A durable submission record allows retrying the exact transaction
after a lost response, without treating an attempted submission as acceptance.

These transfers do not yet implement key rotation or policy renewal. The browser can switch
to a fresh wallet/signer and back to the retained original test device; keys are never copied.
This makes unavailable-device and attacker behavior testable without deleting recovery data.

See the verification ledger for tested boundaries and remaining work.
