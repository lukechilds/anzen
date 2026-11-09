import { test, expect, BrowserContext, Page } from '@playwright/test';
import fixtures from '../../test-vectors/chain-headers.json';
import { closeVaultPanel, openVaultPanel, vaultAddress, setUpVault, recoverWithSigner, approveOnSigner, deviceId } from './helpers';
import { WebSocketServer, WebSocket } from 'ws';
import { once } from 'node:events';
import { AddressInfo } from 'node:net';

const origin = (baseURL?: string) => baseURL || 'http://localhost:8081';

// Real historic headers (the fixture was retrieved from public indexers on 2026-09-17). Only
// the Electrum server is replaced; UI, WASM, BDK and crypto execute normally.
async function indexer(context: BrowserContext, network: 'bitcoin' | 'signet', wrongChain = false) {
  const requests: string[] = [];
  await electrumIndexer(context, requests, { network, wrongChain });
  return requests;
}

const socketServers: WebSocketServer[] = [];
test.afterEach(async () => {
  for (const server of socketServers.splice(0)) {
    for (const client of server.clients) client.terminate();
    await new Promise<void>(resolve => server.close(() => resolve()));
  }
});
type ElectrumOptions = { network?: 'bitcoin' | 'signet'; wrongChain?: boolean; failFirstHistory?: boolean; silentFirst?: boolean; offline?: boolean };
async function electrumIndexer(context: BrowserContext, requests: string[], options: ElectrumOptions = {}) {
  // Upstream servers of sync connections, in order. The live-update subscription socket is
  // tracked separately so failover assertions see only BDK's sync connections.
  const connections: string[] = [], watchers: WebSocket[] = [], subscribed: string[] = [];
  const fixture = fixtures[options.network ?? 'signet'];
  // Playwright's routeWebSocket does not intercept worker-owned sockets. Use a
  // real loopback fixture, replacing ONLY the platform WebSocket constructor URL.
  // Rust/BDK, timeout handling, framing, selection and failover are untouched.
  const server = new WebSocketServer({ host: '127.0.0.1', port: 0 });
  socketServers.push(server); await once(server, 'listening');
  const local = `ws://127.0.0.1:${(server.address() as AddressInfo).port}`;
  await context.route('**/runtime/electrum-ws.mjs', async route => {
    const response = await route.fetch();
    const body = (await response.text()).replace('WebSocketImpl = WebSocket,', `WebSocketImpl = class extends WebSocket { constructor(url) { super(${JSON.stringify(local)} + '/?upstream=' + encodeURIComponent(url)); } },`);
    await route.fulfill({ response, body });
  });
  server.on('connection', (ws: WebSocket, request) => {
    const upstream = new URL(request.url!, local).searchParams.get('upstream')!;
    if (options.offline) { connections.push(upstream); ws.close(1011, 'offline test'); return; }
    let purpose: 'sync' | 'watch' | undefined, connection = 0;
    ws.on('message', message => {
      const payload = JSON.parse(String(message)), calls = Array.isArray(payload) ? payload : [payload];
      if (!purpose) {
        purpose = calls[0].method === 'server.version' && calls[0].params[0] === 'anzen-watch' ? 'watch' : 'sync';
        if (purpose === 'sync') connection = connections.push(upstream); else watchers.push(ws);
      }
      if (options.silentFirst && connection === 1) return;
      if (connection === 1 && options.failFirstHistory && calls.some(call => call.method === 'blockchain.scripthash.get_history')) {
        ws.close(1011, 'simulated indexer outage during discovery'); return;
      }
      const replies = calls.map(call => {
        const { method, params } = call;
        let result: unknown;
        if (method === 'server.version') result = ['anzen-test-indexer', '1.4'];
        else if (method === 'blockchain.block.header') {
          requests.push(`/block-height/${params[0]}`);
          result = options.wrongChain && params[0] === 1 ? fixtures.bitcoin.headers[0] : fixture.headers[1 - params[0]];
        } else if (method === 'blockchain.headers.subscribe') result = { height: 1, hex: fixture.headers[0] };
        else if (method === 'blockchain.block.headers') {
          const headers = [...fixture.headers].reverse().slice(params[0], params[0] + params[1]);
          result = { count: headers.length, hex: headers.join(''), max: 2016 };
        } else if (method === 'blockchain.scripthash.get_history') { requests.push(`/scripthash/${params[0]}`); result = []; }
        else if (method === 'blockchain.scripthash.listunspent') result = [];
        else if (purpose === 'watch' && method === 'blockchain.scripthash.subscribe') { subscribed.push(params[0]); result = null; }
        else if (purpose === 'watch' && method === 'server.ping') result = null;
        expect(result, `unexpected Electrum RPC ${method}`).toBeDefined();
        return { id: call.id, result };
      });
      ws.send(JSON.stringify(Array.isArray(payload) ? replies.reverse() : replies[0]));
    });
  });
  return Object.assign(connections, { watchers, subscribed });
}

