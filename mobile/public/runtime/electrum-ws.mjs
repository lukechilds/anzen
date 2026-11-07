// WebSocket mechanics only. Rust chooses servers, frames Electrum JSON-RPC, validates chain data
// and runs BDK; this module opens the socket it names and passes whole text frames either way.
const MAX_BYTES = 8 * 1024 * 1024;

function checkUrl(url) {
  let parsed;
  try { parsed = new URL(url); } catch { throw new Error('Invalid Electrum server URL'); }
  if (parsed.protocol !== 'wss:') throw new Error('Electrum servers must use wss://');
}

export function createElectrumTransport({
  WebSocketImpl = WebSocket,
  connectTimeoutMs = 8_000,
  receiveTimeoutMs = 15_000,
} = {}) {
  let current, sequence = 0;
  // Synchronous device calls: open, send, receive the next frame, close. One sync session at a time.
  const call = async call => {
    if (call.operation === 'open') {
      checkUrl(call.url);
      current?.close();
      current = new Socket(new WebSocketImpl(call.url), connectTimeoutMs);
      current.id = ++sequence;
      await current.ready;
      return current.id;
    }
    if (!current || call.session !== current.id) throw new Error('Electrum session is closed');
    if (call.operation === 'close') { current.close(); current = undefined; return null; }
    if (call.operation === 'send') { current.send(call.frame); return null; }
    if (call.operation === 'receive') return current.receive(receiveTimeoutMs);
    throw new Error('Unknown Electrum operation');
  };
  // The live-update listener's own socket, driven by events rather than device calls.
  call.listen = (url, { opened, frame, closed }) => {
    checkUrl(url);
    const socket = new Socket(new WebSocketImpl(url), connectTimeoutMs);
    let reported = false;
    const close = () => { if (!reported) { reported = true; closed(); } };
    socket.ready.then(opened, close);
    socket.onFrame = frame;
    socket.onClose = close;
    return { send: text => { try { socket.send(text); } catch { socket.close(); } }, close: () => { reported = true; socket.close(); } };
  };
  return call;
}

class Socket {
  constructor(socket, connectTimeoutMs) {
    this.socket = socket;
    this.frames = [];
    this.waiting = undefined;
    this.ready = new Promise((resolve, reject) => {
      const timer = setTimeout(() => this.fail(new Error('Electrum connection timed out')), connectTimeoutMs);
      this.connect = { resolve: () => { clearTimeout(timer); resolve(); }, reject: error => { clearTimeout(timer); reject(error); } };
    });
    this.ready.catch(() => {});
    socket.binaryType = 'arraybuffer';
    socket.addEventListener('open', () => { this.connect?.resolve(); this.connect = undefined; });
    socket.addEventListener('error', () => this.fail(new Error('Electrum WebSocket connection failed')));
    socket.addEventListener('close', () => this.fail(new Error('Electrum WebSocket disconnected')));
    socket.addEventListener('message', event => {
      if (this.error) return;
      let text;
      try {
        text = typeof event.data === 'string' ? event.data : new TextDecoder('utf-8', { fatal: true }).decode(event.data);
      } catch { this.fail(new Error('Electrum frame is not UTF-8')); return; }
      if (text.length > MAX_BYTES) { this.fail(new Error('Electrum frame exceeds 8 MiB')); return; }
      if (this.onFrame) { this.onFrame(text); return; }
      if (this.waiting) { const { resolve } = this.waiting; this.waiting = undefined; resolve(text); }
      else this.frames.push(text);
    });
  }
  fail(error) {
    if (this.error) return;
    this.error = error;
    this.connect?.reject(error); this.connect = undefined;
    this.waiting?.reject(error); this.waiting = undefined;
    this.frames = [];
    this.socket.close();
    this.onClose?.();
  }
  close() { this.fail(new Error('Electrum session closed')); }
  send(text) {
    if (this.error) throw this.error;
    if (typeof text !== 'string' || text.length > MAX_BYTES) throw new Error('Electrum frame exceeds limit');
    // Never retry writes here: Rust decides whether an operation may be repeated.
    this.socket.send(text);
  }
  receive(timeoutMs) {
    if (this.error) return Promise.reject(this.error);
    if (this.frames.length) return Promise.resolve(this.frames.shift());
    if (this.waiting) return Promise.reject(new Error('Concurrent Electrum receive is not supported'));
    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => this.fail(new Error('Electrum request timed out')), timeoutMs);
      this.waiting = { resolve: value => { clearTimeout(timer); resolve(value); }, reject: error => { clearTimeout(timer); reject(error); } };
    });
  }
}
