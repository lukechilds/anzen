import React, { useMemo, useState } from 'react';
import { ActivityIndicator, Pressable, ScrollView, Text, TextInput, View } from 'react-native';
import { requireOptionalNativeModule } from 'expo';
import { File, Paths } from 'expo-file-system';
import AnzenApp, { Button } from './AnzenApp';
import DeviceSettings, { DevicePrefs } from './DeviceSettings';
import { BenchmarkRunner } from './Benchmarks';
import { DeviceTransport, JsonObject, ProtocolMessage, Role } from './transport';
import { parseProtocolMessage } from './fileExchange';
import { nativeFileExchange } from './nativeFileExchange';
import { colors, styles as s } from './theme';

interface NativeRuntime {
  execute(role: Role, command: string, settings: string): Promise<string>;
  // Live updates on Rust's own thread: start, targets, changes or stop. Never waits for a command.
  watch(settings: string, request: string): string;
  // Vanity-search benchmarks on their own queue: {"engine": "gpu" | "cpu", "seconds": n}.
  benchmark(request: string): Promise<string>;
}
// This phone's network, role and regtest node, kept between launches. Nothing secret: a regtest
// node's credentials at most.
const defaults: DevicePrefs = { network: 'signet', role: 'wallet', rpc: { url: 'http://127.0.0.1:18443', user: 'anzen', password: 'anzen' } };
const prefsFile = () => new File(Paths.document, 'anzen-device.json');
function loadPrefs(): DevicePrefs {
  try {
    const file = prefsFile();
    if (!file.exists) return defaults;
    const saved = JSON.parse(file.textSync());
    return {
      network: ['signet', 'bitcoin', 'regtest'].includes(saved.network) ? saved.network : defaults.network,
      role: ['wallet', 'signer'].includes(saved.role) ? saved.role : defaults.role,
      rpc: { ...defaults.rpc, ...saved.rpc },
    };
  } catch { return defaults; }
}
function savePrefs(prefs: DevicePrefs) {
  try { const file = prefsFile(); if (!file.exists) file.create(); file.write(JSON.stringify(prefs)); }
  catch { /* The choice still applies until the app closes. */ }
}

