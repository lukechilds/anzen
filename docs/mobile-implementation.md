# Mobile app implementation and verification ledger

This tracks the complete requested outcome. Unchecked items are not implemented or not
yet verified; documentation and a rendered mockup alone do not establish completion.
Existing mockup edits are preserved. Do not push while this work is in progress.

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
- [x] Docker regtest, funding, confirmation mining, mock-time advancement, recovery mining.
- [ ] Observable/cancellable long-running tasks without silent timeouts.
- [x] Real Electrum mainnet backend with explicit opt-in and isolated state; read-only network test.
- [x] Evaluate Mutinynet; defer enabling it until custom signet identity and delay displays exist.
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
- [x] Lost phone and restore/rotation (including recovery contacts)
- [x] Stolen phone
- [x] Lost HWW and phone recovery/new signer (including recovery contacts)
- [x] Stolen HWW
- [x] Lost phone with no cloud backup
- [x] Both devices lost
- [x] Cloud compromise
- [x] Social recovery (contact approval and delayed vault sweep)
- [x] Both devices compromised
- [x] On-time rollover
- [x] Forgotten rollover
- [x] Normal wallet payments and cooperative sweeps

## Final gates

- [x] Existing Rust, vector, CLI and regtest tests pass.
- [x] New role isolation / application API tests pass.
- [ ] Browser interaction, responsive visual and accessibility checks pass.
- [x] Browser end-to-end scenarios pass against real Rust and regtest (25-case combined run below).
- [ ] Native iOS and Android builds and real Rust smoke tests pass.
- [ ] Build/run/test instructions match commands actually verified.

## Scenario audit

The source of truth for parity is the named tests in `scripts/e2e-demo.sh`, not the number
of browser tests (which also include privacy, responsive layout and transport checks).

Every named CLI scenario now has a passing browser counterpart. All 25 browser tests passed
together against real Rust and regtest (17.0 minutes), including ordinary-wallet contact recovery
and file-courier-related UI changes. Subsequent native build configuration changes also have a
passing targeted browser smoke run. These results do not establish the remaining native gates.

Signer replacement now selects mature groups and retains old vaults for later recovery. The
application fixture covers selection, restart, cancellation, tampering, changed destinations,
early/spent-input rejection and lost-response retries. The native and browser real-node cases
also pass with younger deposits left behind and subsequently recovered.
Retired-address monitoring currently runs on explicit vault syncs and its history is local to
the surviving phone; backup/migration of that history remains unfinished. Do not claim that the
full attacker-scenario matrix is complete.

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

- Root Rust workspace tests passed, including vector/CLI tests; six opt-in real-node tests
  were skipped by that default command. All 18 Docker CLI E2E tests passed. That binary
  predates the final backup-key-wrap refactor; the updated Rust suite passes, and the real
  social-recovery CLI scenario also passes against a fresh build (61,199 real delay blocks).
- Twenty-three `mobile_application` tests pass: independent keys/pairing, malformed inputs,
  mainnet guard, policy review binding, confidential backup ceremony, cooperative sweep approval
  and early recovery rejection. Offline requests also cover previous-transaction tampering,
  duplicate/wrong-path inputs, rejected approvals, a replacement phone without the old key,
  signature validation, and retry after a lost broadcast response.
  Signer replacement additionally covers an unavailable old HWW, altered CSV/previous
  transactions/limits, rejected and mismatched approvals, the one-block-early boundary,
  mixed-age and already-spent inputs, policy preservation (including disabled spending),
  and replay-safe signer adoption.
  Recovery-contact enrollment verifies independent private storage, validated public fingerprints,
  explicit rejection/approval, duplicate refusal, unchanged active backups before approval,
  preservation across backup recreation, and adding a second contact without dropping the first.
  Contact restore additionally removes both original private-key files before recovery, uses no
  separate public-config import, rejects altered requests/responses/cloud payloads and stale
  approvals, preserves the new phone identity, and checks the one-block-early sweep boundary.
  Mature funds are not postponed by newer deposits; unsigned reviews, cancellation and exact
  retry after a lost broadcast response are covered by the application fixture.
