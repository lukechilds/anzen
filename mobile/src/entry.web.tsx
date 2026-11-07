import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Animated, Easing, Pressable, ScrollView, Text, TextInput, View, useWindowDimensions } from 'react-native';
import AnzenApp, { Button } from './AnzenApp';
import { DeviceTransport, JsonObject, ProtocolMessage, Role, SignerLink, Tap, TapAnswer } from './transport';
import { colors, styles as s } from './theme';
import { createBrowserDevice, listBrowserDevices, openBrowserDevice, BrowserDevice, Settings } from './browserRuntime';
import { browserCloud } from './browserCloud';
import { parseEncryptedBackup } from './fileExchange';

const options = new URLSearchParams(window.location.hash.slice(1));
const staticMode = !options.has('playground') && !options.has('port');
function savedSession(network: string) {
  if (!staticMode) return options;
  const current = new URLSearchParams(window.location.hash.slice(1));
  if (current.get('network') === network && current.has('wallet') && current.has('signer')) return current;
  return new URLSearchParams(localStorage.getItem(`anzen-static-session:${network}`) || `network=${network}`);
}
function saveLocation(next: URLSearchParams) {
  window.history.replaceState(null, '', `#${next}`);
  if (staticMode) localStorage.setItem(`anzen-static-session:${next.get('network')}`, next.toString());
}
const endpoint = `http://127.0.0.1:${Number(options.get('port') || '8787')}`;
async function request(path: string, capability: string, body: JsonObject): Promise<JsonObject> {
  const response = await fetch(endpoint + path, { method: 'POST', headers: {
    'Content-Type': 'application/json', Authorization: `Bearer ${capability}`,
  }, body: JSON.stringify(body) });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || `Request failed (${response.status})`);
  return result;
}

// The playground's stand-in for holding the signer to the phone. The phone waits for a tap;
// "Hold signer to phone" connects them. Messages cross as JSON text, as they would over a radio.
function usePlaygroundTap(nudge: () => Promise<void>) {
  const [waiting, setWaiting] = useState(false);
  const [connected, setConnected] = useState(false);
  const waiter = useRef<((link: SignerLink) => void) | null>(null);
  const answer = useRef<TapAnswer | null>(null);
  const tap = useMemo<Tap>(() => ({
    connect: signal => new Promise((resolve, reject) => {
      waiter.current = resolve; setWaiting(true);
      signal.addEventListener('abort', () => {
        if (waiter.current === resolve) { waiter.current = null; setWaiting(false); }
        reject(new Error('Cancelled'));
      }, { once: true });
    }),
    answer: handler => {
      answer.current = handler;
      return () => { if (answer.current === handler) answer.current = null; };
    },
  }), []);
  async function touch() {
    const resolve = waiter.current;
    if (!resolve) return;
    waiter.current = null; setWaiting(false);
    await nudge();
    setConnected(true);
    const abort = new AbortController();
    const copy = (message: ProtocolMessage): ProtocolMessage => JSON.parse(JSON.stringify(message));
    resolve({
      request: async message => {
        if (abort.signal.aborted) throw new Error('The signer moved away.');
        if (!answer.current) throw new Error('The signer didn’t respond. Check it is open.');
        return copy(await answer.current(copy(message), abort.signal));
      },
      close: () => { abort.abort(); setConnected(false); },
    });
  }
  return { tap, waiting, connected, touch };
}

