// Each simulated device owns a WASM instance and a private platform worker. React sees
// application results/protocol messages only, never the storage encryption key or raw files.
let runtime, settings, io, poisoned = false;
const buffer = new SharedArrayBuffer(16 + 16 * 1024 * 1024);
const header = new Int32Array(buffer, 0, 4);
const decoder = new TextDecoder('utf-8', { fatal: true });

function platform(message) {
  if (poisoned) throw new Error('Browser device interrupted; reload before retrying');
  Atomics.store(header, 0, 0);
  Atomics.store(header, 1, 0);
  io.postMessage({ ...message, buffer });
  // Public address discovery is paced by the indexer's limits. A recovered wallet
  // with years of address history can legitimately take longer than a local RPC scan.
  const deadline = performance.now() + (settings?.network && settings.network !== 'regtest' ? 15 * 60_000 : 310_000);
  while (Atomics.load(header, 0) === 0) {
    const remaining = deadline - performance.now();
    if (remaining <= 0 || Atomics.wait(header, 0, 0, remaining) === 'timed-out') {
      poisoned = true; // Never reuse a mailbox that a late operation may still write to.
      io.close();
      throw new Error('Browser operation timed out; reload before retrying. A broadcast may already have succeeded.');
    }
  }
  // TextDecoder rejects SharedArrayBuffer-backed views. Copy the completed response before
  // decoding, which also keeps the next platform call from changing the bytes being parsed.
  return decoder.decode(new Uint8Array(buffer, 16, Atomics.load(header, 1)).slice());
}
globalThis.anzenPlatformCall = request => platform({ type: 'call', request });

let queue = Promise.resolve();
self.onmessage = ({ data }) => {
  queue = queue.then(async () => {
    try {
      if (data.type === 'initialize') {
        if (runtime) throw new Error('Browser device is already initialized');
        settings = Object.freeze(data.settings);
        // The UI creates two sibling workers and transfers a private channel between them.
        // Starting a nested worker while its parent blocks can stall browser worker startup.
        io = data.port;
        if (!io) throw new Error('Browser platform channel is missing');
        runtime = await import('../wasm/anzen_browser.js');
        await runtime.default();
        self.postMessage({ id: data.id, ok: null }); return;
      }
      if (data.type === 'close') { io?.postMessage({ type: 'close' }); self.close(); return; }
      if (!runtime || poisoned) throw new Error('Browser device is unavailable; reload before retrying');
      if (data.command.method === 'initialize' && settings.network && data.command.network !== settings.network) throw new Error('Browser device network mismatch');
      const result = JSON.parse(runtime.execute(settings.role, settings.allowMainnet, JSON.stringify(data.command)));
      self.postMessage({ id: data.id, ...result });
    } catch (error) {
      if (error instanceof WebAssembly.RuntimeError) { poisoned = true; io?.close(); }
      self.postMessage({ id: data.id, error: error.message || 'Browser device failed', fatal: poisoned });
    }
  });
};
