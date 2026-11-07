import { openDeviceStore } from './storage.mjs';
import { createElectrumTransport } from './electrum-ws.mjs';

let store, config, release, electrum, watching;
const encoder = new TextEncoder();
function reply(buffer, result) {
  const header = new Int32Array(buffer, 0, 4), body = new Uint8Array(buffer, 16);
  let bytes = encoder.encode(JSON.stringify(result));
  if (bytes.length > body.length) bytes = encoder.encode(JSON.stringify({ error: 'Platform response exceeds mailbox capacity' }));
  body.set(bytes);
  Atomics.store(header, 1, bytes.length);
  Atomics.store(header, 0, 1);
  Atomics.notify(header, 0);
}
async function initialize(settings) {
  if (config) throw new Error('Device already initialized');
  config = Object.freeze(settings);
  // Mutinynet and mainnet use Electrum over WebSocket; regtest uses the local playground host.
  if (config.role === 'wallet') electrum = createElectrumTransport();
  await new Promise((resolve, reject) => {
    navigator.locks.request(`anzen-device-${config.id}`, { ifAvailable: true }, async lock => {
      if (!lock) throw new Error('This test device is open in another tab; close it before resuming');
      store = await openDeviceStore(config.id, config.role);
      resolve();
      await new Promise(done => { release = done; });
    }).catch(reject);
  });
}
async function proxy(request, timeoutMs) {
  const response = await fetch(`${config.endpoint}/chain`, {
    method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${config.capability}` },
    body: JSON.stringify({ network: 'regtest', request }), signal: AbortSignal.timeout(timeoutMs),
  });
  const text = await response.text();
  if (!response.ok) {
    let detail = text;
    try { detail = JSON.parse(text).error || text; } catch {}
    throw new Error(`Chain transport failed (${response.status}): ${detail.slice(0, 500)}`);
  }
  return JSON.parse(text);
}
// Live updates. The listener is shared Rust (a separate, keyless WASM instance); this worker only
// carries out its actions: open or close its socket, send frames, report hints, poll regtest.
async function startWatching() {
  const wasm = await import('../wasm/anzen_browser.js');
  await wasm.default();
  const listener = new wasm.ChainWatcher(config.network);
  let socket, polling = false, closed = false;
  const now = () => Date.now();
  const perform = json => {
    for (const action of JSON.parse(json)) {
      if (action.action === 'changed') self.postMessage({ type: 'chain-changed', reason: action.reason });
      else if (action.action === 'send') socket?.send(action.frame);
      else if (action.action === 'close') { socket?.close(); socket = undefined; }
      else if (action.action === 'connect') {
        socket?.close();
        try {
          const current = electrum.listen(action.url, {
            opened: () => { if (!closed) perform(listener.opened(now())); },
            frame: text => { if (!closed) perform(listener.received(text, now())); },
            closed: () => { if (closed || socket !== current) return; socket = undefined; perform(listener.closed(now())); },
          });
          socket = current;
        } catch { socket = undefined; perform(listener.closed(now())); }
      }
    }
  };
  const timer = setInterval(async () => {
    if (listener.polls_activity() && !polling) {
      polling = true;
      try { perform(listener.activity(JSON.stringify(await proxy({ method: 'activity' }, 10_000)))); }
      catch { /* The next poll retries; a failed poll never refreshes by itself. */ }
      finally { polling = false; }
    }
    if (!closed) perform(listener.tick(now()));
  }, 1_000);
  perform(listener.tick(now()));
  return {
    watch: scripthashes => perform(listener.watch(JSON.stringify(scripthashes))),
    close: () => { closed = true; clearInterval(timer); socket?.close(); listener.free(); },
  };
}
function watch(scripthashes) {
  if (config?.role !== 'wallet') return;
  watching ??= startWatching();
  watching.then(watcher => watcher.watch(scripthashes)).catch(() => { watching = undefined; });
}
function unwatch() {
  watching?.then(watcher => watcher.close()).catch(() => {});
  watching = undefined;
}
async function handle({ data }) {
  const { buffer } = data;
  try {
    if (data.type === 'close') { unwatch(); store?.close(); release?.(); self.close(); return; }
    if (!store) throw new Error('Browser device storage is not ready');
    const call = JSON.parse(data.request);
    if (call.type === 'storage') { reply(buffer, { ok: await store.execute(call.request) }); return; }
    if (call.type === 'electrum') {
      if (!electrum) throw new Error('Offline signer cannot access chain transport');
      if (config.network === 'bitcoin' && !config.allowMainnet) throw new Error('Browser network is not enabled');
      reply(buffer, { ok: await electrum(call) }); return;
    }
    if (call.type !== 'chain' || config.role !== 'wallet') throw new Error('Offline signer cannot access chain transport');
    if (call.network !== 'regtest' || (config.network && config.network !== 'regtest')) throw new Error('Only regtest uses the playground host');
    reply(buffer, { ok: await proxy(call.request, 300_000) });
  } catch (error) { reply(buffer, { error: error.message || 'Browser platform operation failed' }); }
}
self.onmessage = async ({ data }) => {
  if (data.type === 'watch') { watch(Array.isArray(data.scripthashes) ? data.scripthashes : []); return; }
  if (data.type === 'unwatch') { unwatch(); return; }
  if (data.type !== 'configure') return;
  try {
    await initialize(data.settings);
    data.port.onmessage = handle;
    self.postMessage({ ready: true });
  } catch (error) { self.postMessage({ error: error.message || 'Browser storage failed to initialize' }); }
};
