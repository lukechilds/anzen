import { spawnSync } from 'node:child_process';
import { createServer } from 'node:net';
import { randomBytes } from 'node:crypto';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

export const repository = resolve(dirname(fileURLToPath(import.meta.url)), '../..');

export function run(command, args, { cwd = repository, env = process.env } = {}) {
  const result = spawnSync(command, args, { cwd, env, stdio: 'inherit' });
  if (result.status !== 0) throw new Error(`${command} failed (${result.status ?? result.error})`);
}

export async function withRegtest(label, operation) {
  const project = `anzen-${label}-test-${process.pid}-${randomBytes(4).toString('hex')}`;
  const listener = createServer();
  await new Promise((resolve, reject) => { listener.once('error', reject); listener.listen(0, '127.0.0.1', resolve); });
  const port = listener.address().port;
  await new Promise(resolve => listener.close(resolve));
  const url = `http://127.0.0.1:${port}`;
  const env = { ...process.env, ANZEN_PLAYGROUND_RPC_PORT: String(port), ANZEN_NATIVE_TEST_RPC_URL: url, ANZEN_RPC_URL: url, ANZEN_RPC_USER: 'anzen', ANZEN_RPC_PASSWORD: 'anzen' };
  const compose = ['compose', '-p', project, '-f', 'compose.playground.yaml'];
  try {
    run('docker', [...compose, 'up', '-d', '--wait'], { env });
    await operation(env);
  } finally {
    // Only this freshly created project is removed; never reset the interactive chain.
    run('docker', [...compose, 'down', '--volumes'], { env });
  }
}