- Seven native-library tests pass: encrypted persistent keys, fresh address indexes, no xprvs
  in the SQLite cache, wrong-key/tamper rejection, role separation, C ABI allocation/free.
  The contact test additionally verifies encrypted private OpenPGP persistence, public-only
  responses, reopen stability, and refusal to overwrite an existing contact key.
  The new restore test drops both original device directories, performs the contact protocol
  through the native entry point, and verifies encrypted recovered-key/state persistence,
  unchanged replacement identity, reopen stability and wrong-storage-key rejection.
- Native-library real-regtest test passes: encrypted independent devices, confidential backup, policy approval
  and activation, exact amount/fee payment review, cancellation and idempotent confirmation.
  A fresh encrypted phone coordinates a real two-connector HWW revocation with public data only.
  It also restores a cloud backup, rotates the vault while preserving policy, and separately
  reviews/confirms recovery of the old hot-wallet balance. Repeated confirmations are idempotent.
- The second native real-regtest test passes after mining the full phone-recovery delay:
  the old signer key is deleted, early and one-block-early activations fail, replacement
  preserves the phone key and limits, and the new signer can restore the new backup onto
  another fresh phone. Both native regtest tests passed serially in 689.56 seconds.
- Public mainnet Electrum smoke test passes (tip/network only; no keys or broadcasts).
- Rust transport/regtest test passes monthly execution/revocation, emergency delay/withdrawal/
  cancellation, payment, policy-preserving refresh, cooperative sweep and mining-job cancellation.
- Browser interaction tests passed pairing, policy approval/activation, early timelock rejection,
  monthly withdrawal, exact payment review/broadcast, mainnet opt-in guard and a narrow viewport.
  Mining-job completion/cancellation and a delayed-response vault privacy race also pass.
  The offline-action browser test passes explicit rejection/approval, early HWW fallback
  rejection and confirmed revocation of monthly/emergency connectors. Browser runs now build
  the current Rust host and create their own disposable Docker chain.
- Twelve browser tests pass, including zero soft limit/over-limit rejection, phone revocation,
  emergency withdrawal/cancellation, partial-funding warning, and on-time non-default policy
  renewal. The replacement-phone revocation test switches back to the original phone to
  verify that both retained policy actions are unusable after confirmation.
- The lost-phone browser test passes cloud export/import, comparing the replacement identity,
  rejected/approved recovery and rotation reviews, reload, preservation of non-default limits,
  new backup export, monthly delay/execution, old hot-wallet transfer review/cancellation/
  confirmation, and rejection of retained actions on the old phone after confirmed rotation.
- The lost-signer browser test passes against real regtest (10.5 minutes): execute an
  allowance without the old HWW, reject/approve a replacement proposal, reject early and
  one-block-early activation, replace at maturity, preserve non-default limits, export the
  new backup, execute the renewed policy, and reject the old signer's access to moved funds.
- Rotation application tests reject changed limits, destinations, previous transactions,
  missing renewal/signatures and mismatched approvals. Lost broadcast responses retry exact
  transactions; completed rotation adoption cannot roll back a later signed policy.
  Disabled-policy rotation is also tested: it keeps both limits at zero and creates no
  renewed presigned withdrawals.
- The cloud transport test passes opaque backup save/load, idempotent storage, role-capability
  denial and invalid object/path rejection. The rotation-review screenshot was inspected;
  the full native visual/accessibility audit remains open.
- The full Docker Rust runner passes on Rust 1.85, including every opt-in RPC/ceremony/recovery
  integration. Its long recovery/rotation test completed in 537.92 seconds on this machine.
- Mining-job reload/reconnection/cancellation, message sharing, and funded narrow-screen
  balance rendering pass targeted browser checks. Small-label contrast was increased; this is
  not a substitute for native screen-reader, dynamic-type or app-switcher privacy verification.
- The 12 short browser regressions pass after the recovery-navigation guard. A separate new
  test passes delayed signer inspection: navigating to Wallet keeps the private details hidden,
  while explicitly returning to Vault retains the proposal workflow. The tab's selected state
  is now exposed using the native/web ARIA alias. That test also captures the completed signer
  review rather than its loading indicator. Together with the long recovery test, 14 browser
  scenarios have passed across these runs; this is not the complete CLI scenario matrix.
