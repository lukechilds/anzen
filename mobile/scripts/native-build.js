const { accessSync, constants, readFileSync, readdirSync } = require('node:fs');
const { resolve } = require('node:path');

const targets = require('../native-targets.json');
const androidLinkArgs = [
  `-Wl,-z,max-page-size=${targets.android.pageSize}`,
  `-Wl,-z,common-page-size=${targets.android.pageSize}`,
  // Named, so the JNI bridge that links against it asks the device for "libanzen_mobile.so".
  // A library with no name is recorded by the path it was linked from, on the build machine,
  // and the app then fails to start with that path "not found".
  '-Wl,-soname,libanzen_mobile.so',
];

// Resolve tools before rustup/downloads/builds. Do not guess a missing NDK, install
// an SDK or accept a platform license as a side effect of building the Rust crate.
function androidToolchain(ndk, platform = process.platform) {
  if (!ndk) throw new Error(`Set ANZEN_ANDROID_NDK to an installed Android NDK (r${targets.android.minNdkMajor} or later) after reviewing its SDK licenses.`);
  if (!['darwin', 'linux'].includes(platform)) throw new Error('Android cross-builds currently support macOS and Linux hosts.');
  const revision = readFileSync(resolve(ndk, 'source.properties'), 'utf8').match(/^Pkg\.Revision\s*=\s*(\d+)\./m);
  if (!revision || Number(revision[1]) < targets.android.minNdkMajor) throw new Error(`Android NDK r${targets.android.minNdkMajor} or later is required.`);
  const prebuilt = resolve(ndk, 'toolchains/llvm/prebuilt');
  const host = readdirSync(prebuilt).find(name => name.startsWith(`${platform}-`));
  if (!host) throw new Error('No compatible NDK host toolchain');
  const bin = resolve(prebuilt, host, 'bin');
  const readelf = resolve(bin, 'llvm-readelf');
  const builds = targets.android.targets.map(target => {
    const suffix = target.rust.replaceAll('-', '_');
    const compiler = resolve(bin, `${target.compiler}${targets.android.minSdk}-clang`);
    const cxx = `${compiler}++`;
    const ar = resolve(bin, 'llvm-ar');
    for (const path of [compiler, cxx, ar, readelf]) accessSync(path, constants.X_OK);
    return { ...target, env: {
      [`CARGO_TARGET_${suffix.toUpperCase()}_LINKER`]: compiler,
      [`CC_${suffix}`]: compiler, [`CXX_${suffix}`]: cxx, [`AR_${suffix}`]: ar,
    } };
  });
  return { builds, readelf };
}

// llvm-readelf -Wl prints one program-header row per line. Check both LOAD alignment
// and the end of RELRO; older linkers need common-page-size as well as max-page-size.
// This checks our Rust .so, not every dependency or the final APK's ZIP alignment.
function checkAndroidElf(output) {
  let loads = 0;
  let relro = 0;
  const page = BigInt(targets.android.pageSize);
  for (const line of output.split('\n')) {
    const fields = line.trim().split(/\s+/);
    if (!['LOAD', 'GNU_RELRO'].includes(fields[0])) continue;
    if (fields.length < 8 || !fields.slice(1, 6).every(value => /^0x[\da-f]+$/i.test(value)) || !/^0x[\da-f]+$/i.test(fields.at(-1))) {
      throw new Error('Cannot read ELF program headers.');
    }
    const offset = BigInt(fields[1]), address = BigInt(fields[2]), size = BigInt(fields[5]);
    if (fields[0] === 'LOAD') {
      loads++;
      const alignment = BigInt(fields.at(-1));
      if (alignment < page || (alignment & (alignment - 1n)) !== 0n || offset % alignment !== address % alignment) {
        throw new Error('Android library LOAD segments are not 16 KB aligned.');
      }
    } else {
      relro++;
      if ((address + size) % page !== 0n) throw new Error('Android library RELRO end is not 16 KB aligned.');
    }
  }
  if (!loads || !relro) throw new Error('Android library is missing LOAD or GNU_RELRO program headers.');
}

module.exports = { targets, androidLinkArgs, androidToolchain, checkAndroidElf };
