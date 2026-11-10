import React, { useEffect, useMemo, useRef, useState } from 'react';
import { Pressable, ScrollView, StyleSheet, Text, TextInput, View, useWindowDimensions } from 'react-native';
import Svg, { Defs, LinearGradient, RadialGradient, Rect, Stop } from 'react-native-svg';
import AnzenApp, { Button } from './AnzenApp';
import { DeviceTransport, JsonObject, ProtocolMessage, Role, Tap, TapAnswer } from './transport';
import { colors, formatBtc, styles as s } from './theme';
import { closeBrowserDevice, createBrowserDevice, listBrowserDevices, openBrowserDevice, BrowserDevice, Settings } from './browserRuntime';
import { browserCloud } from './browserCloud';
import { browserBenchmark } from './browserBenchmark';
import { parseEncryptedBackup } from './fileExchange';

type Network = 'regtest' | 'signet' | 'bitcoin';
const options = new URLSearchParams(window.location.hash.slice(1));
// Opened from the local host's link: its regtest node is available as well. Mutinynet and
// mainnet run entirely in the browser.
const hostCapability = options.get('playground');
const networks: Network[] = hostCapability ? ['signet', 'bitcoin', 'regtest'] : ['signet', 'bitcoin'];
const networkNames: Record<Network, string> = { signet: 'Mutinynet', bitcoin: 'Mainnet', regtest: 'Regtest' };
const slots = ['wallet', 'signer'];

// What this browser used last: the network, and whether real bitcoin was accepted.
interface Prefs { network?: Network; mainnet?: boolean }
function prefs(): Prefs { try { return JSON.parse(localStorage.getItem('anzen-emulator') || '{}'); } catch { return {}; } }
function savePrefs(next: Prefs) { try { localStorage.setItem('anzen-emulator', JSON.stringify({ ...prefs(), ...next })); } catch { /* Only a convenience. */ } }
function initialNetwork(): Network | null {
  const linked = options.get('network') as Network | null, last = prefs().network;
  const chosen = linked && networks.includes(linked) ? linked : last && networks.includes(last) ? last : hostCapability ? 'regtest' : null;
  return chosen === 'bitcoin' && !prefs().mainnet ? null : chosen;
}
// Each network keeps its own devices, remembered by this browser. Regtest's belong to the host
// session, so they are kept under its capability. Nothing is written to the URL: the host's link
// keeps its capability and port, and `#network=…` only picks a network when opened.
const sessionKey = (network: Network) => network === 'regtest' ? `anzen-regtest-session:${hostCapability}` : `anzen-static-session:${network}`;
function savedDevices(network: Network) {
  try {
    const saved = localStorage.getItem(sessionKey(network));
    if (saved) return new URLSearchParams(saved);
  } catch { /* Only a convenience. */ }
  // Links from before kept regtest's devices in the URL.
  return network === 'regtest' && options.get('network') === 'regtest' ? options : new URLSearchParams();
}
function saveDevices(network: Network, ids: string[]) {
  const next = new URLSearchParams();
  ids.forEach((id, i) => next.set(slots[i], id));
  try { localStorage.setItem(sessionKey(network), next.toString()); } catch { /* Only a convenience. */ }
  const host = new URLSearchParams();
  if (hostCapability) { host.set('playground', hostCapability); if (options.get('port')) host.set('port', options.get('port')!); }
  window.history.replaceState(null, '', hostCapability ? `#${host}` : window.location.pathname + window.location.search);
}
const settingsFor = (network: Network): Settings => network === 'regtest'
  ? { endpoint, capability: hostCapability!, allowMainnet: false, network }
  : { endpoint: '', capability: `anzen-static:${network}`, allowMainnet: network === 'bitcoin', network };