- A fifteenth browser scenario passes recovery-contact enrollment on an optional third,
  independently stored phone. It checks fingerprint review, rejection, explicit approval,
  cloud export, reload of all three devices, and preservation on backup recreation. That
  enrollment-only test does not itself establish contact-assisted restore; the later dedicated
  recovery scenario below verifies that path.
- All 14 short browser scenarios pass together after the contact-device changes (1.7 minutes).
  The separately verified long signer-recovery test was not rerun in this enrollment increment.
- The updated root workspace passes with 62 library tests, 20 application tests and the
  existing CLI/vector/architecture/cold-signer tests; six opt-in real-node tests are skipped by
  that default invocation. A targeted vanity-backup regression also validates self-contained
  reconstruction and rejects a wrong index/network or a valid but weakened script/address pair.
  Root and native Clippy pass with warnings denied; native host tests are not device builds.
- The contact-recovery browser scenario passes against a fresh real regtest node: both original
  vault devices become unavailable, a third contact rejects/approves release to a fresh phone,
  no configuration is imported, and recovered state survives reload. Review cancellation,
  early and one-block-early rejection, the full 61,200-block delay, confirmed vault sweep and
  the exact received balance in the new phone's BDK wallet are all exercised through the UI.
  This is a sixteenth browser scenario, not completion of the remaining CLI scenario matrix.
- All 14 short browser regressions pass together after the contact-restore changes (1.8 minutes).
  The long contact-recovery scenario passed separately; the older long signer-recovery test was
  not rerun in this increment. TypeScript checking and production web export also pass.
- Contact-preserving rotation adds two passing application cases (22 total). They decrypt the
  newly rotated backup with the retained contact, verify its new descriptor/device keys, reject
  altered encrypted backup requests, and ensure an unapproved cloud object cannot remove
  previously authenticated contacts. A legacy recovered-key test brings the library total to
  63: old records can still recover funds, but cannot assume an empty contact list for rotation.
- Phone rotation passes through the browser both with and without a contact. The updated
  lost-signer browser test passes with an enrolled contact after the full real recovery delay
  (11.8 minutes); all three selected cases pass together in 12.8 minutes. Both contact-bearing
  cases also decrypt the new exported backup on a fresh phone using the independent contact.
- Both native real-regtest tests pass with contacts enrolled (717.25 seconds): immediate
  restored-phone rotation and delayed signer replacement retain their contacts. Each newly
  exported backup decrypts through the contact protocol on an independent fresh phone;
  replacement also verifies recovery through the new HWW. These run the native Rust entry
  point with encrypted storage, not an installed iOS/Android app.
- All 15 short browser regressions pass together after contact-preserving rotation (2.1 minutes).
  The long signer-replacement case passed separately in this increment; the previously verified
  long social-recovery sweep was not rerun. Root/native Clippy, TypeScript checking and web
  export pass. The Xcode SDK still explicitly refuses use until its license is accepted;
  reporting `xcodebuild -version` successfully is not evidence that SDK builds are available.
- TypeScript checking and production web export pass. Swift parses, its podspec has valid
  Ruby syntax, and Expo discovers both platform modules. These are **not** native app builds.
- The updated root workspace passes with 64 library tests and 23 application tests; the
  native library's seven host tests and root Clippy also pass. Recovery timing and bounded
  input selection now cover mixed-age deposits, a 1,000-input limit, deterministic ordering,
  one-block-early maturity, empty vaults and checked amount overflow. The application test
  coordinates an HWW fallback from a fresh phone with no original phone key.
- Two new real-node browser cases pass together (41.1 seconds): cloud ciphertext plus public
  vault data cannot authorize unrelated attacker keys, while both compromised devices can
  immediately approve a cooperative sweep with the exact resulting balance in the receiver.
- The stolen-phone browser case passes (31.1 seconds): a real hot-wallet theft succeeds,
  early phone-only cold recovery fails, the honest HWW rejects the attacker sweep, and a
  separate owner phone restores and rotates with unchanged limits. The owner also recovers
  the remaining hot funds. Old policy actions fail after rotation; the attacker still holds
  the funds already stolen. Tests initially exposed a review-read race in the new test;
  it now waits for the completed signer review before comparing recipient identities.