export default function Playground() {
  const [admin, setAdmin] = useState(options.get('playground') || '');
  const [devices, setDevices] = useState<DeviceTransport[]>([]);
  const [deviceIds, setDeviceIds] = useState<string[]>([]);
  const [knownDevices, setKnownDevices] = useState<BrowserDevice[]>([]);
  const [cloudBackups, setCloudBackups] = useState<string[]>([]);
  const [cloudBackup, setCloudBackup] = useState<JsonObject | null>(null);
  const [lastExportedBackup, setLastExportedBackup] = useState<string | null>(null);
  const [inboxes, setInboxes] = useState<(ProtocolMessage | null)[]>([null, null]);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [switchingIndex, setSwitchingIndex] = useState<number | null>(null);
  const [controlNotice, setControlNotice] = useState('');
  const [fundAddress, setFundAddress] = useState('');
  const [fundAmount, setFundAmount] = useState('210000000');
  const [node, setNode] = useState<JsonObject | null>(null);
  const [mainnet, setMainnet] = useState(options.get('network') === 'bitcoin');
  const [mainnetConsent, setMainnetConsent] = useState(false);
  const network = mainnet ? 'bitcoin' : staticMode ? 'signet' : 'regtest';
  const sessionOptions = savedSession(network);
  const scope = staticMode ? `anzen-static:${network}` : admin;
  const [hostMainnet, setHostMainnet] = useState(false);
  const runtimeSettings: Settings = { endpoint: staticMode ? '' : endpoint, capability: scope, allowMainnet: mainnet && (staticMode ? mainnetConsent : hostMainnet), network };
  const [mining, setMining] = useState<JsonObject | null>(null);
  const [miningBlocks, setMiningBlocks] = useState('61200');
  const miningActive = Boolean(mining && ['queued', 'running'].includes(mining.status));
  useEffect(() => {
    if (!mining || !['queued', 'running'].includes(mining.status)) return;
    let active = true;
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      try {
        const next = await request('/node/job', admin, { method: 'status', id: mining.id });
        if (active) setMining(next);
      } catch (e) { if (active) setError(String(e)); }
      if (active) timer = setTimeout(poll, 1000);
    };
    timer = setTimeout(poll, 500);
    return () => { active = false; clearTimeout(timer); };
  }, [mining?.id, mining?.status, admin]);
  const width = useWindowDimensions().width;
  // The signer frame moves into the wallet frame for a tap, then settles back.
  const touchMotion = useRef(new Animated.Value(0)).current;
  const nudge = () => new Promise<void>(resolve => {
    Animated.timing(touchMotion, { toValue: 1, duration: 260, easing: Easing.out(Easing.cubic), useNativeDriver: false }).start(() => {
      resolve();
      Animated.timing(touchMotion, { toValue: 0, duration: 420, delay: 160, easing: Easing.inOut(Easing.cubic), useNativeDriver: false }).start();
    });
  });
  const playgroundTap = usePlaygroundTap(nudge);
  const sideBySide = width >= 850;
  async function restoreMining() {
    if (mainnet || staticMode) return;
    const result = await request('/node/job', admin, { method: 'list' });
    const savedId = new URLSearchParams(window.location.hash.slice(1)).get('mining');
    setMining(result.jobs.find((job: JsonObject) => ['queued', 'running'].includes(job.status))
      || result.jobs.find((job: JsonObject) => job.id === savedId) || null);
  }
  async function refreshCloud() {
    const result = await cloud({ method: 'list' });
    setCloudBackups(result.backups);
  }
  async function saveBackup(backup: JsonObject) {
    const saved = await cloud({ method: 'save', backup });
    setLastExportedBackup(saved.id);
    await refreshCloud();
    return 'completed' as const;
  }
  async function loadBackup(id: string) {
    setError('');
    try { setCloudBackup(await cloud({ method: 'load', id })); }
    catch (e) { setError(String(e)); }
  }
  const cloud = (body: JsonObject) => staticMode ? browserCloud(scope, body) : request('/cloud', admin, body);
  async function downloadBackup(id: string) {
    const backup = await cloud({ method: 'load', id });
    const url = URL.createObjectURL(new Blob([JSON.stringify(backup, null, 2)], { type: 'application/json' }));
    const link = document.createElement('a'); link.href = url; link.download = `anzen-backup-${id.slice(0, 8)}.json`;
    link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  async function connectionSettings() {
    if (staticMode) {
      if (mainnet && !mainnetConsent) throw new Error('Explicitly enable mainnet before opening real-bitcoin devices.');
      return runtimeSettings;
    }
    const settings = await request('/session', admin, {});
    setHostMainnet(settings.allow_mainnet);
    return { ...runtimeSettings, allowMainnet: mainnet && settings.allow_mainnet };
  }
  async function start() {
    setBusy(true); setError('');
    try {
      const runtimeSettings = await connectionSettings();
      if (staticMode && mainnet && sessionOptions.has('wallet') && sessionOptions.has('signer')) {
        throw new Error('Saved mainnet devices already exist. Resume them instead of creating replacements.');
      }
      const transports: DeviceTransport[] = [];
      const deviceIds: string[] = [];
      const created: BrowserDevice[] = [];
      for (const role of ['wallet', 'signer'] as Role[]) {
        const device = await createBrowserDevice(role, runtimeSettings);
        created.push(device);
        deviceIds.push(device.id);
        transports.push(await openBrowserDevice(device, runtimeSettings));
      }
      await restoreMining();
      setDevices(transports);
      setDeviceIds(deviceIds); setKnownDevices(staticMode ? await listBrowserDevices(scope) : created);
      const next = new URLSearchParams(window.location.hash.slice(1));
      next.delete('contact'); next.set('wallet', deviceIds[0]); next.set('signer', deviceIds[1]); next.set('network', network);
      saveLocation(next);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function resume() {
    setBusy(true); setError('');
    try {
      if ((sessionOptions.get('network') === 'bitcoin') !== mainnet) throw new Error('Choose the saved session’s network before resuming its devices.');
      const runtimeSettings = await connectionSettings();
      const existing = await listBrowserDevices(scope);
      const slots = sessionOptions.get('contact') ? ['wallet', 'signer', 'contact'] : ['wallet', 'signer'];
      const restored = await Promise.all(slots.map(async slot => {
        const role: Role = slot === 'signer' ? 'signer' : 'wallet';
        const device = existing.find(item => item.id === sessionOptions.get(slot) && item.role === role);
        if (!device) throw new Error('This browser does not contain the saved device. Use its original browser profile or a recovery backup.');
        return openBrowserDevice(device, runtimeSettings);
      }));
      await restoreMining();
      setDevices(restored);
      setDeviceIds(slots.map(slot => sessionOptions.get(slot)!));
      setInboxes(slots.map(() => null));
      setKnownDevices(existing);
      saveLocation(new URLSearchParams(sessionOptions));
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function selectDevice(device: BrowserDevice, index: number) {
    setSwitchingIndex(index); setBusy(true);
    try {
      const selected = await openBrowserDevice(device, runtimeSettings);
      setDevices(current => current.map((transport, i) => i === index ? selected : transport));
      setDeviceIds(current => current.map((id, i) => i === index ? device.id : id));
      setInboxes(current => current.map(() => null));
      const next = new URLSearchParams(window.location.hash.slice(1));
      next.set(index === 2 ? 'contact' : device.role, device.id);
      saveLocation(next);
    } finally { setSwitchingIndex(null); setBusy(false); }
  }
  async function newDevice(role: Role, index: number) {
    setBusy(true); setError('');
    try {
      const device = await createBrowserDevice(role, runtimeSettings);
      setKnownDevices(current => [...current, device]);
      await selectDevice(device, index);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function addContactDevice() {
    setBusy(true); setError('');
    try {
      const device = await createBrowserDevice('wallet', runtimeSettings);
      const transport = await openBrowserDevice(device, runtimeSettings);
      setKnownDevices(current => [...current, device]);
      setDevices(current => [...current, transport]);
      setDeviceIds(current => [...current, device.id]);
      setInboxes(current => [...current, null]);
      const next = new URLSearchParams(window.location.hash.slice(1));
      next.set('contact', device.id); saveLocation(next);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function control(command: JsonObject) {
    setBusy(true); setError('');
    setControlNotice(command.method === 'fund' ? 'Funding a real regtest output. Initial faucet setup may take a while.' : 'Updating the regtest chain…');
    try { setNode(await request('/node', admin, command)); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); setControlNotice(''); }
  }
  function deliverFrom(index: number, message: ProtocolMessage) {
    const recipient = index === 0 ? (message.kind === 'contact-restore-proposal' ? 2 : 1) : 0;
    if (!devices[recipient]) throw new Error('Add a recovery contact device before sending this request.');
    setInboxes(current => current.map((item, i) => i === recipient ? message : item));
    return 'completed' as const;
  }
  async function miningJob(command: JsonObject) {
    setError('');
    try {
      const result = await request('/node/job', admin, command);
      setMining(result);
      const next = new URLSearchParams(window.location.hash.slice(1));
      next.set('mining', result.id);
      window.history.replaceState(null, '', `#${next}`);
    }
    catch (e) { setError(String(e)); }
  }
  return <ScrollView style={{ flex: 1, backgroundColor: '#121214' }} contentContainerStyle={{ padding: 32, alignItems: 'center' }}>
    <View style={{ maxWidth: 1220, width: '100%' }}>
      <Text style={[s.brand, { fontSize: 32 }]}>anzen <Text style={{ color: colors.faint, fontSize: 20 }}> / playground</Text></Text>
      <Text style={s.subtext}>Browser-local Rust · independent WASM devices · {staticMode ? `direct ${mainnet ? 'mainnet HTTPS' : 'Mutinynet Electrum WSS'} · no Anzen backend` : `real ${mainnet ? 'mainnet Electrum' : 'regtest'} transactions`}.</Text>
      {!devices.length ? <View style={{ maxWidth: 520 }}>
        {!staticMode && <><Text style={s.label}>Local playground capability</Text><TextInput accessibilityLabel="Local playground capability" secureTextEntry value={admin} onChangeText={setAdmin} style={s.input} />
        <Text style={s.subtext}>Start the local Rust host, then open the private link it prints. The capability stays in browser memory, not local storage.</Text></>}
        {staticMode && <Text style={s.subtext}>Wallet keys and signing run in this browser. Mutinynet uses test bitcoin. No local server or Docker is needed. Both device screens are simulated here, not independent hardware security.</Text>}
        {staticMode ? <>
          {mainnet && <Button title="Use Mutinynet instead" onPress={() => { setMainnet(false); setMainnetConsent(false); }} />}
          {(!mainnet || !mainnetConsent) && <Button title="Dangerously enable mainnet" danger onPress={() => { setMainnet(true); setMainnetConsent(true); }} />}
        </> : <Button title={mainnet ? 'Use regtest instead' : 'Dangerously enable mainnet'} danger={!mainnet} onPress={() => setMainnet(value => !value)} />}
        {mainnet && <Text style={s.errorText}>{staticMode ? 'Real bitcoin. Experimental browser wallet. Browser compromise can expose both simulated devices. Do not use this demo to secure savings. Explicit mainnet consent is required each time you reopen it.' : 'Real bitcoin. Experimental software. The Rust host must also be started with --dangerously-enable-mainnet. These devices use newly created, isolated state and real Electrum servers.'}</Text>}
        {sessionOptions.get('wallet') && sessionOptions.get('signer') && <Button title="Resume playground" primary onPress={resume} disabled={busy} />}
        {!(staticMode && mainnet && sessionOptions.has('wallet') && sessionOptions.has('signer')) && <Button title="Start playground" primary onPress={start} disabled={busy} />}</View> : <View style={{ flexDirection: width >= 1180 ? 'row' : 'column', alignItems: 'center', justifyContent: 'center', gap: 32, marginTop: 24 }}>
        <View style={{ flexDirection: width >= 850 ? 'row' : 'column', flexWrap: 'wrap', maxWidth: width >= 1180 ? 778 : width - 64, gap: 28, alignSelf: width >= 1180 ? 'flex-start' : 'center' }}>
          {devices.map((transport, index) => <Animated.View key={deviceIds[index] || transport.role} style={index === 1 && { zIndex: 2, transform: sideBySide
            ? [{ translateX: touchMotion.interpolate({ inputRange: [0, 1], outputRange: [0, -70] }) }, { rotate: touchMotion.interpolate({ inputRange: [0, 1], outputRange: ['0deg', '-3deg'] }) }]
            : [{ translateY: touchMotion.interpolate({ inputRange: [0, 1], outputRange: [0, -70] }) }] }}>
            <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between', minHeight: 34, marginBottom: 8 }}>
              <Text style={[s.eyebrow, { textAlign: 'left', marginLeft: 14, marginBottom: 0 }]}>{index + 1} / {index === 2 ? 'Recovery contact’s phone' : transport.role === 'wallet' ? 'Wallet phone' : 'Signer'}</Text>
              {index === 1 && playgroundTap.waiting && <Pressable accessibilityRole="button" accessibilityLabel="Hold signer to phone" onPress={() => void playgroundTap.touch()}
                style={({ pressed }) => ({ height: 34, paddingHorizontal: 16, borderRadius: 17, justifyContent: 'center', backgroundColor: 'rgba(167,227,187,.16)', boxShadow: '0 0 0 1px rgba(167,227,187,.45), 0 0 24px rgba(167,227,187,.25)', transform: [{ scale: pressed ? .95 : 1 }] })}>
                <Text style={{ color: colors.mint, fontSize: 12.5, fontWeight: '600' }}>{sideBySide ? '← ' : '↑ '}Hold signer to phone</Text>
              </Pressable>}
              {index === 1 && playgroundTap.connected && <Text style={{ color: colors.mint, fontSize: 11.5, marginRight: 14 }}>Connected</Text>}
            </View>
            <View testID={index === 2 ? 'device-contact' : `device-${transport.role}`} style={{ width: 375, height: 812, maxWidth: width - 64, borderRadius: 58, overflow: 'hidden', padding: 10, backgroundColor: '#0a0a0c',
              boxShadow: index < 2 && (playgroundTap.waiting || playgroundTap.connected) ? '0 0 0 1.5px rgba(167,227,187,.5), 0 0 40px rgba(167,227,187,.18), 0 34px 80px rgba(20,18,14,.30)' : '0 34px 80px rgba(20,18,14,.30)' }}>
              {switchingIndex === index ? <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center' }}><Text style={s.subtext}>Opening device…</Text></View> : <AnzenApp transport={transport} network={network} incoming={inboxes[index]} dismissIncoming={() => setInboxes(current => current.map((item, i) => i === index ? null : item))}
                saveBackup={index === 0 ? saveBackup : undefined} cloudBackup={index === 0 ? cloudBackup : null}
                tap={index < 2 ? playgroundTap.tap : undefined} deliver={message => deliverFrom(index, message)} />}
            </View>
            {!mainnet && <View style={{ width: 375, maxWidth: width - 64, paddingTop: 14 }}>
              <Text style={s.tiny}>Test devices · switching keeps each device’s independent state. A fresh device cannot access the previous device’s key.</Text>
              <Button title={index === 2 ? 'New contact device' : transport.role === 'wallet' ? 'New wallet phone' : 'New signer device'} disabled={busy} onPress={() => newDevice(transport.role, index)} />
              {knownDevices.filter(device => device.role === transport.role).length > 1 && knownDevices.filter(device => device.role === transport.role && !deviceIds.some((id, i) => i !== index && id === device.id)).map(device =>
                <Button key={device.id} title={`${index === 2 ? 'Contact' : transport.role === 'wallet' ? 'Wallet' : 'Signer'} ${device.id.slice(0, 6)}`} disabled={busy || device.id === deviceIds[index]} onPress={() => { void selectDevice(device, index).catch(e => setError(String(e))); }} />)}
            </View>}
          </Animated.View>)}
        </View>
        <View style={{ width: 280, alignSelf: width >= 1180 ? 'flex-start' : 'center', paddingTop: 25 }}>
          {!mainnet && !staticMode && <>
          <Text style={s.heading}>Regtest controls</Text><Text style={s.subtext}>Mining and time travel affect the shared test chain, not the device keys.</Text>
          {devices.length === 2 && <Button title="Add recovery contact device" disabled={busy} onPress={addContactDevice} />}
          {!!controlNotice && <Text accessibilityRole="alert" style={s.noticeText}>{controlNotice}</Text>}
          <Text style={s.label}>Funding address</Text><TextInput accessibilityLabel="Funding address" value={fundAddress} onChangeText={setFundAddress} style={s.input} />
          <Text style={s.label}>Funding amount in sats</Text><TextInput accessibilityLabel="Funding amount in sats" value={fundAmount} onChangeText={setFundAmount} style={s.input} />
          <Button title="Fund address" primary disabled={busy || miningActive} onPress={() => control({ method: 'fund', address: fundAddress, amount_sats: Number(fundAmount) })} />
          <Button title="Mine confirmation" disabled={busy || miningActive} onPress={() => control({ method: 'mine', blocks: 1 })} />
          <Button title="Advance 31 days" disabled={busy || miningActive} onPress={() => control({ method: 'advance_time', seconds: 31 * 86400 })} />
          <Button title="Advance 1 week" disabled={busy || miningActive} onPress={() => control({ method: 'advance_time', seconds: 7 * 86400 + 512 })} />
          <Button title="Refresh chain status" disabled={busy || miningActive} onPress={() => control({ method: 'status' })} />
          <Text style={s.label}>Recovery mining blocks</Text><TextInput accessibilityLabel="Recovery mining blocks" value={miningBlocks} onChangeText={setMiningBlocks} style={s.input} keyboardType="number-pad" />
          <Button title="Start recovery mining" disabled={busy || miningActive} onPress={() => miningJob({ method: 'start', blocks: Number(miningBlocks) })} />
          {mining && <View style={s.notice}><Text style={s.noticeText}>Mining · {mining.status}</Text><Text style={s.subtext}>{mining.mined_blocks} / {mining.requested_blocks} real blocks</Text>
            {['queued', 'running'].includes(mining.status) && <><Text style={s.tiny}>This may take a while. Cancelling keeps blocks already mined.</Text><Button title="Cancel mining" onPress={() => miningJob({ method: 'cancel', id: mining.id })} /></>}
            {mining.error && <Text style={s.errorText}>{mining.error}</Text>}
          </View>}
          {node && <Text selectable style={s.mono}>{JSON.stringify(node, null, 2)}</Text>}
          <Text style={s.tiny}>Local development playground. Full loss/theft scenarios and native device verification remain in progress. No existing wallet is imported.</Text>
          </>}
          {staticMode && <>
            <Text style={s.heading}>{mainnet ? 'Mainnet' : 'Mutinynet'}</Text>
            <Text style={s.subtext}>{mainnet ? 'electrum.lu.ke Electrum WSS' : 'Mutinynet / Arkade Electrum WSS'} · Public indexers can see queried addresses and transaction history. Confirmations use the real network clock; no mining or time travel controls.</Text>
            {!mainnet && <Text style={s.tiny}>Mutinynet targets 30-second blocks. Recovery still requires 61,200 / 65,535 blocks (~21 / 23 days here). The emergency delay is still one week. Electrum sync uses a randomly selected server and automatically tries the other if it fails.</Text>}
            {devices.length === 2 && <Button title="Add recovery contact device" disabled={busy} onPress={addContactDevice} />}
          </>}
          <Text style={s.heading}>Local cloud simulation</Text>
          <Text style={s.subtext}>{staticMode ? 'Encrypted backups are stored separately in this browser’s IndexedDB. This is not an off-device backup: clearing site data can erase the wallet AND its simulated cloud. Download a copy and keep it elsewhere.' : 'Encrypted backups live outside both device folders. They are not delivered to the signer.'}</Text>
          <Button title="Refresh cloud backups" onPress={() => { void refreshCloud().catch(e => setError(String(e))); }} />
          {lastExportedBackup && <Button title="Load latest exported backup" onPress={() => { void loadBackup(lastExportedBackup); }} />}
          {cloudBackups.map(id => <Button key={id} title={`Load backup ${id.slice(0, 8)}`} onPress={() => { void loadBackup(id); }} />)}
          {staticMode && <>
            {cloudBackups.map(id => <Button key={`download-${id}`} title={`Download backup ${id.slice(0, 8)}`} onPress={() => { void downloadBackup(id).catch(e => setError(String(e))); }} />)}
            <Text style={s.label}>Import encrypted cloud backup</Text>
            <input aria-label="Import encrypted cloud backup" type="file" accept="application/json,.json" style={{ color: colors.muted, maxWidth: '100%' }} onChange={event => {
              const file = event.target.files?.[0]; event.target.value = '';
              if (!file) return;
              void (async () => {
                if (file.size > 8 * 1024 * 1024) throw new Error('Backup exceeds 8 MiB');
                await saveBackup(parseEncryptedBackup(await file.text()));
              })().catch(e => setError(String(e)));
            }} />
          </>}
        </View>
      </View>}
      {!!error && <Text selectable style={s.errorText}>{error}</Text>}
    </View>
  </ScrollView>;
}
