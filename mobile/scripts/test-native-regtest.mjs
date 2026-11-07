import { run, withRegtest } from './isolated-regtest.mjs';

await withRegtest('native', env => {
  run('cargo', ['test', '--manifest-path', 'mobile/rust/Cargo.toml', '--test', 'regtest', '--locked', ...process.argv.slice(2), '--', '--ignored', '--test-threads=1', '--nocapture'], { env });
});