- All three compromise scenarios also pass together (46.4 seconds) with the shared disposable
  host fixture and the 30-second missing-control timeout. Long mining and sync assertions retain
  their own explicit budgets. The HWW-delay test's first full-delay attempt reached a successful
  broadcast but then waited for an already-consumed resend control; it was interrupted and its
  test step corrected. That interrupted attempt is not a completed recovery-test pass.
- The corrected missing-backup HWW recovery test passes end to end (11.2 minutes): a fresh
  phone has no exported backup or old key, the HWW reviews/rejects/approves the public request,
  both early and one-block-early broadcasts fail, and the full 65,535-block CSV deadline is
  enforced by a real node. Confirmation empties the source vault and the new phone's BDK wallet
  receives exactly the reviewed amount. The earlier phone deadline does not supply a missing key.
- All 18 short browser scenarios pass together after these changes (2.9 minutes), including
  the three compromise cases, both restored-phone rotation variants, policy execution,
  privacy and cancellable mining. The new long HWW fallback test passed separately; the older
  long signer-replacement and social-sweep tests were not rerun in this increment. TypeScript,
  production web export and formatting checks pass. No changes have been committed or pushed.

## Verification follow-up (2026-09-17)

- Signer replacement now selects mature/oldest groups and retains the old configuration before
  any submission. The extended application case verifies separately timed deposits, restart,
  unsigned reviews, cancellation, destination/record tampering, one-block-early and spent-input
  rejection, exact lost-response retries, and continued tracking after an address is emptied.
- Root workspace tests pass (64 library and 23 application tests, plus existing CLI/vector/
  architecture/cold-signer tests; six opt-in real-node tests are skipped by that invocation).
  Root and native Clippy pass with warnings denied; all seven native host-library tests pass.
- Contact-assisted browser recovery passes again after sharing the review/submission engine
  with retired-vault recovery (11.2 minutes, including the real 61,200-block delay and exact
  destination balance). The paired run's signer case failed on a test assumption: its expected
  leftovers omitted the younger continuing monthly-chain output. That test is corrected and
  awaiting its new full-delay result; the failed run is not a signer-scenario pass.
- Both encrypted native-facade real-regtest tests pass (810.56 seconds). The delayed case now
  includes a later deposit, verifies it cannot postpone older savings, checks the protected
  retired-address record, rejects early/one-block-early recovery of the later deposit, moves it
  to the current vault at maturity, and checks idempotent confirmation and zero old balance.
  These remain Rust native-interface tests, not installed iOS/Android builds.
- The corrected browser signer-replacement case passes (14.6 minutes). The retained old HWW
  is rejected during the phone-priority window, older savings rotate with preserved limits
  and contact, the continuing allowance output and later deposit are recovered as separate
  groups, and retired-address tracking survives reload. The test then passes the old HWW's
  actual latest deadline and confirms that it has no old vault inputs left to recover.
- All 18 short browser regressions pass together again (2.8 minutes), before the outgoing-message
  work below. The two long expired-vault scenarios are still running; neither is yet recorded
  as passing.
- Both application dispatchers now retain outgoing protocol messages and expose offline
  list/read operations through their own facades. The new application test verifies restart,
  rejected/failed review without approval, resend/re-review/explicit approval, no implicit
  broadcast, cross-device denial, path rejection and altered-record rejection. Native coverage
  verifies encryption, reopen, deduplication and role/wrong-key rejection.
- The updated root workspace passes (64 library, 24 application, existing CLI/vector/architecture
  and cold-signer tests; six opt-in real-node tests skipped). All eight native host-library
  tests and root/native Clippy pass. Saved-message UI/browser coverage is being added; these
  library results alone do not establish a working UI resume flow.
- Saved-message browser coverage now passes (17.7 seconds): both app instances reload before
  approval, the phone resends its persisted request, the signer rejects/re-reviews/explicitly
  approves, both reload before delivery, and the signer resends its saved approval. The phone
  activates it and executes a real monthly withdrawal. Message history stays off the Wallet tab.
