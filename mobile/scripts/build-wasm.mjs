import { spawnSync } from 'node:child_process';
import { existsSync, mkdirSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';

const mobile = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const env = { ...process.env };
if (process.platform === 'darwin') {
  env.DEVELOPER_DIR ||= '/Library/Developer/CommandLineTools';
  if (existsSync('/opt/homebrew/opt/llvm/bin/clang')) {
    env.CC_wasm32_unknown_unknown ||= '/opt/homebrew/opt/llvm/bin/clang';
    env.AR_wasm32_unknown_unknown ||= '/opt/homebrew/opt/llvm/bin/llvm-ar';
  }
}
function run(command, args) {
  const result = spawnSync(command, args, { cwd: mobile, env, stdio: 'inherit' });
  if (result.error) throw result.error;
  if (result.status !== 0) process.exit(result.status || 1);
}
run('cargo', ['build', '--manifest-path', 'wasm/Cargo.toml', '--target', 'wasm32-unknown-unknown', '--release', '--locked']);
mkdirSync(resolve(mobile, 'public/wasm'), { recursive: true });
run('wasm-bindgen', ['wasm/target/wasm32-unknown-unknown/release/anzen_browser.wasm', '--target', 'web', '--out-dir', 'public/wasm']);