// Live updates sync by themselves (on open, and once more when the subscription starts). Wait
// until no Electrum query has arrived for a second before counting what a step causes.
async function settled(queries: string[]) {
  await expect.poll(async () => { const count = queries.length; await new Promise(r => setTimeout(r, 1_000)); return queries.length - count; }, { timeout: 60_000 }).toBe(0);
}

function noBackend(context: BrowserContext) {
  const requests: string[] = [];
  context.on('request', request => {
    const url = new URL(request.url());
    if (['/session', '/chain', '/cloud', '/node', '/node/job'].includes(url.pathname)) requests.push(request.url());
  });
  return requests;
}

async function pair(page: Page, url: string) {
  await page.goto(url); await page.getByRole('button', { name: 'Mutinynet', exact: true }).click();
  const wallet = page.getByTestId('device-wallet'), signer = page.getByTestId('device-signer');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  const s = (name: string) => signer.getByRole('button', { name, exact: true });
  await w('Get started').click(); await setUpVault(page, wallet);
  await expect(wallet.getByTestId('device-screen')).toHaveAttribute('data-vault-height', '1');
  return { wallet, signer, w, s };
}

test('static Mutinynet pairs, syncs, exports a local backup and restores a replacement phone after reload', async ({ page, context, baseURL }) => {
  const external = noBackend(context), queries = await indexer(context, 'signet');
  const { wallet, w, s } = await pair(page, origin(baseURL));
  await expect(page.getByText('Regtest controls', { exact: true })).toHaveCount(0);
  const address = await vaultAddress(wallet); expect(address).toMatch(/^tb1p/);
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  // The wallet discovered its addresses by itself when it opened.
  await expect.poll(() => queries.filter(path => path.startsWith('/scripthash/')).length, { timeout: 180_000 }).toBeGreaterThanOrEqual(200);
  // Setup saved the recovery backup; exporting it again stores the same object.
  await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  const download = page.waitForEvent('download');
  await page.getByRole('button', { name: /^Download backup / }).click();
  expect((await download).suggestedFilename()).toMatch(/^anzen-backup-[a-f0-9]{8}\.json$/);
  await page.reload();
  await page.getByRole('button', { name: 'Refresh cloud backups', exact: true }).click();
  await expect(page.getByRole('button', { name: /^Load backup / })).toHaveCount(1);
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click(); await w('Get started').click();
  await recoverWithSigner(page);
  await page.getByRole('button', { name: /^Load backup / }).click();
  await w('Request phone key recovery').click(); await approveOnSigner(page, 'Approve phone recovery');
  await expect(wallet.getByText('Old phone key recovered into encrypted storage. Your new wallet key is unchanged.')).toBeVisible();
  expect(external).toEqual([]);
  await page.screenshot({ path: test.info().outputPath('static-browser-recovery.png'), fullPage: true });
});

