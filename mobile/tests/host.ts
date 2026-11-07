import { spawn } from 'node:child_process';
import { mkdtemp } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

// Only the disposable test host is controlled here; tests interact with devices through the UI.
export async function startTestHost(port: number) {
  const directory = await mkdtemp(path.join(tmpdir(), 'anzen-ui-e2e-'));
  const child = spawn('../target/debug/anzen-playground', ['--port', String(port), '--data-dir', directory],
    { stdio: ['ignore', 'pipe', 'pipe'] });
  const url = await new Promise<string>((resolve, reject) => {
    let output = '', errors = '';
    const timer = setTimeout(() => { child.kill('SIGTERM'); reject(new Error(`Host startup timed out: ${errors}`)); }, 15_000);
    child.stderr!.on('data', chunk => { errors += chunk; });
    child.stdout!.on('data', chunk => {
      output += chunk;
      const match = output.match(/Open (http:\/\/[^\s]+)/);
      if (match) { clearTimeout(timer); resolve(match[1]); }
    });
    child.once('error', error => { clearTimeout(timer); reject(error); });
    child.once('exit', code => { clearTimeout(timer); reject(new Error(`Host exited ${code}: ${errors}`)); });
  });
  return { url, directory, stop: () => { child.kill('SIGTERM'); } };
}
