import { test, expect } from '@playwright/test';
import { createElectrumTransport } from '../../public/runtime/electrum-ws.mjs';

// Socket mechanics only. Electrum framing, failover and validation are Rust (anzen-wallet).
class Socket extends EventTarget {
  static instances: Socket[] = [];
  static connect = true;
  closed = false;
  sent: string[] = [];
  url: string;
  constructor(url: string) {
    super(); this.url = url; Socket.instances.push(this);
    if (Socket.connect) queueMicrotask(() => this.dispatchEvent(new Event('open')));
  }
  send(text: string) { this.sent.push(text); }
  message(data: string | ArrayBuffer) { this.dispatchEvent(new MessageEvent('message', { data })); }
  close() { if (this.closed) return; this.closed = true; this.dispatchEvent(new Event('close')); }
}
const transport = () => createElectrumTransport({ WebSocketImpl: Socket as unknown as typeof WebSocket, connectTimeoutMs: 25, receiveTimeoutMs: 25 });
test.beforeEach(() => { Socket.instances = []; Socket.connect = true; });

test('only wss:// servers named by Rust are opened', async () => {
  const call = transport();
  for (const url of ['ws://example.com', 'https://example.com', 'not a url']) {
    await expect(call({ operation: 'open', url })).rejects.toThrow(/wss:\/\/|Invalid/);
  }
  const session = await call({ operation: 'open', url: 'wss://electrum.example' });
  expect(Socket.instances.map(s => s.url)).toEqual(['wss://electrum.example']);
  await call({ operation: 'close', session });
  expect(Socket.instances[0].closed).toBe(true);
});

test('frames pass through unchanged in both directions and in order', async () => {
  const call = transport(), session = await call({ operation: 'open', url: 'wss://a' });
  await call({ operation: 'send', session, frame: '[{"id":1},{"id":2}]\n' });
  expect(Socket.instances[0].sent).toEqual(['[{"id":1},{"id":2}]\n']);
  const socket = Socket.instances[0];
  socket.message('first'); socket.message(new TextEncoder().encode('second').buffer as ArrayBuffer);
  expect(await call({ operation: 'receive', session })).toBe('first');
  expect(await call({ operation: 'receive', session })).toBe('second');
  const waiting = call({ operation: 'receive', session });
  socket.message('third');
  expect(await waiting).toBe('third');
  await call({ operation: 'close', session });
});

test('timeouts, disconnects and stale sessions fail and close the socket', async () => {
  const call = transport(); Socket.connect = false;
  await expect(call({ operation: 'open', url: 'wss://a' })).rejects.toThrow('connection timed out');
  expect(Socket.instances[0].closed).toBe(true);
  Socket.connect = true;
  const session = await call({ operation: 'open', url: 'wss://b' });
  await expect(call({ operation: 'receive', session })).rejects.toThrow('request timed out');
  expect(Socket.instances[1].closed).toBe(true);
  await expect(call({ operation: 'send', session, frame: 'x' })).rejects.toThrow();
  const next = await call({ operation: 'open', url: 'wss://b' });
  await expect(call({ operation: 'receive', session })).rejects.toThrow('closed');
  const pending = call({ operation: 'receive', session: next });
  Socket.instances[2].close();
  await expect(pending).rejects.toThrow('disconnected');
});

test('oversized, non-UTF-8 and concurrent reads are refused', async () => {
  const call = transport();
  let session = await call({ operation: 'open', url: 'wss://a' });
  await expect(call({ operation: 'send', session, frame: 'x'.repeat(8 * 1024 * 1024 + 1) })).rejects.toThrow('limit');
  Socket.instances[0].message('x'.repeat(8 * 1024 * 1024 + 1));
  await expect(call({ operation: 'receive', session })).rejects.toThrow('8 MiB');
  session = await call({ operation: 'open', url: 'wss://a' });
  Socket.instances[1].message(new Uint8Array([0xff, 0xfe]).buffer as ArrayBuffer);
  await expect(call({ operation: 'receive', session })).rejects.toThrow('UTF-8');
  session = await call({ operation: 'open', url: 'wss://a' });
  const first = call({ operation: 'receive', session });
  await expect(call({ operation: 'receive', session })).rejects.toThrow('Concurrent');
  Socket.instances[2].message('ok'); expect(await first).toBe('ok');
});

test('the listener socket reports open, frames and a single close', async () => {
  const events: string[] = [];
  const call = transport();
  const socket = call.listen('wss://a', { opened: () => events.push('opened'), frame: (t: string) => events.push(t), closed: () => events.push('closed') });
  await new Promise(r => setTimeout(r, 5));
  socket.send('{"id":1}\n');
  expect(Socket.instances[0].sent).toEqual(['{"id":1}\n']);
  Socket.instances[0].message('notification');
  Socket.instances[0].close(); Socket.instances[0].close();
  expect(events).toEqual(['opened', 'notification', 'closed']);
  // A connection that never opens reports closed; closing it ourselves reports nothing.
  Socket.connect = false;
  call.listen('wss://b', { opened: () => events.push('opened b'), frame: () => {}, closed: () => events.push('closed b') });
  await new Promise(r => setTimeout(r, 40));
  expect(events.at(-1)).toBe('closed b');
  Socket.connect = true;
  const own = call.listen('wss://c', { opened: () => {}, frame: () => {}, closed: () => events.push('closed c') });
  own.close();
  await new Promise(r => setTimeout(r, 5));
  expect(events).not.toContain('closed c');
  expect(() => call.listen('ws://d', { opened: () => {}, frame: () => {}, closed: () => {} })).toThrow('wss://');
});
