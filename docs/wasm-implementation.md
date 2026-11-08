# Browser-local Rust and Dark UI implementation

Goal: run the existing wallet/signer application APIs inside browser WASM, and match the
Dark reference in `mockups/compare.html`. Keep native React Native as a UI over those same
Rust APIs. Do not commit or push this work yet.

## Required evidence before completion

- [x] Real browser WASM initializes independent wallet/signer keys, builds, validates and
  signs every protocol operation locally. No wallet-command fallback to the Rust host.
- [x] Browser storage survives reload, remains device-separated, and commits writes before
  signing/broadcast workflows continue. Test failed writes and interruption/retry behavior.
- [x] The local service only supplies public chain data, broadcasts signed transactions,
  simulates cloud storage of encrypted backups, and administers the real regtest node.
- [x] All 25 existing browser scenarios pass through WASM; add tests proving local execution
  and no private key material in backend requests/state.
- [x] Native/CLI Rust regression suites, architecture tests and native bindings still pass.
- [x] Wallet, Vault, privacy gate, activity, sheets, signer and setup use the Dark mockup's
  typography, spacing, palette, icons and surfaces. Verify rendered desktop/narrow layouts.
- [x] The UI remains usable for all real operations, with no invented balances/history or
  mock signing. Wallet/Vault stay separate; private balances must not leak into Wallet.
- [x] Document reproducible build/run commands and remaining platform/security distinctions.

## Platform boundary

Keep synchronous Rust application APIs (native already runs them off the UI thread). A
dedicated browser worker runs each WASM device. A separate I/O worker performs asynchronous
IndexedDB transactions and HTTP requests; a bounded shared-memory mailbox lets the Rust
worker wait for completion without blocking the UI. This requires cross-origin isolation.
Browser storage adapters must acknowledge a write only after its transaction commits.
The same Rust policy, signing, review binding, and retry logic remains authoritative.

The existing native SQLite store remains intact. Browser BDK persistence uses serialized
BDK ChangeSets rather than compiling the native SQLite/filesystem stack into WASM.
No browser worker/storage partition is claimed equivalent to native OS key protection or
independent physical devices. The browser remains an experimental testing playground.

## Initial inspection

The existing app depends on native filesystem paths, atomic file replacement, SQLite and
TCP/TLS clients. Browser bindings therefore require platform adapters, not just a target
flag. The reference is the `themeDark` panel, not `clarity.html`. Existing dirty mockup
changes are preserved. In-app browser discovery returned no available browsers; use the
repository's isolated Playwright browser for rendered verification.

## Verification log (2026-09-17)

`cargo test --workspace --locked` passed after the browser filesystem port and real wallet
activity projection. This includes the CLI, architecture boundaries, device application
tests, policy-activation retry tests, Electrum recovery and transaction-vector snapshots.
The explicit regtest tests remain opt-in in that command.

`cargo test --manifest-path mobile/rust/Cargo.toml --locked --lib` passed all eight C ABI
unit tests. `cd mobile && npm run test:native:regtest` separately passed the real-node C ABI
payment/policy approval scenario. Its disposable Docker chain was removed by the runner. These host tests do not establish iOS/Android application builds or device UI
behavior; local Xcode/Android tooling still needs configuration for that separate work.

The six opt-in root regtest cases also passed, in the repository runner's order:
`regtest_rpc`, `regtest_ceremony`, then `regtest_recovery`. Running these Cargo test targets
together sorts their execution differently and exhausts regtest's block subsidy before
`regtest_rpc`, which intentionally mines its initial funding. Keep the existing order or use
fresh chains; this is not a wallet synchronization failure.

`npm run test:unit` passed 34 tests. The browser runtime cases cover role/connection binding,
failed worker construction, startup failure, WASM initialization timeout, retry and mandatory
cross-origin isolation. `npm run typecheck` passed.

`npm run export:web` now builds WASM before exporting. The exported production assets were
served into an isolated browser context with COOP/COEP and successfully created/paired two
real Rust WASM devices and generated a fresh receive address, without using Metro. That
synthetic asset-serving check explicitly granted its context local-network access (synthetic
responses lack a real loopback socket address); normal loopback dev serving needs no such
test override.

