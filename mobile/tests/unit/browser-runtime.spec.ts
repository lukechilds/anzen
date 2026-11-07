import { test, expect } from '@playwright/test';
import { openBrowserDevice } from '../../src/browserRuntime';

// Worker lifecycle tests only. Cryptographic behavior is exercised by the real WASM E2E
// suite; these doubles make browser startup failures reproducible without waiting a minute.
const workers: FakeWorker[] = [];
let stall = false, failIO = false, sequence = 100;
let answer: (message: any) => any = () => ({});
class FakeWorker {
  onmessage: ((event: { data: any }) => void) | null = null;
  onerror: (() => void) | null = null;
  terminated = false;
  ports: MessagePort[] = [];
  messages: any[] = [];
  constructor(readonly url: string) {
    if (failIO && url.includes('io-worker')) throw new Error('Worker creation rejected');
    workers.push(this);
  }
  postMessage(message: any, transfer: Transferable[] = []) {
    this.messages.push(message);
    this.ports.push(...transfer.filter((item): item is MessagePort => item instanceof MessagePort));
    queueMicrotask(() => {
      if (this.terminated) return;
      if (message.type === 'configure') this.onmessage?.({ data: { ready: true } });
      else if (message.type === 'execute' || message.type === 'initialize') { if (!stall) this.onmessage?.({ data: { id: message.id, ok: answer(message) } }); }
    });
  }
  terminate() { this.terminated = true; this.ports.forEach(port => port.close()); }
}
const originalWorker = Object.getOwnPropertyDescriptor(globalThis, 'Worker');
const originalIsolation = Object.getOwnPropertyDescriptor(globalThis, 'crossOriginIsolated');
test.beforeEach(() => {
  stall = false; failIO = false; workers.length = 0; answer = () => ({});
  Object.defineProperty(globalThis, 'Worker', { value: FakeWorker, configurable: true });
  Object.defineProperty(globalThis, 'crossOriginIsolated', { value: true, configurable: true });
});
test.afterEach(() => {
  workers.forEach(worker => { worker.onerror?.(); worker.terminate(); });
  if (originalWorker) Object.defineProperty(globalThis, 'Worker', originalWorker); else Reflect.deleteProperty(globalThis, 'Worker');
  if (originalIsolation) Object.defineProperty(globalThis, 'crossOriginIsolated', originalIsolation); else Reflect.deleteProperty(globalThis, 'crossOriginIsolated');
});
const device = () => ({ id: (++sequence).toString(16).padStart(64, '0'), role: 'wallet' as const });
const settings = { endpoint: 'http://127.0.0.1:8787', capability: 'unit-test', allowMainnet: false };

test('browser device cannot reopen with changed network authority or another role', async () => {
  const identity = device(), runtime = await openBrowserDevice(identity, settings);
  expect(await openBrowserDevice(identity, { ...settings })).toBe(runtime);
  await expect(openBrowserDevice(identity, { ...settings, allowMainnet: true })).rejects.toThrow('settings changed');
  await expect(openBrowserDevice(identity, { ...settings, endpoint: 'http://127.0.0.1:1' })).rejects.toThrow('settings changed');
  await expect(openBrowserDevice(identity, { ...settings, network: 'signet' })).rejects.toThrow('settings changed');
  await expect(openBrowserDevice({ ...identity, role: 'signer' }, settings)).rejects.toThrow('role mismatch');
});

test('failure to create the IO worker terminates the already-created WASM worker', async () => {
  failIO = true;
  await expect(openBrowserDevice(device(), settings)).rejects.toThrow('Worker creation rejected');
  expect(workers).toHaveLength(1); expect(workers[0].terminated).toBe(true);
});

test('failed worker initialization cleans up and allows an explicit retry', async () => {
  const identity = device(); stall = true;
  const opening = openBrowserDevice(identity, settings);
  const rejected = expect(opening).rejects.toThrow(/worker failed|closed/);
  workers[0].onerror?.(); await rejected;
  expect(workers.every(worker => worker.terminated)).toBe(true);
  stall = false;
  await expect(openBrowserDevice(identity, settings)).resolves.toBeTruthy();
});