const endpoint = `http://127.0.0.1:${Number(options.get('port') || '8787')}`;
async function request(path: string, capability: string, body: JsonObject): Promise<JsonObject> {
  const response = await fetch(endpoint + path, { method: 'POST', headers: {
    'Content-Type': 'application/json', Authorization: `Bearer ${capability}`,
  }, body: JSON.stringify(body) });
  const result = await response.json();
  if (!response.ok) throw new Error(result.error || `Request failed (${response.status})`);
  return result;
}

// A phone in the signer's finish: an anodised metal body with a bright machined edge, lit from
// the top left, a two-layer shadow, and black glass inset behind a thin bezel. `on` false (the
// signer, which draws its own body) leaves the children as they are.
function PhoneBody({ id, on, children }: { id: number; on: boolean; children: React.ReactNode }) {
  if (!on) return <>{children}</>;
  return <View style={{ flex: 1, borderRadius: 58, padding: 8, backgroundColor: '#16161a',
    boxShadow: [
      'inset 0 0 0 1.5px rgba(255,255,255,.2)', // the machined edge
      'inset 0 2px 1px -1px rgba(255,255,255,.3)', // its highlight along the top
      'inset 0 -3px 4px rgba(0,0,0,.45)', // and its shade along the bottom
      '0 30px 34px rgba(0,0,0,.55)', '0 8px 10px rgba(0,0,0,.4)',
    ].join(', ') } as object}>
    <View pointerEvents="none" style={[StyleSheet.absoluteFill, { borderRadius: 58, overflow: 'hidden' }]}>
      <Svg width="100%" height="100%">
        <Defs>
          <LinearGradient id={`phone-metal-${id}`} x1="0" y1="0" x2="1" y2="1">
            <Stop offset="0" stopColor="#26262c"/><Stop offset=".5" stopColor="#17171b"/><Stop offset="1" stopColor="#0f0f12"/>
          </LinearGradient>
          <RadialGradient id={`phone-glare-${id}`} cx="22%" cy="6%" r="70%">
            <Stop offset="0" stopColor="#fff" stopOpacity={.06}/><Stop offset=".6" stopColor="#fff" stopOpacity={0}/>
          </RadialGradient>
        </Defs>
        <Rect width="100%" height="100%" fill={`url(#phone-metal-${id})`}/>
        <Rect width="100%" height="100%" fill={`url(#phone-glare-${id})`}/>
      </Svg>
    </View>
    {/* The glass: black, with a faint edge where it meets the bezel. */}
    <View style={{ flex: 1, borderRadius: 50, overflow: 'hidden', backgroundColor: '#000', boxShadow: '0 0 0 1px rgba(0,0,0,.9), 0 0 0 1.5px rgba(255,255,255,.05)' } as object}>
      {children}
    </View>
  </View>;
}

// The playground's stand-in for the Bluetooth link between phone and signer: always in range,
// so connecting is immediate. Messages cross as JSON text, as they would over the radio.
function usePlaygroundLink() {
  const answer = useRef<TapAnswer | null>(null);
  return useMemo<Tap>(() => ({
    connect: async () => {
      const abort = new AbortController();
      const copy = (message: ProtocolMessage): ProtocolMessage => JSON.parse(JSON.stringify(message));
      return {
        request: async message => {
          if (abort.signal.aborted) throw new Error('The signer disconnected.');
          if (!answer.current) throw new Error('Your signer isn’t responding. Check it is on.');
          return copy(await answer.current(copy(message), abort.signal));
        },
        close: () => abort.abort(),
      };
    },
    answer: handler => {
      answer.current = handler;
      return () => { if (answer.current === handler) answer.current = null; };
    },
  }), []);
}