test('mainnet asks once, then reopens where it left off, never a localhost backend', async ({ page, context, baseURL }) => {
  const backend = noBackend(context), queries = await indexer(context, 'bitcoin');
  await page.goto(`${origin(baseURL)}/#network=bitcoin`);
  // A link alone never opens real-bitcoin devices: the first visit asks.
  await expect(page.getByText('Choose a network', { exact: true })).toBeVisible();
  expect(queries).toEqual([]);
  await page.getByRole('button', { name: 'Mainnet', exact: true }).click();
  const wallet = page.getByTestId('device-wallet');
  await wallet.getByRole('button', { name: 'Get started', exact: true }).click();
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0000', { timeout: 60_000 });
  await wallet.getByRole('button', { name: '↓ Receive', exact: true }).click();
  await expect(wallet.getByText(/^bc1p/)).toBeVisible();
  const savedWallet = await deviceId(page, 'wallet', 'bitcoin');
  expect(savedWallet).toBeTruthy();
  // The devices are remembered by the browser, not the URL.
  expect(new URL(page.url()).hash).toBe('');
  await page.reload();
  await expect(page.getByText('Real bitcoin', { exact: true })).toBeVisible();
  await expect(wallet.getByRole('tab', { name: 'Wallet', exact: true })).toBeVisible();
  // A later visit to the plain page opens the same mainnet devices without asking again.
  await page.goto('about:blank'); await page.goto(origin(baseURL));
  await expect(page.getByText('Choose a network', { exact: true })).toHaveCount(0);
  await expect(wallet.getByRole('tab', { name: 'Wallet', exact: true })).toBeVisible();
  expect(await deviceId(page, 'wallet', 'bitcoin')).toBe(savedWallet);
  expect(backend).toEqual([]);
});

test('Mutinynet rejects an indexer with the shared signet genesis but a different chain', async ({ page, context, baseURL }) => {
  const backend = noBackend(context), queries = await indexer(context, 'signet', true);
  await page.goto(origin(baseURL)); await page.getByRole('button', { name: 'Mutinynet', exact: true }).click(); const wallet = page.getByTestId('device-wallet');
  await wallet.getByRole('button', { name: 'Get started', exact: true }).click();
  await expect(wallet.getByTestId('sync-status')).toContainText('not on Mutinynet');
  expect(queries.every(path => path.startsWith('/block-height/'))).toBe(true);
  expect(backend).toEqual([]);
});

test('offline indexer failure is visible and retry does not fall back to a backend', async ({ page, context, baseURL }) => {
  const backend = noBackend(context);
  const availability = { offline: true };
  const failed = await electrumIndexer(context, [], availability);
  await page.goto(origin(baseURL)); await page.getByRole('button', { name: 'Mutinynet', exact: true }).click(); const wallet = page.getByTestId('device-wallet');
  await wallet.getByRole('button', { name: 'Get started', exact: true }).click();
  await expect(wallet.getByTestId('sync-status')).toContainText('All 2 Mutinynet Electrum servers failed');
  expect(new Set(failed).size).toBe(2);
  availability.offline = false;
  await wallet.getByRole('button', { name: 'Retry', exact: true }).click();
  await expect(wallet.getByTestId('sync-status')).toHaveCount(0, { timeout: 180_000 });
  await expect(wallet.getByRole('alert')).toHaveCount(0);
  expect(backend).toEqual([]);
});

test('mainnet syncs only over its Electrum WebSocket server', async ({ page, context, baseURL }) => {
  const backend = noBackend(context), queries: string[] = [];
  const connections = await electrumIndexer(context, queries, { network: 'bitcoin' });
  const http: string[] = [];
  context.on('request', request => { if (/^https?:\/\/(?!localhost|127\.0\.0\.1)/.test(request.url())) http.push(request.url()); });
  await page.goto(`${origin(baseURL)}/#network=bitcoin`);
  await page.getByRole('button', { name: 'Mainnet', exact: true }).click();
  const wallet = page.getByTestId('device-wallet');
  await wallet.getByRole('button', { name: 'Get started', exact: true }).click();
  await expect.poll(() => queries.filter(path => path.startsWith('/scripthash/')).length, { timeout: 180_000 }).toBeGreaterThanOrEqual(200);
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0000');
  expect(new Set(connections)).toEqual(new Set(['wss://electrum.lu.ke']));
  expect(http.filter(url => !url.includes('fonts.'))).toEqual([]);
  expect(backend).toEqual([]);
});