- The first forgotten-rollover run hit its actual 20-minute mining assertion timeout. Its next
  both-lost case was explicitly interrupted, not counted as passing. Core 28.1's regtest-only
  default block-index debug traversal was causing height-dependent overhead. The playground
  now disables that debug pass without changing consensus/script/timelock validation. A fresh
  test node mined all 65,535 blocks; after its test faucet was unloaded, 10,000-block batches
  took roughly 3–7 seconds. No interactive node was restarted or reset.
- Forgotten rollover now passes (1.7 minutes): both recovery paths mature on a real chain;
  late cooperative renewal preserves limits and resets them to 61,199/65,534 blocks remaining;
  new monthly, phone-only and HWW-only attempts are rejected before their respective locks.
- Both devices lost without a contact passes (1.6 minutes): unrelated replacement keys cannot
  recover the phone backup or spend the old vault, including after both full CSV deadlines.
  The saved-message and two expiry cases pass together (3.7 minutes).
- Both updated encrypted native-facade regtest cases pass (121.16 seconds), including policy,
  payments, backup/rotation/contact preservation and mature-cohort signer replacement. This
  verifies the Rust entry point with real Bitcoin Core, not an installed phone app.
- The architecture suite now also checks the native adapter source: the wallet dispatch uses
  only its hot facade, while signer dispatch uses its cold facade and never connects a backend.
  All six architecture tests pass. Native consumers are included in the no-`core`-import check.
- The saved-message browser case passes again with a captured screenshot (18.1 seconds); the
  rendered dark layout was visually inspected. This is not a full accessibility/device audit.
- All 24 browser cases passed together (16.8 minutes), including every named CLI loss/theft,
  policy and renewal scenario, plus privacy and message-resume checks. This precedes the small
  ordinary-wallet contact-recovery increment below.
- Contact recovery now exposes immediate recovery of the old ordinary wallet separately from
  the delayed vault sweep. Its new browser case passes (23.1 seconds), covering reload, exact
  amount/fee, review cancellation, confirmed receipt, later deposits, unchanged fresh identity,
  and an untouched/locked vault. The replacement signer is never initialized or used.
- All three native-facade regtest cases pass together (152.77 seconds). The new case physically
  drops both original temporary device stores before contact recovery, recovers ordinary funds
  and later deposits without the vault delay, checks encrypted signed-transaction storage,
  rejects cancellation after submission, and verifies exact amounts and idempotent confirmation.
- The new application fixture passes response-loss retry: signed bytes are retained before
  broadcast; altered unsigned contents are rejected; reopening retries the identical transaction;
  a receipt prevents another send. This is fixture state/signing coverage, not a consensus test.
- The contact ordinary-wallet native case also passes after deliberately removing its local
  broadcast receipt once Core has confirmed the payment (13.73 seconds). The retained signed
  transaction retries successfully against the real node, and cancellation remains refused.
  The native regtest runner now accepts a Cargo test-name filter for individual scenarios.
- The updated root workspace passes (64 library, 25 application and six architecture tests,
  plus existing CLI/vector/cold-signer tests; six opt-in real-node tests skipped). All eight
  native host-library tests, root/native Clippy, TypeScript checking and web export pass.
- All 20 short browser regressions pass together after the ordinary-wallet recovery change
  (3.5 minutes), including contact recovery, saved-message resume, both phone-rotation variants,
  compromise cases, policy actions and privacy checks. The five long cases passed in the preceding
  24-case combined run; that is not a claim of a new 25-case combined run.
- Native file exchange now has 20 passing pure adapter tests: all known message kinds,
  separate wallet-only backup import/export, malformed framing/JSON/UTF-8, exact byte limits,
  misleading file metadata, short reads, cancellation, I/O failure and retry. These test
  the transport adapter with fake file/share I/O, not the native OS dialogs or wallet logic.
- TypeScript checks and production exports for all three platforms pass. iOS and Android
  produce Hermes bytecode bundles; Expo discovers AnzenRuntime, DocumentPicker, FileSystem
  and Sharing on both platforms. This is not an installed native build or Rust bridge test.
  Latest export artifacts: `/tmp/anzen-platform-bundles-final.9pticj`.
- The six Rust architecture tests pass after adding the native file adapter. Apple SDK
  access still explicitly fails on the unaccepted Xcode license; Java is still unavailable.
  The existing upstream Expo/Xcode `uuid` advisory remains in the dependency audit; npm's
  suggested incompatible forced downgrade was not applied.
