import { test, expect } from '@playwright/test';
import { fileExchange, MAX_TRANSFER_BYTES, parseEncryptedBackup, parseProtocolMessage, readTransfer } from '../../src/fileExchange';
import type { FileExchangeIO, SelectedFile } from '../../src/fileExchange';
import { protocolKinds } from '../../src/transport';
import type { ProtocolMessage } from '../../src/transport';

const message: ProtocolMessage = { kind: 'policy-proposal', body: { opaque: 'Rust validates this, not the file courier 🔒' } };
// Deliberately not a valid cryptographic backup: the courier must not pretend to
// authenticate/decrypt it. Those operations are tested through the Rust facades.
const backup = { encrypted_payload: { ciphertext: 'opaque' } };

function selected(bytes: Uint8Array, options: { size?: number; chunk?: number; failRead?: boolean } = {}) {
  let offset = 0;
  const state = { opens: 0, closes: 0, requested: 0, read: 0 };
  const file: SelectedFile = {
    size: options.size,
    open() {
      state.opens++;
      return {
        readBytes(length) {
          state.requested += length;
          if (options.failRead) throw new Error('reader failed');
          const result = bytes.slice(offset, offset + Math.min(length, options.chunk ?? length));
          offset += result.length; state.read += result.length;
          return result;
        },
        close() { state.closes++; },
      };
    },
  };
  return { file, state };
}

function fixture(text: string | null = JSON.stringify(message)) {
  const source = selected(new TextEncoder().encode(text ?? ''));
  const events: string[] = [];
  const exports: { channel: string; text: string; uri: string }[] = [];
  const io: FileExchangeIO = {
    async pick() { events.push('pick'); return text === null ? null : source.file; },
    async sharingAvailable() { events.push('available'); return true; },
    async createExport(channel, text) {
      events.push('create');
      const uri = `file:///private-cache/export-${exports.length}.json`;
      exports.push({ channel, text, uri }); return uri;
    },
    async share(uri, title) { events.push(`share ${uri} ${title}`); },
  };
  return { ...source, events, exports, io, courier: fileExchange(io) };
}

test('all supported kinds round-trip their opaque body, with no wallet commands', async () => {
  for (const kind of protocolKinds) {
    const original = { ...message, kind };
    const f = fixture(JSON.stringify(original));
    expect(await f.courier.receiveMessage()).toEqual(original);
    expect(f.events).toEqual(['pick']);
    expect(f.state.opens).toBe(1); expect(f.state.closes).toBe(1);
    expect(f.exports).toEqual([]);
  }
});

for (const [name, value] of Object.entries({
  null: null, array: [], scalar: 'seed words', unknown: { kind: 'approve-everything', body: {} },
  missingBody: { kind: 'policy-proposal' }, arrayBody: { kind: 'policy-proposal', body: [] },
  extraPayload: { ...message, encrypted_payload: backup.encrypted_payload }, backup,
})) {
  test(`rejects wrong protocol framing: ${name}`, () => {
    expect(() => parseProtocolMessage(JSON.stringify(value))).toThrow('Not an Anzen protocol message');
  });
}

test('invalid JSON does not echo potentially private input in the error', () => {
  expect(() => parseProtocolMessage('accidentally pasted private seed words')).toThrow('The selected content is not valid JSON.');
  expect(() => parseEncryptedBackup('accidentally pasted private seed words')).toThrow('The selected content is not valid JSON.');
});

test('backup channel is separate and wallet-only, before opening any file picker', async () => {
  const f = fixture(JSON.stringify(backup));
  await expect(f.courier.receiveBackup('signer')).rejects.toThrow('not the signer');
  await expect(f.courier.saveBackup('signer', backup)).rejects.toThrow('not the signer');
  expect(f.events).toEqual([]);
  expect(await f.courier.receiveBackup('wallet')).toEqual(backup);
  expect(() => parseEncryptedBackup(JSON.stringify(message))).toThrow('Protocol messages must be imported separately');
  expect(() => parseEncryptedBackup(JSON.stringify({ encrypted_payload: [] }))).toThrow('Not an encrypted backup');
});

test('picker cancellation returns no new message or backup and never reads or exports', async () => {
  const f = fixture(null);
  expect(await f.courier.receiveMessage()).toBeNull();
  expect(await f.courier.receiveBackup('wallet')).toBeNull();
  expect(f.state.opens).toBe(0); expect(f.exports).toEqual([]);
});

