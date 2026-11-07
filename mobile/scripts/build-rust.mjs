import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdirSync, renameSync } from 'node:fs';
import { dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import nativeBuild from './native-build.js';
const { androidLinkArgs, androidToolchain, checkAndroidElf, targets } = nativeBuild;

const mobile = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const platform = process.argv[2];
const run = (command, args, env = {}) => {
  const result = spawnSync(command, args, { cwd: mobile, stdio: 'inherit', env: { ...process.env, ...env } });
  if (result.status !== 0) throw new Error(`${command} failed (${result.status ?? result.error})`);
};
const artifactRoot = resolve(mobile, 'rust/target');
const build = (target, env = {}, linkArgs = []) => {
  run('rustup', ['target', 'add', target]);
  // Explicit target-dir prevents a global CARGO_TARGET_DIR from making us package
  // an old library from the path below. Final-crate args also cannot be lost to a
  // user's RUSTFLAGS taking precedence over target-specific environment flags.
  run('cargo', ['rustc', '--manifest-path', 'rust/Cargo.toml', '--locked', '--release', '--lib',
    '--target-dir', artifactRoot, '--target', target,
    ...(linkArgs.length ? ['--', ...linkArgs.flatMap(arg => ['-C', `link-arg=${arg}`])] : [])], env);
};

if (platform === 'ios') {
  // Do not accept Xcode's license programmatically. Fail here before starting compilation.
  run('xcrun', ['--sdk', 'iphoneos', '--show-sdk-path']);
  for (const target of targets.ios.targets) build(target, { IPHONEOS_DEPLOYMENT_TARGET: targets.ios.deploymentTarget });
  const output = resolve(mobile, 'modules/anzen-runtime/ios/AnzenMobile.xcframework');
  if (existsSync(output)) renameSync(output, `${output}.previous-${Date.now()}`);
  run('xcodebuild', ['-create-xcframework', ...targets.ios.targets.flatMap(target => [
    '-library', resolve(artifactRoot, `${target}/release/libanzen_mobile.a`),
    '-headers', resolve(mobile, 'modules/anzen-runtime/ios/include'),
  ]), '-output', output]);
} else if (platform === 'android') {
  const { builds, readelf } = androidToolchain(process.env.ANZEN_ANDROID_NDK);
  for (const target of builds) {
    build(target.rust, target.env, androidLinkArgs);
    const result = spawnSync(readelf, ['-Wl', resolve(artifactRoot, `${target.rust}/release/libanzen_mobile.so`)], { encoding: 'utf8' });
    if (result.status !== 0) throw new Error(`Could not inspect Android library: ${result.stderr || result.error}`);
    checkAndroidElf(result.stdout);
  }
  // Do not replace packaged libraries until all requested builds pass the check.
  for (const target of builds) {
    const output = resolve(mobile, `modules/anzen-runtime/android/src/main/jniLibs/${target.abi}`);
    mkdirSync(output, { recursive: true });
    copyFileSync(resolve(artifactRoot, `${target.rust}/release/libanzen_mobile.so`), resolve(output, 'libanzen_mobile.so'));
  }
} else {
  throw new Error('Usage: node scripts/build-rust.mjs ios|android');
}