export default function NativeApp() {
  const [prefs, setPrefs] = useState<DevicePrefs>(loadPrefs);
  const { network, role } = prefs;
  const updatePrefs = (next: DevicePrefs) => { savePrefs(next); setPrefs(next); setIncoming(null); setCloudBackup(null); setError(''); setFileNotice(''); };
  const [incoming, setIncoming] = useState<ProtocolMessage | null>(null);
  const [exchange, setExchange] = useState(false);
  const [fileBusy, setFileBusy] = useState(false);
  const [cloudBackup, setCloudBackup] = useState<JsonObject | null>(null);
  const [fileNotice, setFileNotice] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const runtime = requireOptionalNativeModule<NativeRuntime>('AnzenRuntime');
  const transport = useMemo<DeviceTransport | null>(() => {
    if (!runtime) return null;
    // Electrum with no servers given uses the network's defaults (Mutinynet's or mainnet's).
    const settings = JSON.stringify({ network, dangerously_enable_mainnet: network === 'bitcoin', backend: network === 'regtest'
      ? { type: 'rpc', url: prefs.rpc.url, user: prefs.rpc.user, password: prefs.rpc.password }
      : { type: 'electrum', servers: [] } });
    const watchCall = (request: JsonObject) => {
      const response = JSON.parse(runtime.watch(settings, JSON.stringify(request)));
      if (response.error) throw new Error(response.error);
      return response.ok;
    };
    let updateTargets: (() => void) | undefined;
    const execute = async (command: JsonObject) => {
      const response = JSON.parse(await runtime.execute(role, JSON.stringify(command), settings));
      if (response.error) throw new Error(response.error);
      // Commands can reveal addresses or pair a vault, so re-read what Electrum should watch.
      if (command.method !== 'watch_targets') updateTargets?.();
      return response.ok;
    };
    const watch = (listener: (reason: string) => void) => {
      watchCall({ method: 'start', network });
      let targetsTimer: ReturnType<typeof setTimeout> | undefined;
      // Regtest polls the node as a whole; only Electrum subscribes to scripts.
      updateTargets = network === 'regtest' ? undefined : () => {
        clearTimeout(targetsTimer);
        targetsTimer = setTimeout(() => {
          execute({ method: 'watch_targets' })
            .then(({ scripthashes }) => watchCall({ method: 'targets', scripthashes }))
            .catch(() => { /* No wallet key yet; the command that creates one retries. */ });
        }, 250);
      };
      updateTargets?.();
      const timer = setInterval(() => {
        try { const { changes } = watchCall({ method: 'changes' }); if (changes.length) listener(changes.join(',')); }
        catch { /* A stopped watcher reports no changes. */ }
      }, 1_000);
      return () => { clearInterval(timer); clearTimeout(targetsTimer); updateTargets = undefined; try { watchCall({ method: 'stop' }); } catch {} };
    };
    return { role, execute, ...(role === 'wallet' ? { watch } : {}) };
  }, [role, runtime, network, prefs.rpc]);
  const benchmark = useMemo<BenchmarkRunner | undefined>(() => {
    if (!runtime?.benchmark) return undefined;
    return {
      run: async (engine, seconds) => {
        const response = JSON.parse(await runtime.benchmark(JSON.stringify({ engine, seconds })));
        if (response.error) throw new Error(response.error);
        return response.ok;
      },
    };
  }, [runtime]);
  const acceptMessage = (value: ProtocolMessage) => {
    setIncoming(value); setMessage(''); setExchange(false); setError(''); setFileNotice('');
  };
  async function receiveFile(backup = false) {
    if (fileBusy) return;
    setFileBusy(true); setError(''); setFileNotice('');
    try {
      if (backup) {
        const value = await nativeFileExchange.receiveBackup(role);
        if (value) {
          setCloudBackup(value); setExchange(false);
          setFileNotice('Backup file loaded for recovery. Open a recovery action in Vault to continue; no key has been restored yet.');
        }
      } else {
        const value = await nativeFileExchange.receiveMessage();
        if (value) acceptMessage(value);
      }
    } catch (e) { setError(String(e)); }
    finally { setFileBusy(false); }
  }
  if (!runtime) return <View style={[s.screen, { padding: 32, justifyContent: 'center' }]}>
    <Text style={s.brand}>anzen</Text><Text style={s.subtext}>The native Rust module is not installed in this build. Native integration is under implementation; this screen does not simulate a working wallet.</Text>
  </View>;
  return <View style={[s.screen, { paddingTop: 50, paddingBottom: 20 }]}>
    {exchange && <ScrollView contentContainerStyle={{ padding: 24 }}><Text style={s.heading}>Receive a file</Text>
      <Text style={s.subtext}>Choose the protocol JSON shared by the other device. Importing it does not approve, sign, or broadcast anything.</Text>
      <Button title="Choose protocol message file" primary disabled={fileBusy} onPress={() => void receiveFile()} />
      {role === 'wallet' && <>
        <Text style={s.label}>Encrypted recovery backup</Text>
        <Text style={s.subtext}>Load a backup saved separately from the signer. Never send this file to your signer.</Text>
        <Button title="Choose encrypted backup file" disabled={fileBusy} onPress={() => void receiveFile(true)} />
      </>}
      <Text style={s.label}>Or paste a protocol message</Text>
      <Text style={s.tiny}>Protocol JSON only, never seed words or an encrypted recovery backup. Files are recommended for larger packages. Maximum 8 MiB.</Text>
      <TextInput accessibilityLabel="Received protocol JSON" multiline style={[s.input, { minHeight: 180 }]} value={message} onChangeText={setMessage} autoCapitalize="none" autoCorrect={false} />
      <Button title="Import message" disabled={fileBusy} onPress={() => {
        try {
          acceptMessage(parseProtocolMessage(message));
        } catch (e) { setError(String(e)); }
      }} /><Button title="Cancel import" disabled={fileBusy} onPress={() => { setExchange(false); setMessage(''); setError(''); }} />
      {fileBusy && <ActivityIndicator color={colors.mint} />}
    </ScrollView>}
    {/* Importing a response must not unmount the app and discard an in-progress review. */}
    <View style={{ flex: 1, display: exchange ? 'none' : 'flex' }}>
      {/* A new network or role is another device: start it fresh. */}
      <AnzenApp key={`${network}:${role}`} transport={transport!} network={network} incoming={incoming} dismissIncoming={() => setIncoming(null)}
        cloudBackup={cloudBackup} saveBackup={backup => nativeFileExchange.saveBackup(role, backup)}
        deliver={nativeFileExchange.sendMessage} benchmark={role === 'wallet' ? benchmark : undefined}
        deviceSettings={<DeviceSettings prefs={prefs} onChange={updatePrefs}
          onImport={() => { setExchange(true); setError(''); setFileNotice(''); }}/>} />
    </View>
    {/* What a file import did, until dismissed. */}
    {!exchange && (!!fileNotice || !!error) && <Pressable accessibilityRole="button" accessibilityLabel="Dismiss" onPress={() => { setFileNotice(''); setError(''); }}
      style={{ position: 'absolute', left: 16, right: 16, bottom: 96, padding: 14, borderRadius: 16, backgroundColor: colors.surface }}>
      {!!fileNotice && <Text style={s.tiny}>{fileNotice}</Text>}
      {!!error && <Text style={s.errorText}>{error}</Text>}
    </Pressable>}
  </View>;
}
