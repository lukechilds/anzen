// Browser transport only. Wallet commands execute in a device-local WASM worker, not HTTP.
import { DeviceTransport, JsonObject, Role } from './transport';

export interface BrowserDevice { id: string; role: Role }
export interface Settings {
  endpoint: string; capability: string; allowMainnet: boolean;
  // Regtest uses the playground host at `endpoint`; Mutinynet and mainnet use Electrum.
  network?: 'regtest' | 'bitcoin' | 'signet';
}
const runtimes = new Map<string, BrowserRuntime>();
function randomId() { return [...crypto.getRandomValues(new Uint8Array(32))].map(b => b.toString(16).padStart(2, '0')).join(''); }
function result<T>(request: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => { request.onsuccess = () => resolve(request.result); request.onerror = () => reject(request.error); });
}
async function catalog() {
  const request = indexedDB.open('anzen-playground-catalog-v1', 1);
  request.onupgradeneeded = () => request.result.createObjectStore('devices', { keyPath: 'id' });
  return result(request);
}
async function session(capability: string) {
  return [...new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(capability)))].map(b => b.toString(16).padStart(2, '0')).join('');
}
export async function listBrowserDevices(capability: string): Promise<BrowserDevice[]> {
  const scope = await session(capability), db = await catalog();
  try {
    const devices = await result(db.transaction('devices').objectStore('devices').getAll());
    return devices.filter(device => device.session === scope).map(({ id, role }) => ({ id, role }));
  } finally { db.close(); }
}
export async function createBrowserDevice(role: Role, settings: Settings): Promise<BrowserDevice> {
  const device = { id: randomId(), role }, scope = await session(settings.capability);
  await openBrowserDevice(device, settings);
  let db: IDBDatabase | undefined;
  try {
    db = await catalog();
    const tx = db.transaction('devices', 'readwrite', { durability: 'strict' });
    const done = new Promise<void>((resolve, reject) => { tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error); });
    tx.objectStore('devices').add({ ...device, session: scope });
    await done;
  } catch (error) {
    runtimes.get(device.id)?.close(); runtimes.delete(device.id);
    throw error;
  } finally { db?.close(); }
  return device;
}
export async function openBrowserDevice(device: BrowserDevice, settings: Settings): Promise<DeviceTransport> {
  const existing = runtimes.get(device.id);
  if (existing) {
    if (existing.role !== device.role) throw new Error('Browser device role mismatch');
    if (!existing.matches(settings)) throw new Error('Browser device connection settings changed; reload before reopening it');
    await existing.ready; return existing;
  }
  if (!globalThis.crossOriginIsolated || typeof SharedArrayBuffer === 'undefined') {
    throw new Error('Browser WASM requires cross-origin isolation. Serve the static site with COOP: same-origin and COEP: require-corp (or use npm run web locally).');
  }
  const runtime = new BrowserRuntime(device, settings);
  runtimes.set(device.id, runtime);
  try { await runtime.ready; return runtime; }
  catch (error) { runtimes.delete(device.id); runtime.close(); throw error; }
}
// Stop a device's workers, e.g. when switching networks. Opening it again starts afresh.
export function closeBrowserDevice(id: string) {
  runtimes.get(id)?.close();
  runtimes.delete(id);
}
class BrowserRuntime implements DeviceTransport {
  readonly role: Role;
  readonly ready: Promise<JsonObject>;
  private worker: Worker;
  private io: Worker;
  private closed = false;
  private sequence = 0;
  private pending = new Map<number, { resolve: (value: JsonObject) => void; reject: (error: Error) => void }>();
  private listeners = new Set<(reason: string) => void>();
  private watched?: string;
  private targetsTimer?: ReturnType<typeof setTimeout>;
  private changeTimer?: ReturnType<typeof setTimeout>;
  private changeReasons = new Set<string>();
  matches(settings: Settings) { return settings.endpoint === this.settings.endpoint && settings.capability === this.settings.capability && settings.allowMainnet === this.settings.allowMainnet && settings.network === this.settings.network; }
  constructor(device: BrowserDevice, private readonly settings: Settings) {
    this.role = device.role;
    this.worker = new Worker('/runtime/device-worker.mjs', { type: 'module' });
    try { this.io = new Worker('/runtime/io-worker.mjs', { type: 'module' }); }
    catch (error) { this.worker.terminate(); throw error; }
    this.worker.onmessage = ({ data }) => {
      const pending = this.pending.get(data.id);
      if (!pending) return;
      this.pending.delete(data.id);
      if ('error' in data) pending.reject(new Error(data.error)); else pending.resolve(data.ok);
      if (data.fatal) this.close();
    };
    let rejectInitialization: ((error: Error) => void) | undefined;
    this.worker.onerror = () => { rejectInitialization?.(new Error('Browser WASM worker failed')); this.close(); };
    const channel = new MessageChannel();
    const configuration = { ...device, ...settings };
    this.ready = new Promise<void>((resolve, reject) => {
      const timer = setTimeout(() => reject(new Error('Browser storage worker did not initialize')), 20_000);
      rejectInitialization = error => { clearTimeout(timer); reject(error); };
      this.io.onerror = () => { clearTimeout(timer); reject(new Error('Browser storage worker failed')); this.close(); };
      this.io.onmessage = ({ data }) => {
        if (data.type === 'chain-changed') { this.changed(String(data.reason)); return; }
        clearTimeout(timer);
        if (data.error) reject(new Error(data.error)); else if (data.ready) resolve();
      };
      this.io.postMessage({ type: 'configure', settings: configuration, port: channel.port2 }, [channel.port2]);
    }).then(() => this.request({ type: 'initialize', settings: configuration, port: channel.port1 }, [channel.port1], 60_000));
  }
  private request(message: object, transfer: Transferable[] = [], timeoutMs?: number) {
    return new Promise<JsonObject>((resolve, reject) => {
      if (this.closed) { reject(new Error('Browser device interrupted; reload before retrying')); return; }
      const id = ++this.sequence;
      const timer = timeoutMs ? setTimeout(() => {
        this.pending.delete(id); reject(new Error('Browser WASM did not initialize; check its build and reload'));
        this.close();
      }, timeoutMs) : undefined;
      this.pending.set(id, {
        resolve: value => { clearTimeout(timer); resolve(value); },
        reject: error => { clearTimeout(timer); reject(error); },
      });
      try { this.worker.postMessage({ ...message, id }, transfer); }
      catch (error) { this.pending.delete(id); clearTimeout(timer); reject(error); }
    });
  }
  async execute(command: JsonObject): Promise<JsonObject> {
    await this.ready;
    const result = await this.request({ type: 'execute', command });
    // Commands can reveal addresses or pair a vault, so re-read what Electrum should watch.
    if (this.listeners.size && this.subscribes && command.method !== 'watch_targets') this.scheduleTargets();
    return result;
  }
  watch(listener: (reason: string) => void) {
    if (this.role !== 'wallet' || this.closed) return () => {};
    this.listeners.add(listener);
    if (this.listeners.size === 1) this.scheduleTargets(0);
    return () => {
      this.listeners.delete(listener);
      if (this.listeners.size || this.closed) return;
      clearTimeout(this.targetsTimer); clearTimeout(this.changeTimer);
      this.watched = undefined; this.io.postMessage({ type: 'unwatch' });
    };
  }
  private scheduleTargets(delay = 250) {
    clearTimeout(this.targetsTimer);
    this.targetsTimer = setTimeout(() => void this.updateTargets(), delay);
  }
  // Only Electrum subscribes to scripts. Other transports watch the whole chain, so they need
  // no watch list and no extra device command after every action.
  private get subscribes() { return this.settings.network === 'signet' || this.settings.network === 'bitcoin'; }
  private async updateTargets() {
    if (!this.listeners.size || this.closed) return;
    if (!this.subscribes) {
      if (this.watched === undefined) { this.watched = ''; this.io.postMessage({ type: 'watch', scripthashes: [] }); }
      return;
    }
    try {
      await this.ready;
      const { scripthashes } = await this.request({ type: 'execute', command: { method: 'watch_targets' } });
      const key = scripthashes.join();
      if (key === this.watched || !this.listeners.size || this.closed) return;
      this.watched = key;
      this.io.postMessage({ type: 'watch', scripthashes });
    } catch { /* No wallet key yet; the command that creates one schedules another attempt. */ }
  }
  // One block can touch many watched scripts; deliver a burst as a single refresh.
  private changed(reason: string) {
    if (!this.listeners.size) return;
    this.changeReasons.add(reason);
    clearTimeout(this.changeTimer);
    this.changeTimer = setTimeout(() => {
      const reasons = [...this.changeReasons].join(','); this.changeReasons.clear();
      this.listeners.forEach(listener => listener(reasons));
    }, 300);
  }
  private fail(error: Error) { this.pending.forEach(request => request.reject(error)); this.pending.clear(); }
  close() {
    this.closed = true; this.listeners.clear(); clearTimeout(this.targetsTimer); clearTimeout(this.changeTimer);
    this.fail(new Error('Browser device closed; reload before retrying')); this.worker.terminate(); this.io.terminate();
  }
}
