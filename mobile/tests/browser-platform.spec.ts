import { test, expect } from '@playwright/test';

// Real browser IndexedDB/WebCrypto and workers. These do not mock wallet algorithms.
test.beforeEach(async ({ page }) => { await page.goto('http://localhost:8081'); });

test('device records are encrypted, role-bound, path-bound and survive reopening', async ({ page }) => {
  const result = await page.evaluate(async () => {
    const modulePath = '/runtime/storage.mjs';
    const { openDeviceStore } = await import(modulePath);
    const id = '01'.repeat(32), name = `anzen-device-v1-${id}`;
    let store = await openDeviceStore(id, 'wallet');
    await store.execute({ method: 'write', path: '/device/phone/key.json', data: btoa('test secret') });
    store.close();
    store = await openDeviceStore(id, 'wallet');
    const read = await store.execute({ method: 'read', path: '/device/phone/key.json' });
    const badPath = await store.execute({ method: 'read', path: '/device/../other/key.json' });
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const req = indexedDB.open(name); req.onsuccess = () => resolve(req.result); req.onerror = () => reject(req.error);
    });
    const get = (table: string, key: string) => new Promise<any>(resolve => {
      const req = db.transaction(table).objectStore(table).get(key); req.onsuccess = () => resolve(req.result);
    });
    const record = await get('files', '/device/phone/key.json');
    const identity = await get('metadata', 'identity');
    await new Promise<void>((resolve, reject) => {
      const tx = db.transaction('files', 'readwrite');
      tx.objectStore('files').put({ ...record, path: '/device/phone/swapped.json' });
      tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error);
    });
    const swapped = await store.execute({ method: 'read', path: '/device/phone/swapped.json' });
    let roleError = '';
    try { await openDeviceStore(id, 'signer'); } catch (error) { roleError = String(error); }
    store.close(); db.close();
    return { read: atob(read), badPath, swapped, roleError, extractable: identity.key.extractable,
      recordKeys: Object.keys(record).sort(), plaintextVisible: new TextDecoder().decode(record.ciphertext).includes('test secret') };
  });
  expect(result.read).toBe('test secret');
  expect(result.extractable).toBe(false);
  expect(result.plaintextVisible).toBe(false);
  expect(result.recordKeys).toEqual(['ciphertext', 'directory', 'iv', 'path']);
  expect(result.badPath.error.kind).toBe('permission_denied');
  expect(result.swapped.error).toBeTruthy();
  expect(result.roleError).toContain('role mismatch');
});

test('request success followed by transaction abort never acknowledges a write or partial rename', async ({ page }) => {
  const result = await page.evaluate(async () => {
    const modulePath = '/runtime/storage.mjs';
    const { openDeviceStore } = await import(modulePath);
    const store = await openDeviceStore('02'.repeat(32), 'wallet');
    const file = '/device/phone/key.json';
    await store.execute({ method: 'write', path: file, data: btoa('original') });
    const put = IDBObjectStore.prototype.put;
    let requestSucceeded = 0;
    IDBObjectStore.prototype.put = function (...args: Parameters<typeof put>) {
      const req = put.apply(this, args);
      req.addEventListener('success', () => { requestSucceeded++; this.transaction.abort(); });
      return req;
    };
    const failedWrite = await store.execute({ method: 'write', path: file, data: btoa('replacement') });
    const failedRename = await store.execute({ method: 'rename', path: '/device/phone', destination: '/device/archive' });
    IDBObjectStore.prototype.put = put;
    const stillOriginal = atob(await store.execute({ method: 'read', path: file }));
    const absent = await store.execute({ method: 'exists', path: '/device/archive' });
    const renamed = await store.execute({ method: 'rename', path: '/device/phone', destination: '/device/archive' });
    const moved = atob(await store.execute({ method: 'read', path: '/device/archive/key.json' }));
    const oldAbsent = await store.execute({ method: 'exists', path: file });
    store.close();
    return { requestSucceeded, failedWrite, failedRename, stillOriginal, absent, renamed, moved, oldAbsent };
  });
  expect(result.requestSucceeded).toBeGreaterThanOrEqual(2);
  expect(result.failedWrite.error).toBeTruthy(); expect(result.failedRename.error).toBeTruthy();
  expect(result.stillOriginal).toBe('original'); expect(result.absent).toBe(false);
  expect(result.renamed).toBeNull(); expect(result.moved).toBe('original'); expect(result.oldAbsent).toBe(false);
});

test('missing browser encryption key fails closed without replacing wallet records', async ({ page }) => {
  const result = await page.evaluate(async () => {
    const modulePath = '/runtime/storage.mjs';
    const { openDeviceStore } = await import(modulePath);
    const id = '03'.repeat(32), store = await openDeviceStore(id, 'wallet');
    await store.execute({ method: 'write', path: '/device/phone/key.json', data: btoa('keep me') });
    store.close();
    const db = await new Promise<IDBDatabase>(resolve => { const req = indexedDB.open(`anzen-device-v1-${id}`); req.onsuccess = () => resolve(req.result); });
    await new Promise<void>(resolve => { const tx = db.transaction('metadata', 'readwrite'); tx.objectStore('metadata').delete('identity'); tx.oncomplete = () => resolve(); });
    let error = '';
    try { await openDeviceStore(id, 'wallet'); } catch (e) { error = String(e); }
    const count = await new Promise<number>(resolve => { const req = db.transaction('files').objectStore('files').count(); req.onsuccess = () => resolve(req.result); });
    const keys = await new Promise<number>(resolve => { const req = db.transaction('metadata').objectStore('metadata').count(); req.onsuccess = () => resolve(req.result); });
    db.close(); return { error, count, keys };
  });
  expect(result.error).toContain('refusing to replace'); expect(result.count).toBeGreaterThan(0); expect(result.keys).toBe(0);
});

test('a device lock prevents concurrent writers in a second worker', async ({ page }) => {
  const result = await page.evaluate(async () => {
    const settings = { id: '04'.repeat(32), role: 'wallet', allowMainnet: false, endpoint: 'http://127.0.0.1:1', capability: '' };
    const workers: Worker[] = [];
    async function open() {
      const worker = new Worker('/runtime/io-worker.mjs', { type: 'module' }); workers.push(worker);
      const channel = new MessageChannel();
      return new Promise<any>(resolve => {
        worker.onmessage = ({ data }) => resolve(data);
        worker.postMessage({ type: 'configure', settings, port: channel.port1 }, [channel.port1]);
      });
    }
    const first = await open(), second = await open(); workers.forEach(worker => worker.terminate());
    return { first, second };
  });
  expect(result.first.ready).toBe(true); expect(result.second.error).toContain('open in another tab');
});
