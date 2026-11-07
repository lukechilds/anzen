// Platform storage only: no keys are derived, no policy is interpreted, no transactions
// are constructed here. Each device has its own database and non-exportable wrapping key.
// Same-origin scripts can still use that key; this is not native Keychain/Keystore security.

const encode = new TextEncoder();
function request(req) {
  return new Promise((resolve, reject) => {
    req.onsuccess = () => resolve(req.result);
    req.onerror = () => reject(req.error);
  });
}
function committed(tx) {
  return new Promise((resolve, reject) => {
    tx.oncomplete = () => resolve();
    tx.onabort = () => reject(tx.error || new Error('Browser storage transaction aborted'));
    tx.onerror = () => {}; // onabort is the authoritative failure, not one request's event.
  });
}
function fail(kind, message) { throw Object.assign(new Error(message), { kind }); }
function path(value) {
  if (typeof value !== 'string' || !/^\/(device|temporary)(\/[^/]+)*$/.test(value)
      || value.includes('\0') || value.split('/').some(p => p === '.' || p === '..')) {
    fail('permission_denied', 'Invalid device-local storage path');
  }
  return value;
}
function bytes(base64) { return Uint8Array.from(atob(base64), c => c.charCodeAt(0)); }
function base64(data) {
  let result = '';
  for (let offset = 0; offset < data.length; offset += 16384) result += String.fromCharCode(...data.subarray(offset, offset + 16384));
  return btoa(result);
}

export async function openDeviceStore(id, role) {
  if (!/^[a-f0-9]{64}$/.test(id) || !['wallet', 'signer'].includes(role)) throw new Error('Invalid browser device identity');
  const opening = indexedDB.open(`anzen-device-v1-${id}`, 1);
  opening.onupgradeneeded = () => {
    opening.result.createObjectStore('files', { keyPath: 'path' });
    opening.result.createObjectStore('metadata');
  };
  const db = await request(opening);
  const read = db.transaction(['metadata', 'files'], 'readonly');
  const readDone = committed(read);
  const [identity, records] = await Promise.all([
    request(read.objectStore('metadata').get('identity')),
    request(read.objectStore('files').getAll()),
  ]);
  await readDone;
  let key = identity?.key;
  if (identity && identity.role !== role) throw new Error('Browser device role mismatch');
  if (!key) {
    if (identity || records.length) throw new Error('Browser storage key is missing; refusing to replace it');
    key = await crypto.subtle.generateKey({ name: 'AES-GCM', length: 256 }, false, ['encrypt', 'decrypt']);
    const tx = db.transaction('metadata', 'readwrite', { durability: 'strict' });
    const done = committed(tx);
    tx.objectStore('metadata').add({ role, key }, 'identity');
    await done;
  }
  const aad = p => encode.encode(`anzen.browser.storage.v1:${id}:${role}:${p}`);
  async function encrypt(p, data) {
    const iv = crypto.getRandomValues(new Uint8Array(12));
    return { path: p, directory: false, iv, ciphertext: await crypto.subtle.encrypt({ name: 'AES-GCM', iv, additionalData: aad(p) }, key, data) };
  }
  async function decrypt(record) {
    return new Uint8Array(await crypto.subtle.decrypt({ name: 'AES-GCM', iv: record.iv, additionalData: aad(record.path) }, key, record.ciphertext));
  }
  async function snapshot() {
    const tx = db.transaction('files', 'readonly');
    const done = committed(tx);
    const values = await request(tx.objectStore('files').getAll());
    await done;
    return new Map(values.map(value => [value.path, value]));
  }
  async function persist(puts, deletes = []) {
    const tx = db.transaction('files', 'readwrite', { durability: 'strict' });
    const done = committed(tx);
    const files = tx.objectStore('files');
    for (const p of deletes) files.delete(p);
    for (const value of puts) files.put(value);
    await done; // Never acknowledge a write on IDBRequest.onsuccess alone.
  }
  function parents(p, files, includeSelf = false) {
    const pieces = p.split('/').slice(1);
    if (!includeSelf) pieces.pop();
    const result = [];
    let current = '';
    for (const piece of pieces) {
      current += `/${piece}`;
      const old = files.get(current);
      if (old && !old.directory) fail('already_exists', 'Storage parent is not a directory');
      if (!old) result.push({ path: current, directory: true });
    }
    return result;
  }
  return {
    async execute(command) {
      try {
        const p = path(command.path), files = await snapshot(), current = files.get(p);
        switch (command.method) {
          case 'exists': return !!current;
          case 'is_file': return !!current && !current.directory;
          case 'read': {
            if (!current) fail('not_found', 'Device-local record does not exist');
            if (current.directory) fail('permission_denied', 'Cannot read a directory');
            return base64(await decrypt(current));
          }
          case 'list': {
            if (!current) fail('not_found', 'Device-local directory does not exist');
            if (!current.directory) fail('permission_denied', 'Cannot list a file');
            return [...files.values()].filter(v => v.path.startsWith(p + '/') && !v.path.slice(p.length + 1).includes('/'))
              .map(v => ({ path: v.path, directory: v.directory }));
          }
          case 'write': {
            if (current?.directory) fail('already_exists', 'Cannot replace a directory with a file');
            const content = bytes(command.data);
            if (content.length > 8 * 1024 * 1024) fail('permission_denied', 'Device record exceeds 8 MiB');
            await persist([...parents(p, files), await encrypt(p, content)]);
            return null;
          }
          case 'mkdir': await persist(parents(p, files, true)); return null;
          case 'remove_file': {
            if (!current) fail('not_found', 'Device-local record does not exist');
            if (current.directory) fail('permission_denied', 'Cannot remove a directory as a file');
            await persist([], [p]); return null;
          }
          case 'remove_dir': {
            if (!current) fail('not_found', 'Device-local directory does not exist');
            if (!current.directory) fail('permission_denied', 'Not a directory');
            await persist([], [...files.keys()].filter(k => k === p || k.startsWith(p + '/'))); return null;
          }
          case 'rename': {
            const destination = path(command.destination);
            if (!current) fail('not_found', 'Rename source does not exist');
            if (destination === p) return null;
            if (destination.startsWith(p + '/')) fail('permission_denied', 'Cannot move a directory inside itself');
            const existing = files.get(destination);
            if (existing && (existing.directory || current.directory)) fail('already_exists', 'Rename destination already exists');
            const moving = [...files.values()].filter(v => v.path === p || v.path.startsWith(p + '/'));
            const puts = parents(destination, files);
            for (const record of moving) {
              const target = destination + record.path.slice(p.length);
              puts.push(record.directory ? { path: target, directory: true } : await encrypt(target, await decrypt(record)));
            }
            await persist(puts, moving.map(v => v.path)); return null;
          }
          default: fail('permission_denied', 'Unsupported storage operation');
        }
      } catch (error) {
        return { error: { kind: error.kind || 'other', message: error.message || 'Browser storage failed' } };
      }
    },
    close() { db.close(); },
  };
}