test('an Electrum notification refreshes the wallet without pressing sync', async ({ page, context, baseURL }) => {
  const queries: string[] = [], connections = await electrumIndexer(context, queries);
  await page.goto(origin(baseURL)); await page.getByRole('button', { name: 'Mutinynet', exact: true }).click(); const wallet = page.getByTestId('device-wallet');
  await wallet.getByRole('button', { name: 'Get started', exact: true }).click();
  // Opening the wallet syncs once by itself, and the watcher subscribes to its scripts.
  await expect.poll(() => queries.filter(path => path.startsWith('/scripthash/')).length, { timeout: 180_000 }).toBeGreaterThanOrEqual(200);
  await expect.poll(() => connections.subscribed.length).toBeGreaterThan(0);
  expect(connections.watchers).toHaveLength(1);
  await settled(queries);
  const before = connections.length, count = queries.length;
  connections.watchers[0].send(JSON.stringify({ jsonrpc: '2.0', method: 'blockchain.scripthash.subscribe', params: [connections.subscribed[0], 'changed'] }));
  // A new sync connection and chain queries, with no click. (A new wallet has revealed no
  // addresses yet, so this incremental sync correctly queries no address history.)
  await expect.poll(() => connections.length).toBe(before + 1);
  await expect.poll(() => queries.length).toBeGreaterThan(count);
  await expect(wallet.getByRole('alert')).toHaveCount(0);
});

for (const failure of ['disconnect', 'stall'] as const) {
  test(`Mutinynet ${failure} fails over, preserves discovery and keeps the healthy server preferred`, async ({ page, context, baseURL }) => {
    const backend = noBackend(context), queries: string[] = [];
    const connections = await electrumIndexer(context, queries, { failFirstHistory: failure === 'disconnect', silentFirst: failure === 'stall' });
    await page.goto(origin(baseURL)); await page.getByRole('button', { name: 'Mutinynet', exact: true }).click();
    const wallet = page.getByTestId('device-wallet');
    await wallet.getByRole('button', { name: 'Get started', exact: true }).click();
    // The wallet's first sync runs by itself: it fails over to the other server and discovers.
    await expect.poll(() => queries.filter(path => path.startsWith('/scripthash/')).length, { timeout: 60_000 }).toBeGreaterThanOrEqual(200);
    await expect(wallet.getByTestId('sync-status')).toHaveCount(0);
    expect(connections[0]).not.toBe(connections[1]);
    expect(connections.slice(1).every(server => server === connections[1])).toBe(true);
    await expect.poll(() => connections.subscribed.length).toBeGreaterThan(0);
    await settled(queries);
    expect(connections.slice(1).every(server => server === connections[1])).toBe(true);
    // A notification triggers the next sync, which stays on the healthy server and only
    // checks known addresses.
    const count = queries.length, opened = connections.length;
    connections.watchers[0].send(JSON.stringify({ jsonrpc: '2.0', method: 'blockchain.headers.subscribe', params: [{ height: 1 }] }));
    await expect.poll(() => connections.length).toBe(opened + 1);
    expect(connections.at(-1)).toBe(connections[1]);
    await expect(wallet.getByTestId('sync-status')).toHaveCount(0);
    expect(queries.length - count).toBeLessThan(20);
    expect(backend).toEqual([]);
  });
}

