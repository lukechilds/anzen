import React, { useMemo, useState } from 'react';
import { ActivityIndicator, Platform, ScrollView, Text, TextInput, View } from 'react-native';
import { requireOptionalNativeModule } from 'expo';
import AnzenApp, { Button } from './AnzenApp';
import { BenchmarkRunner } from './Benchmarks';
import { DeviceTransport, JsonObject, ProtocolMessage, Role } from './transport';
import { parseProtocolMessage } from './fileExchange';
import { nativeFileExchange } from './nativeFileExchange';
import { colors, styles as s } from './theme';

interface NativeRuntime {
  execute(role: Role, command: string, settings: string): Promise<string>;
  // Live updates on Rust's own thread: start, targets, changes or stop. Never waits for a command.
  watch(settings: string, request: string): string;
  // GPU benchmarks on their own queue: {"method":"backends"} or {"method":"run", ...}.
  benchmark(request: string): Promise<string>;
}
export default function NativeApp() {
  const [role, setRole] = useState<Role | null>(null);
  const [incoming, setIncoming] = useState<ProtocolMessage | null>(null);
  const [exchange, setExchange] = useState(false);
  const [fileBusy, setFileBusy] = useState(false);
  const [cloudBackup, setCloudBackup] = useState<JsonObject | null>(null);
  const [fileNotice, setFileNotice] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [mainnet, setMainnet] = useState(false);
  const [rpcUrl, setRpcUrl] = useState('http://127.0.0.1:18443');
  const [rpcUser, setRpcUser] = useState('anzen');
  const [rpcPassword, setRpcPassword] = useState('anzen');
  const runtime = requireOptionalNativeModule<NativeRuntime>('AnzenRuntime');
  const transport = useMemo<DeviceTransport | null>(() => {
    if (!role || !runtime) return null;
    const network = mainnet ? 'bitcoin' : 'regtest';
    const settings = JSON.stringify({ dangerously_enable_mainnet: mainnet, backend: mainnet
      ? { type: 'electrum', servers: [] }
      : { type: 'rpc', url: rpcUrl, user: rpcUser, password: rpcPassword } });
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
  }, [role, runtime, mainnet, rpcUrl, rpcUser, rpcPassword]);
  const benchmark = useMemo<BenchmarkRunner | undefined>(() => {
    if (!runtime?.benchmark) return undefined;
    const call = async (request: JsonObject) => {
      const response = JSON.parse(await runtime.benchmark(JSON.stringify(request)));
      if (response.error) throw new Error(response.error);
      return response.ok;
    };
    // The Rust library is built with native Metal for Apple devices only.
    return { backends: Platform.OS === 'ios' ? ['wgpu', 'metal'] : ['wgpu'], run: (backend, kernel, seconds) => call({ method: 'run', backend, kernel, seconds }) };
  }, [runtime]);
  const acceptMessage = (value: ProtocolMessage) => {
    setIncoming(value); setMessage(''); setExchange(false); setError(''); setFileNotice('');
  };
  async function receiveFile(backup = false) {
    if (fileBusy || !role) return;
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
  if (!role) return <ScrollView style={s.screen} contentContainerStyle={{ padding: 32, paddingTop: 80 }}><Text style={s.brand}>anzen</Text><Text style={s.subtext}>How will you use this device?</Text>
    <Text style={s.label}>Network · {mainnet ? 'MAINNET' : 'REGTEST'}</Text>
    <Button title={mainnet ? 'Use regtest instead' : 'Dangerously enable mainnet'} danger={!mainnet} onPress={() => setMainnet(value => !value)} />
    {mainnet ? <Text style={s.errorText}>Experimental wallet software. Mainnet uses real bitcoin and public Electrum servers. This explicitly enables mainnet for this session. Do not use funds you cannot afford to lose.</Text> : <>
      <Text style={s.label}>Regtest RPC URL (reachable from this device)</Text><TextInput style={s.input} value={rpcUrl} onChangeText={setRpcUrl} autoCapitalize="none" />
      <Text style={s.label}>RPC user</Text><TextInput style={s.input} value={rpcUser} onChangeText={setRpcUser} autoCapitalize="none" />
      <Text style={s.label}>RPC password</Text><TextInput style={s.input} value={rpcPassword} onChangeText={setRpcPassword} secureTextEntry />
    </>}
    <Button title="Use as wallet" primary onPress={() => setRole('wallet')} /><Button title="Enable hardware wallet mode" onPress={() => setRole('signer')} />
    <Text style={s.tiny}>Use separate physical devices for independent keys. A second phone is not a secure-element hardware wallet.</Text>
  </ScrollView>;
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
      <AnzenApp transport={transport!} network={mainnet ? 'bitcoin' : 'regtest'} incoming={incoming} dismissIncoming={() => setIncoming(null)}
        cloudBackup={cloudBackup} saveBackup={backup => nativeFileExchange.saveBackup(role, backup)}
        deliver={nativeFileExchange.sendMessage} benchmark={role === 'wallet' ? benchmark : undefined} />
    </View>
    {!!fileNotice && <Text style={s.tiny}>{fileNotice}</Text>}
    {!!error && <Text style={s.errorText}>{error}</Text>}
    {!exchange && <Button title="Receive a file or message" onPress={() => { setExchange(true); setError(''); setFileNotice(''); }} />}
  </View>;
}