// A quiet text button: the playground's controls should not compete with the devices.
function Link({ title, text = title, onPress, disabled = false, danger = false, selected = false }: { title: string; text?: string; onPress: () => void; disabled?: boolean; danger?: boolean; selected?: boolean }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={title} disabled={disabled} onPress={onPress} style={({ pressed }) => ({ paddingVertical: 8, opacity: disabled && !selected ? .4 : pressed ? .6 : 1 })}>
    <Text style={{ color: danger ? colors.red : selected ? colors.text : colors.muted, fontSize: 12, fontWeight: selected ? '600' : '400' }}>{text}</Text>
  </Pressable>;
}
function Chip({ title, text, onPress, disabled, big = false }: { title: string; text: string; onPress: () => void; disabled: boolean; big?: boolean }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={title} disabled={disabled} onPress={onPress}
    style={({ pressed }) => ({ height: big ? 48 : 32, paddingHorizontal: big ? 18 : 13, borderRadius: big ? 24 : 16, justifyContent: 'center', backgroundColor: big ? 'rgba(255,255,255,.12)' : 'rgba(255,255,255,.07)', opacity: disabled ? .4 : pressed ? .7 : 1 })}>
    <Text style={{ color: colors.text, fontSize: 12 }}>{text}</Text>
  </Pressable>;
}

