import { resolve } from 'node:path';
import { repository, run, withRegtest } from './isolated-regtest.mjs';

// Never run browser tests against a stale Rust binary or the user's interactive chain.
run(process.execPath, [resolve(repository, 'mobile/scripts/build-wasm.mjs')]);
run('cargo', ['build', '-p', 'anzen-playground', '--locked']);
await withRegtest('browser', env => {
  run(process.execPath, [resolve(repository, 'mobile/node_modules/@playwright/test/cli.js'), 'test', ...process.argv.slice(2)], { cwd: resolve(repository, 'mobile'), env });
});