- The first short browser run after the file adapter passed 19 cases and failed the
  contact-bearing phone-rotation case: its next cloud-load click never issued a request
  while the preceding contact-inspection screen was still updating. The helper now waits
  for the completed contact view and checks the exact loaded backup response in the input.
  The affected recovery/rotation case passed three consecutive runs (1.5 minutes total)
  after the screen wait was added. No timeout was extended and no wallet step was bypassed.
- The final 20 short browser regressions pass together (3.2 minutes), including the exact
  backup-response check, contact-bearing rotation, saved messages, payment/recovery and privacy.
  Results: `/tmp/anzen-file-exchange-final.eFWwhr`. The disposable node was removed by the
  runner; the interactive node was left intact. The five full-delay cases were not rerun in
  this file-adapter increment. This is not a new combined 25-case pass or a native-device pass.
- All 25 browser tests subsequently passed together (17.0 minutes), including all five
  full-delay cases. Results: `/tmp/anzen-complete-browser.jzOXJL`. Its disposable Docker
  node was cleaned up normally. No interactive node or mainnet state was reset.
- Native build settings now share explicit targets across Rust, Expo and the native module:
  Android ARM64/x86-64, API 26+, and iOS ARM64 device/simulator, deployment target 16.4.
  Unsupported Android ABIs fail early; NDK executables and minimum revision are checked;
  final Rust artifacts are built in an explicit directory and inspected for 16 KB LOAD/
  RELRO alignment before packaging. The JNI library receives the matching linker flags.
- All 29 platform-adapter/unit tests pass, including nine new config/toolchain/header-fixture
  cases. They are not actual Android builds. The six Rust architecture tests and eight
  native host-library tests also pass; TypeScript checking and all-platform bundle exports pass.
- Actual Expo native project generation passes in an isolated temporary project. Generated
  Gradle properties, Android backup setting, iOS deployment target and role-module discovery
  are checked. Artifacts: `/var/folders/c5/jrqfkkq577g10qp48vg3wd2c0000gn/T/anzen-native-config-vjZANs`.
  A missing Android SystemUI dependency was added; subsequent generation has no dark-mode warning.
- Pairing/policy/withdrawal and narrow-screen browser smoke tests pass after the native config
  changes (17.8 seconds): `/tmp/anzen-native-config-smoke.OitZoJ`. The browser behavior code and
  Rust implementations did not change during the native-target increment.
- Android and iOS build attempts still fail at the deliberately early prerequisites: no NDK,
  and Xcode SDK access exits 69 for its license. Xcode 27 compatibility additionally requires
  the currently release-age-blocked Expo patches. No SDK was installed, no license accepted,
  and no release-age protection disabled. An asynchronous question asks the user how to provide
  the Android toolchain and requests Xcode license review.

The unchecked workflow/native boxes remain intentionally open. In particular, the current
CLI rotation and recovery implementations still assume a combined filesystem in some paths;
the independent app must not call those unchanged or copy keys between device stores as a
workaround. Backup creation now has an independently tested encrypted protocol exchange;
restoration and phone rotation now have their own public-only protocol, explicit reviews,
policy-preserving renewal, new encrypted backup, and separate hot-wallet fund recovery.
Signer replacement now has an independent protocol path with passing native real-node and
browser recovery tests. The named CLI scenario matrix now has individually passing app cases.
Contact-assisted recovery now has Rust-owned protocol and delayed vault-sweep screens, without
either original device or an imported configuration, and a separate ordinary-wallet recovery
action. Native file exchange is implemented but not physically verified; animated QR,
OS adapter/device verification, durable incoming reviews/full workflow journaling, message
history/cleanup and exhaustive interruption coverage are still unfinished. Outgoing message
resending now has passing Rust, native-library and browser tests.

The latest preview host uses port 8790. Its unused session (zero devices, checked via the API)
was refreshed with the current binary; its files were retained. Older port-8787, port-8788 and
port-8789 sessions were left intact. All use the interactive regtest node. Test runners create separate,
disposable nodes and never reset that interactive chain. No commits or pushes were made.