test('a stalled WASM download times out, releases the device and can be retried', async () => {
  const original = globalThis.setTimeout;
  let expire: (() => void) | undefined;
  globalThis.setTimeout = ((callback: () => void, delay?: number, ...args: any[]) => {
    if (delay === 60_000) { expire = callback; return 0 as unknown as ReturnType<typeof setTimeout>; }
    return original(callback, delay, ...args);
  }) as typeof setTimeout;
  try {
    const identity = device(); stall = true;
    const opening = openBrowserDevice(identity, settings);
    const rejected = expect(opening).rejects.toThrow('WASM did not initialize');
    for (let tick = 0; tick < 8 && !expire; tick++) await Promise.resolve();
    expect(expire).toBeTruthy(); expire!(); await rejected;
    expect(workers.every(worker => worker.terminated)).toBe(true);
    globalThis.setTimeout = original; stall = false;
    await expect(openBrowserDevice(identity, settings)).resolves.toBeTruthy();
  } finally { globalThis.setTimeout = original; }
});

test('cross-origin isolation is mandatory and never falls back to host execution', async () => {
  Object.defineProperty(globalThis, 'crossOriginIsolated', { value: false, configurable: true });
  await expect(openBrowserDevice(device(), settings)).rejects.toThrow('requires cross-origin isolation');
  expect(workers).toHaveLength(0);
});

test('a wallet watch sends Rust watch targets to the I/O worker and coalesces change hints', async () => {
  const runtime = await openBrowserDevice(device(), { ...settings, network: 'signet' as const });
  let targets = ['aa'];
  answer = message => message.command?.method === 'watch_targets' ? { scripthashes: targets } : {};
  const io = workers.find(worker => worker.url.includes('io-worker'))!;
  const watches = () => io.messages.filter(message => message.type === 'watch');
  const until = async (condition: () => boolean) => { for (let i = 0; i < 200 && !condition(); i++) await new Promise(r => setTimeout(r, 5)); expect(condition()).toBe(true); };
  const reasons: string[] = [];
  const stop = runtime.watch!(reason => reasons.push(reason));
  await until(() => watches().length === 1);
  expect(watches()[0].scripthashes).toEqual(['aa']);
  // A burst of hints from one block is delivered as a single refresh.
  for (const reason of ['block', 'address', 'address']) io.onmessage?.({ data: { type: 'chain-changed', reason } });
  await until(() => reasons.length === 1);
  expect(reasons).toEqual(['block,address']);
  // Commands re-read the targets; only a changed list is sent again.
  await runtime.execute({ method: 'wallet_snapshot' }); await new Promise(r => setTimeout(r, 400));
  expect(watches()).toHaveLength(1);
  targets = ['aa', 'bb'];
  await runtime.execute({ method: 'receive_address' });
  await until(() => watches().length === 2);
  stop();
  expect(io.messages.at(-1)).toEqual({ type: 'unwatch' });
});

test('signer devices have no chain watch', async () => {
  const runtime = await openBrowserDevice({ ...device(), role: 'signer' }, settings);
  runtime.watch!(() => {})();
  expect(workers.find(worker => worker.url.includes('io-worker'))!.messages.some(message => message.type === 'watch')).toBe(false);
});

test('transports without script subscriptions start watching without asking the device', async () => {
  const runtime = await openBrowserDevice(device(), settings);
  const worker = workers.find(w => w.url.includes('device-worker'))!, io = workers.find(w => w.url.includes('io-worker'))!;
  runtime.watch!(() => {});
  await new Promise(r => setTimeout(r, 50));
  await runtime.execute({ method: 'receive_address' }); await new Promise(r => setTimeout(r, 400));
  expect(io.messages.filter(message => message.type === 'watch')).toEqual([{ type: 'watch', scripthashes: [] }]);
  expect(worker.messages.some(message => message.command?.method === 'watch_targets')).toBe(false);
});
