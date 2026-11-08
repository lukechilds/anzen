# Mobile app implementation and verification ledger

This tracks the complete requested outcome. Unchecked items are not implemented or not
yet verified; documentation alone does not establish completion.

The verification history below records the earlier host-executed browser implementation.
The current worktree now uses browser-local WASM and the Dark reference. That browser/UI
work is verified separately in [wasm-implementation.md](wasm-implementation.md); the older
results below describe the pre-WASM implementation, not its proof. Native device gates
remain unfinished.

## Architecture and device APIs

- [x] Document Rust-owned business logic, role facades, and UI-only React Native.
- [x] Expose the current consumer API through `hot_wallet` / `cold_wallet`, with boundary tests.
- [x] Public-only pairing with independent phone/signer private storage.
- [ ] Typed snapshots, review/approval, progress, errors and persistent workflows.
- [ ] Wallet operations: setup, sync, receive/send, policy, monthly/soft limit/revoke,
  emergency, refresh, phone/HWW rotation, sweep, backup and recovery.
- [ ] Cold operations: setup, pairing, validated policy/sweep/rotation reviews and signing,
  backups, recovery and controller revocation, without a network client.
- [ ] Tamper/replay/interruption tests and no automatic approval.

## App and native runtime

- [ ] Expo / React Native app matching the Dark reference, stable Wallet / Vault navigation.
- [ ] Selectable Hardware wallet mode with isolated storage and explicit approvals.
- [ ] Native Rust bindings on iOS and Android, running without a playground server.
- [ ] OS-protected secret storage and restart/restore behavior.
- [ ] Exchange protocol packages between two physical devices.
- [ ] Native builds and smoke tests; record exact targets and limitations.

## Browser playground

- [x] Two React Native Web instances using the same screens as the mobile app.
- [x] Browser-local Rust WASM, role-scoped storage and explicit message exchange (replacing host-side devices; current evidence in the WASM ledger).
- [x] Docker regtest, funding, confirmation mining, mock-time advancement (up to 13 months).
- [ ] Observable/cancellable long-running tasks without silent timeouts.
- [x] Real Electrum mainnet backend with explicit opt-in and isolated state; read-only network test.
- [x] Evaluate Mutinynet; time-based timelocks keep its delays identical to mainnet.
- [ ] Optional Mutinynet integration.

## Manual and automated scenario coverage

Every scenario must be reachable manually through the UI; a hidden test-only RPC is not
equivalent. Existing scripts supply the expected behavior, not a license to mock it.