export default function Playground() {
  const admin = hostCapability || '';
  const [network, setNetwork] = useState<Network | null>(initialNetwork);
  const [confirmMainnet, setConfirmMainnet] = useState(false);
  const [devices, setDevices] = useState<DeviceTransport[]>([]);
  const [deviceIds, setDeviceIds] = useState<string[]>([]);
  const [cloudBackups, setCloudBackups] = useState<string[]>([]);
  const [cloudBackup, setCloudBackup] = useState<JsonObject | null>(null);
  // The newest backup this tab saved, kept across reloads so "Load latest backup" still works.
  const latestKey = (n: Network | null) => `anzen-latest-backup:${n === 'regtest' ? admin : n}`;
  const [lastExportedBackup, setLastExportedBackup] = useState<string | null>(() => { try { return sessionStorage.getItem(latestKey(network)); } catch { return null; } });
  const [inboxes, setInboxes] = useState<(ProtocolMessage | null)[]>([null, null]);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [switchingIndex, setSwitchingIndex] = useState<number | null>(null);
  const [controlNotice, setControlNotice] = useState('');
  const [fundAddress, setFundAddress] = useState('');
  const [fundAmount, setFundAmount] = useState('210000000');
  const [node, setNode] = useState<JsonObject | null>(null);
  const mainnet = network === 'bitcoin', regtest = network === 'regtest';
  const scope = network ? settingsFor(network).capability : '';
  const width = useWindowDimensions().width;
  const link = usePlaygroundLink();
  useEffect(() => { document.title = 'anzen emulator'; }, []);
  const sideBySide = width >= 850;
  const cloud = (body: JsonObject, n = network) => n === 'regtest' ? request('/cloud', admin, body) : browserCloud(`anzen-static:${n}`, body);
  async function refreshCloud(n = network) {
    const result = await cloud({ method: 'list' }, n);
    setCloudBackups(result.backups);
  }
  async function saveBackup(backup: JsonObject) {
    const saved = await cloud({ method: 'save', backup });
    setLastExportedBackup(saved.id);
    try { sessionStorage.setItem(latestKey(network), saved.id); } catch { /* Only a convenience. */ }
    await refreshCloud();
    return 'completed' as const;
  }
  async function loadBackup(id: string) {
    setError('');
    try { setCloudBackup(await cloud({ method: 'load', id })); }
    catch (e) { setError(String(e)); }
  }
  async function downloadBackup(id: string) {
    const backup = await cloud({ method: 'load', id });
    const url = URL.createObjectURL(new Blob([JSON.stringify(backup, null, 2)], { type: 'application/json' }));
    const link = document.createElement('a'); link.href = url; link.download = `anzen-backup-${id.slice(0, 8)}.json`;
    link.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  }
  // Open a network's phone and signer: the ones used last time, or a fresh pair.
  const opened = useRef<string[]>([]);
  async function open(next: Network, fresh = false) {
    setBusy(true); setError(''); setNetwork(next);
    opened.current.forEach(closeBrowserDevice); opened.current = [];
    setDevices([]); setDeviceIds([]); setNode(null); setCloudBackups([]); setCloudBackup(null);
    try { setLastExportedBackup(sessionStorage.getItem(latestKey(next))); } catch { setLastExportedBackup(null); }
    try {
      const settings = settingsFor(next);
      if (next === 'regtest') await request('/session', admin, {});
      const saved = fresh ? new URLSearchParams() : savedDevices(next);
      const existing = await listBrowserDevices(settings.capability);
      const ids: string[] = [], transports: DeviceTransport[] = [];
      for (const slot of slots) {
        const role: Role = slot === 'signer' ? 'signer' : 'wallet';
        const device = existing.find(item => item.id === saved.get(slot) && item.role === role)
          ?? await createBrowserDevice(role, settings);
        ids.push(device.id); transports.push(await openBrowserDevice(device, settings));
      }
      opened.current = ids;
      savePrefs({ network: next }); saveDevices(next, ids);
      setDevices(transports); setDeviceIds(ids); setInboxes(transports.map(() => null));
      void refreshCloud(next).catch(() => {});
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  useEffect(() => { if (network) void open(network); }, []);
  function choose(next: Network) {
    if (next === network && devices.length) return;
    if (next === 'bitcoin' && !prefs().mainnet) { setConfirmMainnet(true); return; }
    void open(next);
  }
  function acceptMainnet() { savePrefs({ mainnet: true }); setConfirmMainnet(false); void open('bitcoin'); }
  async function selectDevice(device: BrowserDevice, index: number) {
    setSwitchingIndex(index); setBusy(true);
    try {
      const selected = await openBrowserDevice(device, settingsFor(network!));
      setDevices(current => current.map((transport, i) => i === index ? selected : transport));
      setDeviceIds(current => {
        const ids = current.map((id, i) => i === index ? device.id : id);
        opened.current = ids; saveDevices(network!, ids);
        return ids;
      });
      setInboxes(current => current.map(() => null));
    } finally { setSwitchingIndex(null); setBusy(false); }
  }
  useEffect(() => {
    if (mainnet) return;
    (window as unknown as { anzenPlayground?: object }).anzenPlayground = {
      // Put an earlier device back in a slot, as if it had been found again.
      use: async (slot: string, id: string) => {
        const device = (await listBrowserDevices(scope)).find(item => item.id === id);
        if (!device) throw new Error(`No device ${id}`);
        await selectDevice(device, slots.indexOf(slot));
      },
    };
  });
  async function newDevice(role: Role, index: number) {
    setBusy(true); setError('');
    try {
      const device = await createBrowserDevice(role, settingsFor(network!));
      await selectDevice(device, index);
    } catch (e) { setError(String(e)); }
    finally { setBusy(false); }
  }
  async function control(command: JsonObject) {
    setBusy(true); setError('');
    setControlNotice(command.method === 'fund' ? 'Funding…' : 'Updating…');
    try { setNode(await request('/node', admin, command)); }
    catch (e) { setError(String(e)); }
    finally { setBusy(false); setControlNotice(''); }
  }
  function deliverFrom(index: number, message: ProtocolMessage) {
    const recipient = index === 0 ? 1 : 0;
    setInboxes(current => current.map((item, i) => i === recipient ? message : item));
    return 'completed' as const;
  }
  // One line for the chain: the block height, or what the last action did.
  const chainStatus = !node ? '' : node.amount_sats !== undefined ? `Funded ₿${formatBtc(node.amount_sats)}` : node.height !== undefined ? `Block ${node.height}` : '';
  return <ScrollView style={{ flex: 1, backgroundColor: '#121214' }} contentContainerStyle={{ padding: 32, alignItems: 'center' }}>
    <View style={{ maxWidth: 1220, width: '100%' }}>
      <View style={{ flexDirection: 'row', alignItems: 'center', flexWrap: 'wrap', gap: 18 }}>
        <Text style={[s.brand, { fontSize: 28 }]}>anzen emulator</Text>
        {/* The network switch. Each network keeps its own phone and signer. */}
        {network && <View accessibilityRole="tablist" style={{ flexDirection: 'row', gap: 4, padding: 3, borderRadius: 18, backgroundColor: 'rgba(255,255,255,.05)' }}>
          {networks.map(option => <Pressable key={option} accessibilityRole="tab" accessibilityLabel={networkNames[option]} aria-selected={option === network} disabled={busy} onPress={() => choose(option)}
            style={({ pressed }) => ({ height: 30, paddingHorizontal: 14, borderRadius: 15, justifyContent: 'center', backgroundColor: option === network ? 'rgba(255,255,255,.12)' : 'transparent', opacity: pressed ? .7 : 1 })}>
            <Text style={{ color: option === network ? colors.text : colors.muted, fontSize: 12.5, fontWeight: option === network ? '600' : '400' }}>{networkNames[option]}</Text>
          </Pressable>)}
        </View>}
        {mainnet && <Text style={[s.errorText, { fontWeight: '600' }]}>Real bitcoin</Text>}
        {network && !mainnet && devices.length > 0 && <Link title="Start over" onPress={() => void open(network, true)} disabled={busy}/>}
      </View>
      {confirmMainnet ? <View style={{ maxWidth: 420, marginTop: 40 }}>
        <Text style={s.heading}>Use real bitcoin?</Text>
        <Text style={s.subtext}>Mainnet uses real bitcoin in an experimental browser wallet. Don’t use it for savings.</Text>
        <Button title="Use mainnet" danger onPress={acceptMainnet}/>
        <Link title="Cancel" onPress={() => setConfirmMainnet(false)}/>
      </View> : !network ? <View style={{ maxWidth: 420, marginTop: 40 }}>
        {/* First visit: pick a network. Later visits open where you left off. */}
        <Text style={s.heading}>Choose a network</Text>
        <Button title="Mutinynet" primary onPress={() => void open('signet')}/>
        <Text style={[s.tiny, { marginBottom: 14 }]}>Test bitcoin, free to experiment with. Switch any time.</Text>
        <Button title="Mainnet" danger onPress={acceptMainnet}/>
        <Text style={s.tiny}>Real bitcoin, in an experimental browser wallet. Don’t use it for savings.</Text>
      </View> : !devices.length ? <View style={{ maxWidth: 420, marginTop: 40 }}>
        {busy ? <Text style={s.tiny}>Opening…</Text> : <Button title="Try again" primary onPress={() => void open(network)}/>}
      </View> : <View style={{ flexDirection: width >= 1180 ? 'row' : 'column', alignItems: 'center', justifyContent: 'center', gap: 32, marginTop: 24 }}>
        <View style={{ flexDirection: width >= 850 ? 'row' : 'column', flexWrap: 'wrap', maxWidth: width >= 1180 ? 778 : width - 64, gap: 28, alignSelf: width >= 1180 ? 'flex-start' : 'center' }}>
          {/* Keyed by position, so switching a device keeps its place. */}
          {devices.map((transport, index) => <View key={index}>
            {/* Phones get a phone frame; the hardware signer is shown on its own. */}
            <View testID={`device-${transport.role}`} style={transport.role === 'signer'
              ? { width: 375, height: sideBySide ? 812 : 450, maxWidth: width - 64 }
              : { width: 375, height: 812, maxWidth: width - 64 }}>
              <PhoneBody id={index} on={transport.role !== 'signer'}>
                {switchingIndex === index ? <View style={{ flex: 1, alignItems: 'center', justifyContent: 'center' }}><Text style={s.tiny}>Opening…</Text></View> : <AnzenApp key={deviceIds[index] || transport.role} transport={transport} network={network!} incoming={inboxes[index]} dismissIncoming={() => setInboxes(current => current.map((item, i) => i === index ? null : item))}
                  saveBackup={index === 0 ? saveBackup : undefined} cloudBackup={index === 0 ? cloudBackup : null}
                  tap={link} deliver={message => deliverFrom(index, message)} benchmark={index === 0 ? browserBenchmark : undefined} />}
              </PhoneBody>
            </View>
            {/* Simulate losing a device: the new one takes its place. */}
            {!mainnet && <View style={{ width: 375, maxWidth: width - 64, flexDirection: 'row', flexWrap: 'wrap', justifyContent: 'center', columnGap: 14, paddingTop: 10 }}>
              <Link title={transport.role === 'wallet' ? 'New wallet phone' : 'New signer device'} text={transport.role === 'wallet' ? 'New phone' : 'New signer'} disabled={busy} onPress={() => newDevice(transport.role, index)} />
            </View>}
          </View>)}
        </View>
        <View style={{ width: 280, alignSelf: width >= 1180 ? 'flex-start' : 'center', paddingTop: 42 }}>
          {regtest && <>
            <Text style={[s.eyebrow, { textAlign: 'left' }]}>Chain</Text>
            <TextInput accessibilityLabel="Funding address" placeholder="Address to fund" placeholderTextColor={colors.faint} value={fundAddress} onChangeText={setFundAddress} style={[s.input, { marginBottom: 8 }]} />
            <View style={{ flexDirection: 'row', gap: 8 }}>
              <TextInput accessibilityLabel="Funding amount in sats" value={fundAmount} onChangeText={setFundAmount} style={[s.input, { flex: 1 }]} />
              <Chip big title="Fund address" text="Fund" disabled={busy} onPress={() => control({ method: 'fund', address: fundAddress, amount_sats: Number(fundAmount) })} />
            </View>
            <View style={{ flexDirection: 'row', flexWrap: 'wrap', gap: 6, marginTop: 10 }}>
              <Chip title="Mine confirmation" text="Mine 1" disabled={busy} onPress={() => control({ method: 'mine', blocks: 1 })} />
              <Chip title="Advance 1 week" text="+1 week" disabled={busy} onPress={() => control({ method: 'advance_time', seconds: 7 * 86400 + 512 })} />
              <Chip title="Advance 31 days" text="+31 days" disabled={busy} onPress={() => control({ method: 'advance_time', seconds: 31 * 86400 })} />
              {/* Past the signer's ~388-day time lock, as if a year's renewal were missed. */}
              <Chip title="Advance 13 months" text="+13 months" disabled={busy} onPress={() => control({ method: 'advance_time', seconds: 396 * 86400 })} />
              <Chip title="Refresh chain status" text="↻" disabled={busy} onPress={() => control({ method: 'status' })} />
            </View>
            <Text style={[s.tiny, { marginTop: 10, minHeight: 17 }]} accessibilityRole={controlNotice ? 'alert' : undefined}>{controlNotice || chainStatus}</Text>
          </>}
          {!regtest && <Text style={s.tiny}>{mainnet ? 'Mainnet over electrum.lu.ke.' : 'Mutinynet: 30-second blocks, test bitcoin.'} Public servers see the addresses this wallet checks.</Text>}
          <Text style={[s.eyebrow, { textAlign: 'left', marginTop: 28 }]}>Cloud</Text>
          {cloudBackups.length === 0 && <Text style={s.tiny}>No backups yet.</Text>}
          {lastExportedBackup && <Link title="Load latest exported backup" text="Load latest backup" onPress={() => { void loadBackup(lastExportedBackup); }} />}
          {cloudBackups.map(id => <View key={id} style={{ flexDirection: 'row', columnGap: 14 }}>
            <Link title={`Load backup ${id.slice(0, 8)}`} onPress={() => { void loadBackup(id); }} />
            {!regtest && <Link title={`Download backup ${id.slice(0, 8)}`} text="Download" onPress={() => { void downloadBackup(id).catch(e => setError(String(e))); }} />}
          </View>)}
          <Link title="Refresh cloud backups" text="Refresh" onPress={() => { void refreshCloud().catch(e => setError(String(e))); }} />
          {!regtest && <>
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
