// An opaque object store, not wallet logic. Rust creates/authenticates/decrypts backups.
// Kept separate from simulated device databases so replacing a device preserves backups.
import { parseEncryptedBackup } from './fileExchange';
import { JsonObject } from './transport';

const hash = async (text: string) => [...new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(text)))].map(b => b.toString(16).padStart(2, '0')).join('');
function request<T>(value: IDBRequest<T>): Promise<T> {
  return new Promise((resolve, reject) => { value.onsuccess = () => resolve(value.result); value.onerror = () => reject(value.error); });
}
async function database() {
  const opening = indexedDB.open('anzen-simulated-cloud-v1', 1);
  opening.onupgradeneeded = () => {
    const store = opening.result.createObjectStore('backups', { keyPath: ['scope', 'id'] });
    store.createIndex('scope', 'scope');
  };
  return request(opening);
}
export async function browserCloud(scope: string, operation: JsonObject): Promise<JsonObject> {
  const namespace = await hash(scope), db = await database();
  try {
    if (operation.method === 'save') {
      const text = JSON.stringify(operation.backup);
      const backup = parseEncryptedBackup(text);
      if (backup.version !== 2 || backup.kind !== 'vault-cloud-recovery') throw new Error('Unsupported encrypted cloud backup');
      const id = await hash(text), tx = db.transaction('backups', 'readwrite', { durability: 'strict' });
      const done = new Promise<void>((resolve, reject) => { tx.oncomplete = () => resolve(); tx.onabort = () => reject(tx.error || new Error('Backup write aborted')); });
      tx.objectStore('backups').put({ scope: namespace, id, text });
      await done; return { id };
    }
    const store = db.transaction('backups').objectStore('backups');
    if (operation.method === 'list') {
      const objects = await request(store.index('scope').getAll(namespace));
      return { backups: objects.map(value => value.id).sort() };
    }
    if (operation.method === 'load') {
      if (!/^[a-f0-9]{64}$/.test(operation.id)) throw new Error('Invalid backup ID');
      const object = await request(store.get([namespace, operation.id]));
      if (!object) throw new Error('Encrypted backup not found in this browser');
      if (await hash(object.text) !== operation.id) throw new Error('Cloud object failed its content hash check');
      return parseEncryptedBackup(object.text);
    }
    throw new Error('Unsupported cloud operation');
  } finally { db.close(); }
}
