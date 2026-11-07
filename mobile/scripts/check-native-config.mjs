import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdtempSync, readFileSync, symlinkSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import nativeBuild from './native-build.js';

// Generate real Expo native projects in an isolated copy. This deliberately does
// not run CocoaPods, Gradle, SDK installers or device installation. Keep artifacts
// for inspection; never clean or rewrite the user's mobile/ios or mobile/android.
const mobile = resolve(dirname(fileURLToPath(import.meta.url)), '..');
const targets = nativeBuild.targets;
const app = JSON.parse(readFileSync(join(mobile, 'app.json'), 'utf8'));
assert(app.expo.plugins?.includes('./plugins/withAnzenRuntime'), 'Register the Anzen native config plugin in app.json.');
const project = mkdtempSync(join(tmpdir(), 'anzen-native-config-'));
console.log(`Native configuration check directory: ${project}`);
for (const file of ['package.json', 'app.json', 'native-targets.json', 'index.ts']) copyFileSync(join(mobile, file), join(project, file));
for (const directory of ['node_modules', 'plugins', 'modules']) symlinkSync(join(mobile, directory), join(project, directory), 'dir');
const result = spawnSync(process.execPath, [join(mobile, 'node_modules/expo/bin/cli'), 'prebuild',
  '--platform', process.platform === 'darwin' ? 'all' : 'android', '--no-install',
  '--skip-dependency-update', 'react,react-native'], {
  cwd: project, stdio: 'inherit', env: { ...process.env, CI: '1', EXPO_NO_GIT_STATUS: '1' },
});
if (result.status !== 0) throw new Error(`Expo native project generation failed (${result.status ?? result.error}); artifacts remain at ${project}`);
const properties = readFileSync(join(project, 'android/gradle.properties'), 'utf8');
assert.match(properties, new RegExp(`^reactNativeArchitectures=${targets.android.targets.map(target => target.abi).join(',')}$`, 'm'));
assert.match(properties, new RegExp(`^android.minSdkVersion=${targets.android.minSdk}$`, 'm'));
assert.match(readFileSync(join(project, 'android/app/src/main/AndroidManifest.xml'), 'utf8'), /android:allowBackup="false"/);
console.log('Android generated properties match the Rust ABI/API targets.');
if (process.platform === 'darwin') {
  const pods = JSON.parse(readFileSync(join(project, 'ios/Podfile.properties.json'), 'utf8'));
  assert.equal(pods['ios.deploymentTarget'], targets.ios.deploymentTarget);
  console.log('iOS generated deployment target matches the Rust library and podspec.');
}
for (const platform of process.platform === 'darwin' ? ['apple', 'android'] : ['android']) {
  const linked = spawnSync(process.execPath, [join(mobile, 'node_modules/expo-modules-autolinking/bin/expo-modules-autolinking.js'),
    'resolve', '--platform', platform, '--json'], { cwd: project, encoding: 'utf8' });
  assert.equal(linked.status, 0, linked.stderr);
  const runtime = JSON.parse(linked.stdout).modules.find(module => module.packageName === 'anzen-runtime');
  assert(runtime, `AnzenRuntime is not linked on ${platform}`);
  console.log(`AnzenRuntime discovered from the generated ${platform} project.`);
}
assert(existsSync(join(project, 'android/app/build.gradle')));
console.log(`Native configuration checks passed. No native binary was built or installed. Artifacts: ${project}`);