test('browser cloud imports opaque ciphertext, acknowledges only committed writes, checks hashes and separates networks', async ({ page, context, baseURL }) => {
  const backend = noBackend(context);
  await page.goto(origin(baseURL)); await page.getByRole('button', { name: 'Mutinynet', exact: true }).click(); // Deliberately only transport framing, not a cryptographically valid phone backup.
  // Rust authenticates actual recovery envelopes in the end-to-end recovery test above.
  const backup = { version: 2, kind: 'vault-cloud-recovery', encrypted_payload: { ciphertext: 'opaque test ciphertext' } };
  const upload = (value: object) => page.getByLabel('Import encrypted cloud backup', { exact: true }).setInputFiles({ name: 'backup.json', mimeType: 'application/json', buffer: Buffer.from(JSON.stringify(value)) });
  await upload(backup);
  await expect(page.getByRole('button', { name: /^Load backup / })).toHaveCount(1);
  await page.evaluate(() => {
    const put = IDBObjectStore.prototype.put;
    IDBObjectStore.prototype.put = function (...args: Parameters<typeof put>) {
      const request = put.apply(this, args);
      if (this.transaction.db.name === 'anzen-simulated-cloud-v1') request.addEventListener('success', () => this.transaction.abort());
      return request;
    };
  });
  await upload({ ...backup, encrypted_payload: { ciphertext: 'second opaque object' } });
  await expect(page.getByText(/Backup write aborted|transaction.*abort/i)).toBeVisible();
  await page.reload();
  await page.getByRole('button', { name: 'Refresh cloud backups', exact: true }).click();
  await expect(page.getByRole('button', { name: /^Load backup / })).toHaveCount(1);
  await page.evaluate(async () => {
    const db = await new Promise<IDBDatabase>(resolve => { const request = indexedDB.open('anzen-simulated-cloud-v1'); request.onsuccess = () => resolve(request.result); });
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('backups', 'readwrite'), store = tx.objectStore('backups');
      const request = store.getAll(); request.onsuccess = () => { const object = request.result[0]; object.text += ' '; store.put(object); };
      tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
    }); db.close();
  });
  await page.getByRole('button', { name: /^Load backup / }).click();
  await expect(page.getByText(/content hash check/)).toBeVisible();
  // Each network has its own cloud.
  await page.getByRole('tab', { name: 'Mainnet', exact: true }).click();
  await page.getByRole('button', { name: 'Use mainnet', exact: true }).click();
  await page.getByRole('button', { name: 'Refresh cloud backups', exact: true }).click();
  await expect(page.getByRole('button', { name: /^Load backup / })).toHaveCount(0);
  expect(backend).toEqual([]);
});

// Opt-in: real public servers (read-only). Each Mutinynet server is pinned in turn by redirecting
// the socket; server choice itself is Rust's.
const LIVE_SERVERS = { signet: ['wss://mutinynet.com/electrum-websocket', 'wss://electrum.mutinynet.arkade.sh'], bitcoin: ['wss://electrum.lu.ke'] };
for (const [network, server] of [['signet', 0], ['signet', 1], ['bitcoin', 0]] as const) {
  test(`live public ${network} endpoint ${server} supports empty-wallet BDK sync without broadcasting`, async ({ page, context, baseURL }) => {
    test.skip(process.env.ANZEN_LIVE_ELECTRUM !== '1', 'Opt-in read-only public network smoke test');
    const backend = noBackend(context), pinned = LIVE_SERVERS[network][server];
    await context.route('**/runtime/electrum-ws.mjs', async route => {
      const response = await route.fetch();
      const body = (await response.text()).replace('WebSocketImpl = WebSocket,', `WebSocketImpl = class extends WebSocket { constructor() { super(${JSON.stringify(pinned)}); } },`);
      await route.fulfill({ response, body });
    });
    const broadcasts: string[] = [];
    context.on('request', r => { if (r.method() === 'POST') broadcasts.push(r.url()); });
    await page.goto(`${origin(baseURL)}/#network=${network}`);
    await page.getByRole('button', { name: network === 'bitcoin' ? 'Mainnet' : 'Mutinynet', exact: true }).click();
    const wallet = page.getByTestId('device-wallet');
    const start = Date.now();
    await wallet.getByRole('button', { name: 'Get started', exact: true }).click();
    await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0000', { timeout: 180_000 });
    await expect(wallet.getByTestId('sync-status')).toHaveCount(0);
    await expect(wallet.getByRole('alert')).toHaveCount(0);
    console.log(`${network} server ${server}: full empty-wallet BDK discovery ${Date.now() - start} ms`);
    expect(backend).toEqual([]); expect(broadcasts).toEqual([]);
  });
}