- [x] Setup and policy approval
- [x] Monthly spending and soft limits
- [x] Monthly revocation
- [x] Emergency access and one-week delay
- [x] Emergency cancellation
- [x] HWW revocation
- [x] Partial funding
- [x] Lost phone and restore/rotation
- [x] Stolen phone
- [x] Lost HWW (restored from its maker's backup; explained in the app)
- [x] Stolen HWW
- [ ] Lost phone with no cloud backup (signer alone after the time lock)
- [ ] Both devices lost
- [x] Cloud compromise
- [x] Both devices compromised
- [x] On-time rollover
- [ ] Forgotten rollover (time-based)
- [x] Normal wallet payments and cooperative sweeps

## Final gates

- [x] Existing Rust, vector, CLI and regtest tests pass.
- [x] New role isolation / application API tests pass.
- [ ] Browser interaction, responsive visual and accessibility checks pass.
- [x] Browser end-to-end scenarios pass against real Rust and regtest.
- [ ] Native iOS and Android builds and real Rust smoke tests pass.
- [ ] Build/run/test instructions match commands actually verified.

## Scenario audit

The source of truth for parity is the named tests in `scripts/e2e-demo.sh`, not the number
of browser tests (which also include privacy, responsive layout and transport checks).

Every named CLI scenario has a browser counterpart. The time-based scenarios (missing cloud
backup, both devices lost, forgotten rollover) were rewritten for the one-way timelock and
use **+13 months** instead of mined blocks; they are unchecked above until they pass again.
These results do not establish the remaining native gates. Do not claim that the full
attacker-scenario matrix is complete.

## Environment notes

- 2026-09-16: Apple Xcode tools report an unaccepted license. Asked the user to review it;
  do not accept legal terms on their behalf. Homebrew Git and Docker remain usable.
- No Android SDK/NDK is installed. Build scripts stop with an actionable prerequisite error.
- 2026-09-17 (local): Xcode is version 27.0. Its license is still unaccepted. Expo's
  recommended scene-support patches require SDK 57.0.23+, but npm's configured
  `min-release-age=7` excludes them for now. The attempted update failed with `ETARGET`;
  the compatible existing manifest/lockfile were restored, without overriding the policy.
- No changes committed or pushed. The Docker E2E runner uses its own disposable Compose
  projects; existing mainnet wallet directories are also excluded from build contexts.

## Verification evidence (2026-09-16)

- Root Rust workspace tests passed, including vector/CLI tests; six opt-in real-node tests were
  skipped by that default command. All 18 Docker CLI E2E tests passed.
- Twenty-three `mobile_application` tests pass: independent keys/pairing, malformed inputs,
  mainnet guard, policy review binding, confidential backup ceremony, cooperative sweep
  approval and early recovery rejection. Offline requests also cover previous-transaction
  tampering, duplicate/wrong-path inputs, rejected approvals, a replacement phone without the
  old key, signature validation, and retry after a lost broadcast response. Mature funds are
  not postponed by newer deposits; unsigned reviews, cancellation and exact retry after a lost
  broadcast response are covered by the application fixture.
- Seven native-library tests pass: encrypted persistent keys, fresh address indexes, no xprvs
  in the SQLite cache, wrong-key/tamper rejection, role separation, C ABI allocation/free.
- Native-library real-regtest test passes: encrypted independent devices, confidential backup,
  policy approval and activation, exact amount/fee payment review, cancellation and idempotent
  confirmation. A fresh encrypted phone coordinates a real two-connector HWW revocation with
  public data only. It also restores a cloud backup, rotates the vault while preserving policy,
  and separately reviews/confirms recovery of the old hot-wallet balance. Repeated
  confirmations are idempotent.
- Public mainnet Electrum smoke test passes (tip/network only; no keys or broadcasts).
- Browser interaction tests passed pairing, policy approval/activation, early timelock
  rejection, monthly withdrawal, exact payment review/broadcast, mainnet opt-in guard and a
  narrow viewport. The offline-action browser test passes explicit rejection/approval, early
  HWW fallback rejection and confirmed revocation of monthly/emergency connectors. Browser runs
  now build the current Rust host and create their own disposable Docker chain.
- Twelve browser tests pass, including zero soft limit/over-limit rejection, phone revocation,
  emergency withdrawal/cancellation, partial-funding warning, and on-time non-default policy
  renewal. The replacement-phone revocation test switches back to the original phone to verify
  that both retained policy actions are unusable after confirmation.
- The lost-phone browser test passes cloud export/import, comparing the replacement identity,
  rejected/approved recovery and rotation reviews, reload, preservation of non-default limits,
  new backup export, monthly delay/execution, old hot-wallet transfer review/cancellation/
  confirmation, and rejection of retained actions on the old phone after confirmed rotation.
- Rotation application tests reject changed limits, destinations, previous transactions,
  missing renewal/signatures and mismatched approvals. Lost broadcast responses retry exact
  transactions; completed rotation adoption cannot roll back a later signed policy.
  Disabled-policy rotation is also tested: it keeps both limits at zero and creates no renewed
  presigned withdrawals.
- The cloud transport test passes opaque backup save/load, idempotent storage, role-capability
  denial and invalid object/path rejection. The rotation-review screenshot was inspected; the
  full native visual/accessibility audit remains open.
- The full Docker Rust runner passes on Rust 1.85, including every opt-in RPC/ceremony/recovery
  integration. Its long recovery/rotation test completed in 537.92 seconds on this machine.
- The 12 short browser regressions pass after the recovery-navigation guard. A separate new
  test passes delayed signer inspection: navigating to Wallet keeps the private details hidden,
  while explicitly returning to Vault retains the proposal workflow. The tab's selected state
  is now exposed using the native/web ARIA alias. That test also captures the completed signer
  review rather than its loading indicator. Together with the long recovery test, 14 browser
  scenarios have passed across these runs; this is not the complete CLI scenario matrix.
- The updated root workspace passes with 62 library tests, 20 application tests and the
  existing CLI/vector/architecture/cold-signer tests; six opt-in real-node tests are skipped by
  that default invocation. A targeted vanity-backup regression also validates self-contained
  reconstruction and rejects a wrong index/network or a valid but weakened script/address pair.
  Root and native Clippy pass with warnings denied; native host tests are not device builds.
- TypeScript checking and production web export pass. Swift parses, its podspec has valid Ruby
  syntax, and Expo discovers both platform modules. These are **not** native app builds.
- The updated root workspace passes with 64 library tests and 23 application tests; the native
  library's seven host tests and root Clippy also pass. The application test coordinates an HWW
  fallback from a fresh phone with no original phone key.
- Two new real-node browser cases pass together (41.1 seconds): cloud ciphertext plus public
  vault data cannot authorize unrelated attacker keys, while both compromised devices can
  immediately approve a cooperative sweep with the exact resulting balance in the receiver.
- All three compromise scenarios also pass together (46.4 seconds) with the shared disposable
  host fixture and the 30-second missing-control timeout.

## Verification follow-up (2026-09-17)

- Root workspace tests pass (64 library and 23 application tests, plus existing CLI/vector/
  architecture/cold-signer tests; six opt-in real-node tests are skipped by that invocation).
  Root and native Clippy pass with warnings denied; all seven native host-library tests pass.
- Both encrypted native-facade real-regtest tests pass (810.56 seconds). These remain Rust
  native-interface tests, not installed iOS/Android builds.
- All 18 short browser regressions pass together again (2.8 minutes), before the
  outgoing-message work below.
- Both application dispatchers now retain outgoing protocol messages and expose offline
  list/read operations through their own facades. The new application test verifies restart,
  rejected/failed review without approval, resend/re-review/explicit approval, no implicit
  broadcast, cross-device denial, path rejection and altered-record rejection. Native coverage
  verifies encryption, reopen, deduplication and role/wrong-key rejection.
- The updated root workspace passes (64 library, 24 application, existing
  CLI/vector/architecture and cold-signer tests; six opt-in real-node tests skipped). All eight
  native host-library tests and root/native Clippy pass. Saved-message UI/browser coverage is
  being added; these library results alone do not establish a working UI resume flow.
- Saved-message browser coverage now passes (17.7 seconds): both app instances reload before
  approval, the phone resends its persisted request, the signer rejects/re-reviews/explicitly
  approves, both reload before delivery, and the signer resends its saved approval. The phone
  activates it and executes a real monthly withdrawal. Message history stays off the Wallet
  tab.
- The architecture suite now also checks the native adapter source: the wallet dispatch uses
  only its hot facade, while signer dispatch uses its cold facade and never connects a backend.
  All six architecture tests pass. Native consumers are included in the no-`core`-import check.
- The saved-message browser case passes again with a captured screenshot (18.1 seconds); the
  rendered dark layout was visually inspected. This is not a full accessibility/device audit.
- All 24 browser cases passed together (16.8 minutes), including every named CLI loss/theft,
  policy and renewal scenario, plus privacy and message-resume checks.
- All three native-facade regtest cases pass together (152.77 seconds).
- The new application fixture passes response-loss retry: signed bytes are retained before
  broadcast; altered unsigned contents are rejected; reopening retries the identical
  transaction; a receipt prevents another send. This is fixture state/signing coverage, not a
  consensus test.
- The updated root workspace passes (64 library, 25 application and six architecture tests,
  plus existing CLI/vector/cold-signer tests; six opt-in real-node tests skipped). All eight
  native host-library tests, root/native Clippy, TypeScript checking and web export pass.
- Native file exchange now has 20 passing pure adapter tests: all known message kinds, separate
  wallet-only backup import/export, malformed framing/JSON/UTF-8, exact byte limits, misleading
  file metadata, short reads, cancellation, I/O failure and retry. These test the transport
  adapter with fake file/share I/O, not the native OS dialogs or wallet logic.
- TypeScript checks and production exports for all three platforms pass. iOS and Android
  produce Hermes bytecode bundles; Expo discovers AnzenRuntime, DocumentPicker, FileSystem and
  Sharing on both platforms. This is not an installed native build or Rust bridge test. Latest
  export artifacts: `/tmp/anzen-platform-bundles-final.9pticj`.
- The six Rust architecture tests pass after adding the native file adapter. Apple SDK access
  still explicitly fails on the unaccepted Xcode license; Java is still unavailable. The
  existing upstream Expo/Xcode `uuid` advisory remains in the dependency audit; npm's suggested
  incompatible forced downgrade was not applied.
- Native build settings now share explicit targets across Rust, Expo and the native module:
  Android ARM64/x86-64, API 26+, and iOS ARM64 device/simulator, deployment target 16.4.
  Unsupported Android ABIs fail early; NDK executables and minimum revision are checked; final
  Rust artifacts are built in an explicit directory and inspected for 16 KB LOAD/RELRO
  alignment before packaging. The JNI library receives the matching linker flags.
- All 29 platform-adapter/unit tests pass, including nine new config/toolchain/header-fixture
  cases. They are not actual Android builds. The six Rust architecture tests and eight native
  host-library tests also pass; TypeScript checking and all-platform bundle exports pass.
- Actual Expo native project generation passes in an isolated temporary project. Generated
  Gradle properties, Android backup setting, iOS deployment target and role-module discovery
  are checked. Artifacts:
  `/var/folders/c5/jrqfkkq577g10qp48vg3wd2c0000gn/T/anzen-native-config-vjZANs`. A missing
  Android SystemUI dependency was added; subsequent generation has no dark-mode warning.
- Pairing/policy/withdrawal and narrow-screen browser smoke tests pass after the native config
  changes (17.8 seconds): `/tmp/anzen-native-config-smoke.OitZoJ`. The browser behavior code
  and Rust implementations did not change during the native-target increment.
- Android and iOS build attempts still fail at the deliberately early prerequisites: no NDK,
  and Xcode SDK access exits 69 for its license. Xcode 27 compatibility additionally requires
  the currently release-age-blocked Expo patches. No SDK was installed, no license accepted,
  and no release-age protection disabled. An asynchronous question asks the user how to provide
  the Android toolchain and requests Xcode license review.

## One-way timelock (2026-11-08)

The vault became `tr(NUMS,{multi_a(2,phone,hww),and_v(v:older(4259839),pk(hww))})` (since moved to
a MuSig2 key path, below): both devices, or the signer alone after the time-based BIP68 maximum (about 388 days) since the
last renewal. The phone-only path, recovery contacts, signer replacement and retired-vault
monitoring were removed with it; earlier evidence for those flows was deleted above. Cloud
backups are format 2, wrapped for the signer only. Recovery timing is time-based throughout:
the chain backends supply block median times, the snapshot reports the opening time and
seconds remaining, and the regtest emulator's **+13 months** control replaces recovery mining.

- Root workspace tests, root/native Clippy with warnings denied, the boundary check and the
  42 unit tests pass. Protocol vectors were regenerated and still match rust-bitcoin and
  Miniscript.
- The Docker regtest suite and the browser end-to-end suite have not yet been rerun for this
  change; the browser cases above predate it.

## MuSig2 key path (2026-11-09)

The vault is now `tr(musig(phone,hww),and_v(v:older(4259839),pk(hww)))`. The internal key is the
BIP327 aggregate of both keys, and joint spends carry one key-path signature, about 51 vbytes
cheaper per input (the vault-output vector's rollover fee fell from 291 to 240 sats). Protocol
messages are version 2: requests carry the phone's public nonces, approvals the signer's
`CooperativeSignature`s. The signer signs last with BIP327 `DeterministicSign`; the phone keeps
single-use nonce sessions in `anzen_wallet::cosigning` and replaces their secret nonces with the
finished signatures in one atomic write. Vaults, requests and emulator sessions from before this
change are not compatible.

- Protocol tests check key aggregation against the independent `musig2` crate and verify a full
  two-party signature with rust-bitcoin; sighashes and virtual sizes still match rust-bitcoin.
  Both vectors were regenerated, with the same accepted and rejected policy cases.
- Wallet tests cover finishing, retry reuse and refusal of forged or mismatched halves without
  consuming the session. Root workspace tests, root/native Clippy with warnings denied, the
  boundary check (including the bare-metal signer build), the native library tests and the
  WASM target build pass.
- Bitcoin Core accepts the joint key-path spends: the Docker regtest suite (rollovers,
  allowances, cooperative sweeps and rotations), all 17 CLI end-to-end scenarios, the 29 browser
  end-to-end scenarios, both native regtest scenarios and the static browser suite pass.
- The Ledger and Trezor benchmarks still model the previous script-path signing workload.

The unchecked workflow/native boxes remain intentionally open. In particular, the current
CLI rotation and recovery implementations still assume a combined filesystem in some paths;
the independent app must not call those unchanged or copy keys between device stores as a
workaround. Backup creation now has an independently tested encrypted protocol exchange;
restoration and phone rotation now have their own public-only protocol, explicit reviews,
policy-preserving renewal, new encrypted backup, and separate hot-wallet fund recovery.
A lost signer is restored from its maker's backup as the same key, so it needs no protocol.
Native file exchange is implemented but not physically verified; animated QR,
OS adapter/device verification, durable incoming reviews/full workflow journaling, message
history/cleanup and exhaustive interruption coverage are still unfinished. Outgoing message
resending now has passing Rust, native-library and browser tests.

The latest preview host uses port 8790. Its unused session (zero devices, checked via the API)
was refreshed with the current binary; its files were retained. Older port-8787, port-8788 and
port-8789 sessions were left intact. All use the interactive regtest node. Test runners create separate,
disposable nodes and never reset that interactive chain. No commits or pushes were made.
