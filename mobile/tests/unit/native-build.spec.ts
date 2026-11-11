import { test, expect } from '@playwright/test';
import { chmodSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { compileModsAsync } from 'expo/config-plugins';
import type { PropertiesItem } from '@expo/config-plugins/build/android/Properties';
import withAnzenRuntime from '../../plugins/withAnzenRuntime';
import { androidLinkArgs, androidToolchain, checkAndroidElf, targets } from '../../scripts/native-build';

const base = { name: 'Anzen native config test', slug: 'anzen-native-config-test' };
const projectRoot = resolve(__dirname, '../..');
const property = (key: string, value: string): PropertiesItem => ({ type: 'property', key, value });
async function applyProperties(properties: PropertiesItem[]) {
  return withAnzenRuntime.configureProperties(properties);
}

test('Expo introspection produces the same API and ABI contract as the Rust build', async () => {
  const config = await compileModsAsync(withAnzenRuntime({ ...base }), {
    projectRoot, platforms: ['android'], introspect: true, ignoreExistingNativeFiles: true,
  });
  const properties = config._internal!.modResults.android.gradleProperties;
  expect(properties).toContainEqual(property('reactNativeArchitectures', 'arm64-v8a,x86_64'));
  expect(properties).toContainEqual(property('android.minSdkVersion', '26'));
  expect(config.ios!.deploymentTarget).toBe(targets.ios.deploymentTarget);
  expect(config.ios!.infoPlist!.NSLocalNetworkUsageDescription).toContain('Bitcoin node');
  expect(targets.android.targets.map((target: { abi: string }) => target.abi)).toEqual(['arm64-v8a', 'x86_64']);
});

test('native config is idempotent, retains unrelated properties, and never lowers Android minimums', async () => {
  const comment: PropertiesItem = { type: 'comment', value: 'local build settings' };
  const initial = [comment, property('org.gradle.jvmargs', '-Xmx4g'), property('constructor', 'preserve this'),
    property('reactNativeArchitectures', 'armeabi-v7a,arm64-v8a,x86,x86_64'),
    property('android.minSdkVersion', '29'), property('android.minSdkVersion', '24')];
  const updated = await applyProperties(initial);
  expect(updated).toEqual([comment, initial[1], initial[2], property('reactNativeArchitectures', 'arm64-v8a,x86_64'),
    property('android.minSdkVersion', '29')]);
  expect(await applyProperties(updated)).toEqual(updated);
  expect(initial).toHaveLength(6);
  expect(await applyProperties([property('android.minSdkVersion', '24')])).toContainEqual(property('android.minSdkVersion', '26'));
});

test('iOS adopts the scene lifecycle that iOS 27 requires at launch', async () => {
  const config = await compileModsAsync(withAnzenRuntime({ ...base }), {
    projectRoot, platforms: ['ios'], introspect: true, ignoreExistingNativeFiles: true,
  });
  const configuration = config.ios!.infoPlist!.UIApplicationSceneManifest.UISceneConfigurations.UIWindowSceneSessionRoleApplication[0];
  expect(configuration.UISceneDelegateClassName).toBe('$(PRODUCT_MODULE_NAME).SceneDelegate');

  // The window block from Expo SDK 57's AppDelegate.swift template.
  const template = `  ) -> Bool {
#if os(iOS) || os(tvOS)
    window = UIWindow(frame: UIScreen.main.bounds)
    factory.startReactNative(
      withModuleName: "main",
      in: window,
      launchOptions: launchOptions)
#endif

    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
`;
  const adopted = withAnzenRuntime.adoptSceneLifecycle(template);
  expect(adopted).not.toContain('UIScreen.main.bounds');
  expect(adopted).toContain('class SceneDelegate: UIResponder, UIWindowSceneDelegate');
  expect(withAnzenRuntime.adoptSceneLifecycle(adopted)).toBe(adopted);
  expect(() => withAnzenRuntime.adoptSceneLifecycle('class AppDelegate {}')).toThrow('no longer matches the Expo template');
});

test('native config rejects malformed Android API levels', async () => {
  for (const value of ['invalid', '26.5', '-1', '0']) {
    await expect(applyProperties([property('android.minSdkVersion', value)])).rejects.toThrow('Invalid android.minSdkVersion');
  }
});

function ndkFixture(revision = '27.1.12297006', platform = 'darwin') {
  const root = mkdtempSync(join(tmpdir(), 'anzen-fake-ndk-'));
  writeFileSync(join(root, 'source.properties'), `Pkg.Revision = ${revision}\n`);
  const bin = join(root, 'toolchains/llvm/prebuilt', `${platform}-x86_64`, 'bin');
  mkdirSync(bin, { recursive: true });
  // Executable placeholders for path validation only; never run as compilers.
  for (const name of ['llvm-ar', 'llvm-readelf', ...targets.android.targets.flatMap((target: { compiler: string }) =>
    [`${target.compiler}${targets.android.minSdk}-clang`, `${target.compiler}${targets.android.minSdk}-clang++`])]) {
    writeFileSync(join(bin, name), '#!/bin/sh\nexit 99\n', { mode: 0o755 });
  }
  return { root, bin, close: () => rmSync(root, { recursive: true }) };
}

test('Android tool resolution covers both ABIs and sets target-specific C/C++/link tools', () => {
  for (const platform of ['darwin', 'linux'] as const) {
    const fixture = ndkFixture('27.1.12297006', platform);
    try {
      const toolchain = androidToolchain(fixture.root, platform);
      expect(toolchain.readelf).toBe(join(fixture.bin, 'llvm-readelf'));
      expect(toolchain.builds).toHaveLength(2);
      for (const build of toolchain.builds) {
        const suffix = build.rust.replaceAll('-', '_');
        const compiler = join(fixture.bin, `${build.compiler}26-clang`);
        expect(build.env).toEqual({ [`CARGO_TARGET_${suffix.toUpperCase()}_LINKER`]: compiler,
          [`CC_${suffix}`]: compiler, [`CXX_${suffix}`]: `${compiler}++`, [`AR_${suffix}`]: join(fixture.bin, 'llvm-ar') });
      }
      expect(androidLinkArgs).toEqual(['-Wl,-z,max-page-size=16384', '-Wl,-z,common-page-size=16384', '-Wl,-soname,libanzen_mobile.so']);
    } finally { fixture.close(); }
  }
});

test('missing, old, malformed and unusable NDKs fail before a build can start', () => {
  expect(() => androidToolchain(undefined)).toThrow('Set ANZEN_ANDROID_NDK');
  expect(() => androidToolchain('/unused', 'win32')).toThrow('macOS and Linux');
  for (const revision of ['26.3.11579264', 'unknown']) {
    const fixture = ndkFixture(revision);
    try { expect(() => androidToolchain(fixture.root, 'darwin')).toThrow('r27 or later'); }
    finally { fixture.close(); }
  }
  const fixture = ndkFixture();
  try {
    expect(() => androidToolchain(fixture.root, 'linux')).toThrow('No compatible');
    chmodSync(join(fixture.bin, 'llvm-readelf'), 0o644);
    expect(() => androidToolchain(fixture.root, 'darwin')).toThrow();
  } finally { fixture.close(); }
});

// Program-header fixtures test the verifier, not a compiled Android library.
const elf = `
Elf file type is DYN (Shared object file)
  Type           Offset   VirtAddr           PhysAddr           FileSiz  MemSiz   Flg Align
  LOAD           0x000000 0x0000000000000000 0x0000000000000000 0x001234 0x001234 R   0x4000
  LOAD           0x004000 0x0000000000004000 0x0000000000004000 0x003210 0x003210 R E 0x4000
  LOAD           0x008000 0x0000000000008000 0x0000000000008000 0x004000 0x004000 RW  0x4000
  GNU_RELRO      0x008000 0x0000000000008000 0x0000000000008000 0x001200 0x004000 R   0x1
`;
test('the ELF check accepts 16 KB LOAD segments and a 16 KB aligned RELRO end', () => {
  expect(() => checkAndroidElf(elf)).not.toThrow();
});
test('the ELF check rejects 4 KB, invalid, or incongruent LOAD segments', () => {
  for (const invalid of [elf.replaceAll('0x4000\n', '0x1000\n'), elf.replaceAll('0x4000\n', '0x5000\n'),
    elf.replace('0x004000 0x0000000000004000', '0x004001 0x0000000000004000')]) {
    expect(() => checkAndroidElf(invalid)).toThrow('LOAD segments');
  }
});
test('the ELF check rejects a misaligned RELRO end even when LOAD alignment is correct', () => {
  expect(() => checkAndroidElf(elf.replace('0x001200 0x004000', '0x001200 0x001200'))).toThrow('RELRO end');
});
test('the ELF check rejects absent and malformed program headers', () => {
  expect(() => checkAndroidElf('')).toThrow('missing LOAD or GNU_RELRO');
  expect(() => checkAndroidElf(elf.replace('GNU_RELRO', 'NOTE'))).toThrow('missing LOAD or GNU_RELRO');
  expect(() => checkAndroidElf('LOAD 0x0')).toThrow('Cannot read ELF');
  expect(() => checkAndroidElf(elf.replace('0x003210', '0xINVALID'))).toThrow('Cannot read ELF');
});
