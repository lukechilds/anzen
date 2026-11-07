// Real-regtest transport integration, not a browser/UI substitute.
// Start compose.playground.yaml and build anzen-playground first.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { once } from 'node:events';

const directory = await mkdtemp(path.join(tmpdir(), 'anzen-playground-test-'));
const port = 18787;
const child = spawn('target/debug/anzen-playground', ['--port', String(port), '--data-dir', directory], { stdio: ['ignore', 'pipe', 'pipe'] });
const url = `http://127.0.0.1:${port}`;
let stderr = '';
child.stderr.on('data', chunk => { stderr += chunk; });
const admin = await new Promise((resolve, reject) => {
  let output = '';
  const timer = setTimeout(() => reject(new Error(`Host startup timed out: ${stderr}`)), 15_000);
  child.stdout.on('data', chunk => {
    output += chunk;
    const match = output.match(/#playground=([a-f0-9]{64})/);
    if (match) { clearTimeout(timer); resolve(match[1]); }
  });
  child.once('exit', code => { clearTimeout(timer); reject(new Error(`Host exited ${code}: ${stderr}`)); });
});

async function request(route, capability, body, { fail = false, origin = 'http://127.0.0.1:8081' } = {}) {
  const response = await fetch(url + route, { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${capability}`, Origin: origin }, body: JSON.stringify(body) });
  const text = await response.text();
  if (fail) { assert.ok(!response.ok, `Unexpected success: ${route} ${body.method}`); return text; }
  assert.ok(response.ok, `${route} ${body.method}: ${text}`);
  return JSON.parse(text);
}
const node = (body, options) => request('/node', admin, body, options);
try {
  await request('/session', 'bad-token', {}, { fail: true });
  await request('/session', admin, {}, { fail: true, origin: 'https://untrusted.example' });
  assert.deepEqual(await request('/session', admin, {}), { allow_mainnet: false });
  // There is deliberately no HTTP device API: application commands run in browser WASM.
  for (const route of ['/devices', '/devices/list', '/device']) {
    const response = await fetch(url + route, { method: 'POST', headers: { Authorization: `Bearer ${admin}`, 'Content-Type': 'application/json' }, body: '{}' });
    assert.equal(response.status, 404);
  }
  const chain = (network, body, options) => request('/chain', admin, { network, request: body }, options);
  await chain('bitcoin', { method: 'tip' }, { fail: true });
  await chain('regtest', { method: 'initialize' }, { fail: true });
  await chain('regtest', { method: 'tip', private_key: 'forbidden' }, { fail: true });
  const before = await chain('regtest', { method: 'tip' });
  await node({ method: 'mine', blocks: 1 });
  assert.equal((await chain('regtest', { method: 'tip' })).height, before.height + 1);
  console.log('PASS: public-only chain transport, authentication, origin rejection and mainnet guard.');

  // Opaque transport fixture, not a decryptable recovery backup. Real backup encryption,
  // approval and recovery are covered end to end in the WASM browser scenarios.
  const blob = { version: 1, purpose: 'opaque-transport-test', nonce: Array(24).fill(0), ciphertext: [1, 2, 3, 4] };
  const backup = { version: 1, kind: 'vault-cloud-recovery', encrypted_payload: blob, hww_encrypted_symmetric_key: blob, friends: [], encrypted_friend_manifest: blob };
  await request('/cloud', 'bad-token', { method: 'list' }, { fail: true });
  await request('/cloud', admin, { method: 'load', id: '../../phone/device' }, { fail: true });
  const saved = await request('/cloud', admin, { method: 'save', backup });
  assert.deepEqual(await request('/cloud', admin, { method: 'save', backup }), saved);
  assert.deepEqual(await request('/cloud', admin, { method: 'list' }), { backups: [saved.id] });
  assert.deepEqual(await request('/cloud', admin, { method: 'load', id: saved.id }), backup);
  console.log('PASS: opaque cloud object round trip, idempotent save and invalid path rejection.');

  const job = body => request('/node/job', admin, body);
  await request('/node/job', 'bad-token', { method: 'start', blocks: 1 }, { fail: true });
  await request('/node/job', admin, { method: 'start', blocks: 70001 }, { fail: true });
  async function waitJob(id) {
    for (let attempt = 0; attempt < 100; attempt++) {
      const state = await job({ method: 'status', id });
      if (!['queued', 'running'].includes(state.status)) return state;
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    throw new Error('Mining job did not finish or cancel within 10 seconds');
  }
  const short = await job({ method: 'start', blocks: 3 });
  assert.equal((await waitJob(short.id)).mined_blocks, 3);
  const long = await job({ method: 'start', blocks: 61200 });
  await job({ method: 'cancel', id: long.id });
  const cancelled = await waitJob(long.id);
  assert.equal(cancelled.status, 'cancelled');
  assert.ok(cancelled.mined_blocks < 61200);
  console.log('PASS: mining-job progress, cancellation, request bounds and authentication.');
} finally {
  child.kill('SIGTERM');
  await once(child, 'exit');
  console.log(`Test state retained at ${directory}; no mainnet state was accessed.`);
}