test('sharing exports exact JSON; chooser completion is only offered, never delivered', async () => {
  const f = fixture();
  expect(await f.courier.sendMessage(message)).toBe('offered');
  expect(f.exports[0]).toMatchObject({ channel: 'protocol', text: JSON.stringify(message) });
  expect(f.events).toEqual(['available', 'create', expect.stringContaining('Anzen protocol message')]);
  expect(message.body.opaque).toContain('Rust validates');
  // Sharing the same request again is a new file copy, not another signature.
  expect(await f.courier.sendMessage(message)).toBe('offered');
  expect(f.exports[1].text).toBe(f.exports[0].text);
  expect(f.exports[1].uri).not.toBe(f.exports[0].uri);
  expect(await f.courier.saveBackup('wallet', backup)).toBe('offered');
  expect(f.exports[2]).toMatchObject({ channel: 'backup', text: JSON.stringify(backup) });
  expect(f.events.at(-1)).toContain('save separately from the signer');
});

test('unavailable sharing creates no file and failed sharing can be retried', async () => {
  const f = fixture();
  f.io.sharingAvailable = async () => false;
  await expect(f.courier.sendMessage(message)).rejects.toThrow('unavailable');
  expect(f.exports).toEqual([]);
  f.io.sharingAvailable = async () => true;
  f.io.share = async () => { throw new Error('share failed'); };
  await expect(f.courier.sendMessage(message)).rejects.toThrow('share failed');
  f.io.share = async () => {};
  expect(await f.courier.sendMessage(message)).toBe('offered');
  expect(f.exports.map(item => item.text)).toEqual([JSON.stringify(message), JSON.stringify(message)]);
});

test('picker and export write errors propagate without claiming a transfer', async () => {
  const f = fixture();
  f.io.pick = async () => { throw new Error('picker failed'); };
  await expect(f.courier.receiveMessage()).rejects.toThrow('picker failed');
  f.io.createExport = async () => { throw new Error('disk full'); };
  await expect(f.courier.sendMessage(message)).rejects.toThrow('disk full');
  expect(f.events.some(event => event.startsWith('share '))).toBe(false);
});

test('short reads and UTF-8 characters crossing chunk boundaries round-trip exactly', () => {
  const text = JSON.stringify(message);
  const f = selected(new TextEncoder().encode(text), { chunk: 1 });
  expect(readTransfer(f.file)).toBe(text);
  expect(f.state.closes).toBe(1);
});

test('oversized provider metadata rejects before opening a handle', () => {
  const f = selected(new Uint8Array(), { size: MAX_TRANSFER_BYTES + 1 });
  expect(() => readTransfer(f.file)).toThrow('8 MiB');
  expect(f.state.opens).toBe(0);
});

test('incorrect or missing metadata cannot bypass the byte limit', () => {
  for (const size of [undefined, 1]) {
    const f = selected(new Uint8Array(MAX_TRANSFER_BYTES + 100_000), { size });
    expect(() => readTransfer(f.file)).toThrow('8 MiB');
    expect(f.state.read).toBe(MAX_TRANSFER_BYTES + 1);
    expect(f.state.requested).toBe(MAX_TRANSFER_BYTES + 1);
    expect(f.state.closes).toBe(1);
  }
});

test('the exact byte boundary is accepted; Unicode paste/export uses bytes, not string length', async () => {
  // Construct a valid message then pad to the exact wire-byte boundary.
  const text = JSON.stringify(message);
  const padded = text + ' '.repeat(MAX_TRANSFER_BYTES - new TextEncoder().encode(text).length);
  const f = selected(new TextEncoder().encode(padded));
  expect(new TextEncoder().encode(readTransfer(f.file)).length).toBe(MAX_TRANSFER_BYTES);
  expect(parseProtocolMessage(padded)).toEqual(message);
  const unicode = { ...message, body: { text: '🔒'.repeat(MAX_TRANSFER_BYTES / 4) } };
  expect(JSON.stringify(unicode).length).toBeLessThan(MAX_TRANSFER_BYTES);
  expect(() => parseProtocolMessage(JSON.stringify(unicode))).toThrow('8 MiB');
  const sharing = fixture();
  await expect(sharing.courier.sendMessage(unicode)).rejects.toThrow('8 MiB');
  expect(sharing.events).toEqual([]);
});

test('invalid UTF-8 and read errors close the handle without changing the source', () => {
  const invalid = selected(new Uint8Array([0xC3, 0x28]));
  expect(() => readTransfer(invalid.file)).toThrow('not valid UTF-8');
  expect(invalid.state.closes).toBe(1);
  const broken = selected(new Uint8Array(), { failRead: true });
  expect(() => readTransfer(broken.file)).toThrow('reader failed');
  expect(broken.state.closes).toBe(1);
});