Earlier browser passes exposed outdated test navigation: sheets must be closed before
changing tabs or opening a message underneath them. Those steps now use visible close controls;
no forced clicks or bypasses into the wallet API were added. The final full 31-case run
passed in 15.0 minutes, including all original 25 scenarios and setup/activity visuals. Results: `/tmp/anzen-dark-final-suite`. Its isolated Docker chain was
removed by the runner. The two interruption tests and one reference-comparison test below
passed separately: 34 browser cases verified in total, not a claimed single 34-case run.

Two additional browser interruption tests passed against a separate disposable real node:
Bitcoin accepts the signed rollover, then either its reply is lost or the page reloads before
the reply arrives. After reopening the two WASM devices and confirming the transaction,
explicitly resending the saved approval activates the same policy. Neither case resigns,
creates a different rollover, or automatically broadcasts on reload. The subsequent real
monthly withdrawal succeeds. Results: `/tmp/anzen-browser-interruption`.

The public-only service checks passed again: device/signing endpoints return 404; invalid
capabilities, origins, unknown chain commands and extra private-key fields are rejected;
and encrypted cloud objects round-trip. This complements
the architecture tests and the browser test that inspects all backend requests and host files.

Rendered review compared the current UI with `#themeDark` in `mockups/compare.html`: the
375×812 frame, 46px rounded balance typography, muted labels, watermark/glow/grain, inset glass
highlights, 52px action pills, stable floating tabs and rounded sheets. Setup, private Vault,
signer policy, actual wallet activity and Receive/Deposit QR screens were inspected. The
320px and 390px viewport checks confirm no horizontal overflow; real rendered QR images decode
to their displayed addresses and clipboard copies match. The mockup's sample merchants,
fiat conversions and scheduled payments are deliberately not invented in the live wallet.
The browser's grain/status fade is browser-only; native uses its own status/safe-area treatment.
An additional rendered-reference test passed: it loads the retained mockup and directly
compares computed frame/screen dimensions, typography, colors, action-pill geometry and the
floating tab bar with the real Wallet. Results and paired screenshots:
`/tmp/anzen-dark-reference-check`.

The final Dark assets were exported again and tested without a development bundle. Two real
WASM devices initialized, paired and produced a fresh scannable receive address. Screenshot:
`/tmp/anzen-production-receive.png`. Browser-only success does not establish native device
builds or production wallet security.

## Completion audit

The browser WASM and Dark UI objective is complete. Evidence maps to the gates above:

| Gate | Implementation and verification |
| --- | --- |
| Local Rust execution | `mobile/wasm/src/lib.rs` directly dispatches the shared role facades; `device-worker.mjs` owns each WASM instance. All original browser scenarios pass through this path. |
| Durable, separate storage | `browser-platform.spec.ts` exercises encrypted persistence, path/role binding, missing-key rejection, transaction abort/atomic rename and worker locking. `browser-interruption.spec.ts` proves approved-transaction recovery after reply loss and page termination. |
| Public-only service | `tests/architecture.rs`, `playground/test.mjs` and the main browser scenario check absent device APIs, strict request validation, public network messages and no host private-wallet files. |
| Native/CLI compatibility | Root workspace tests, eight native C ABI tests, all six explicit root regtest cases and three native real-node scenarios passed. Native keeps its filesystem/SQLite backend. |
| Dark style and usable real UI | `dark-reference.spec.ts` compares live rendered styles to the original mockup; `dark-ui.spec.ts` checks real QR/copy, privacy and narrow layouts. The full scenario run exercises setup, signing, payments, renewal, revocation and recovery through visible controls. |
| Build and handoff | TypeScript and 34 adapter/unit tests pass. WASM build and web export pass; the exported assets initialize/pair real devices without Metro. `mobile/README.md` documents commands, runtime requirements and security limitations. |

iOS/Android installation and physical-device verification remain part of the separate native
app work. Browser storage is not a secure element, and this is not a production-security audit.
No mainnet wallet state was accessed, and no commits or pushes were made for this goal.
