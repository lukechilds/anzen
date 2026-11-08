# Static browser runtime

Goal: serve the React Native Web UI and Rust WASM as static files. Mainnet connects
directly to HTTPS Esplora, Mutinynet to Electrum WSS; only regtest uses
the local Rust/Docker test host. No Anzen server holds keys or runs wallet logic.

Implementation checklist:

- [x] Public-only, asynchronous Rust/BDK Esplora adapter in the I/O worker.
- [x] Mutinynet Electrum WSS with randomized server preference and bounded failover.
- [x] Mainnet opt-in and Mutinynet chain-identity checks (not merely signet genesis).
- [x] Backend-free startup, independent browser devices, persistent session selection.
- [x] Browser-local encrypted backup simulation, download/import, honest durability warning.
- [x] Static hosting configuration for WASM and cross-origin isolation.
- [x] Automated adapter, storage, network-boundary and static-browser tests.
- [x] Verify regtest remains usable and live public endpoints work without broadcasts.

The public indexer sees queried scripts and transaction history. Its chain view is
trusted for synchronization, just as with the current Electrum backend; checkpoint
checks identify the selected network, not a fully validated/SPV chain. Production
browser wallet security and independent off-device backup remain separate concerns.

## Electrum update (2026-09-24)

Mutinynet now selects randomly between `wss://mutinynet.com/electrum-websocket` and
`wss://electrum.mutinynet.arkade.sh`. Failed operations restart once against the
other server from the original public wallet state. A successful fallback becomes
the device's preferred server until reload. Mainnet/regtest transports are unchanged.

The device's Rust worker uses BDK's Electrum adapter, with batches of 20 scripts.
JavaScript supplies only framed WebSocket I/O through the existing private worker
channel. See the mobile architecture document for the browser-only dependency patch.

Verification passed: root workspace tests with `--features playground,esplora,electrum`
(six explicit real-node tests skipped), 42 platform unit tests, TypeScript, Clippy with
warnings denied, WASM build and static export. The static suite passed seven deterministic
browser tests, including mid-discovery disconnect, a 15-second stalled RPC, sticky fallback,
both servers unavailable, retry without reload, and wrong-chain rejection. Worker sockets
use a real loopback WebSocket fixture; Playwright's page-level socket interception does
not intercept these worker connections. Results: `/tmp/anzen-electrum-static-final`.

Three read-only live browser checks passed separately: fresh empty-wallet discovery
took **5.900 seconds** with Mutinynet first, **5.909 seconds** with Arkade first, and
12.466 seconds against mainnet Esplora. No existing wallet was opened and no transaction
was broadcast. Results: `/tmp/anzen-electrum-live`. These replace the minutes-long
Mutinynet HTTP timings below, which document the original implementation.

A targeted real-regtest browser test also passed: pair both devices, fund, sign,
activate, enforce the timelock and withdraw (`/tmp/anzen-electrum-regtest`). Its
disposable Docker node/network were removed by the test runner. No changes pushed.

## Original Esplora verification (2026-09-17)

The six Rust Esplora tests pass with `cargo test --features playground,esplora --lib
esplora_transport`. Coverage includes network mismatch before broadcast, linked-header MTP,
bad headers, confirmed-only UTXOs, raw-transaction amount/script checks, changed confirmation
hashes, exact broadcast bytes and accepted-broadcast retry, and BDK recovery of funds at
external index 70 (beyond the usual 20-address gap). The adapter's public update applies to
a watch-only wallet and preserves its balance/derivation index.

The root workspace suite passes with `--features playground,esplora`, including CLI,
device application, architecture, activation and transaction-vector tests. The existing
explicit real-node tests remain ignored in that command. Native C ABI tests pass (8),
as do TypeScript and adapter/unit tests (37), Clippy with warnings denied, formatting,
WASM build and production web export. Native device installation is not part of this goal.

The live, read-only browser smoke tests passed against both public endpoints. Fresh empty
wallets synchronized through the real WASM/BDK transport; neither test made a POST or
contacted `/session`, `/chain`, `/cloud` or `/node`. The initial Mutinynet scan
first hit HTTP 429. The adapter now respects the deployment's two-history-requests/second
limit across devices using a Web Lock and performs bounded GET-only retries. The passing
live Mutinynet scan took 2.7 minutes; mainnet took 14.4 seconds. No valid or invalid
transaction was submitted to either public network during verification.

The deterministic static-site tests use historic real block headers with synthetic indexer
history, not mocked signing or wallet algorithms. They pass backend-free pairing and BDK
sync, real encrypted phone-backup export and restoration after reload, mainnet consent on
each reopening, wrong-signet rejection and retry after an offline indexer. Additional
browser-cloud checks cover file import, transaction abort after request success, stored
content-hash tampering and separate network scopes. Failed writes are never acknowledged.
The final production-asset run passed all five deterministic static tests (3.7 minutes):
`/tmp/anzen-static-production-final`. Both optional live checks passed separately in
`/tmp/anzen-static-paced`; that earlier run also contained two fixture tests with an
outdated 30-second wait, corrected and passed in the final run. The seven checks are not
represented as a single combined seven-test pass.

The full browser regression run passed 38 tests in 19.9 minutes, including the original
34 browser tests and four static cases; its two opt-in live cases were skipped. The run
began before static tests moved to their dedicated no-Docker configuration. Current
At that point `test:e2e` ran the original 34; `test:static` ran five deterministic plus two optional live
cases. The disposable Docker node was removed by the runner. Results:
`/tmp/anzen-browser-static-regression`.

Final persistence testing also passed mainnet resume from the plain homepage (without a
saved fragment), wrong-network rejection and cloud durability/integrity: three tests in
`/tmp/anzen-static-persistence-final`. Network selection finds its saved device pair, and
mainnet startup cannot accidentally replace it with freshly generated keys. All code remains
uncommitted and unpushed; existing CLI mainnet wallet directories were never opened.

Only static files are served by `npm run preview:web`; the export contains `_headers`,
worker modules and the WASM module. A root-path HTTPS static host with COOP/COEP is required.
The browser simulation does not give two roles independent compromise protection or make
browser-local cloud storage an off-device backup. These limitations are visible in the UI
and [mobile usage guide](../mobile/README.md).
