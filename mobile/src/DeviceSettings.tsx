import React, { useState } from 'react';
import { Pressable, Text, TextInput, View } from 'react-native';
import { Button } from './AnzenApp';
import { MenuRow } from './DarkSurfaces';
import { Role } from './transport';
import { colors, styles as s } from './theme';

// How this phone runs: which network, and whether it is the wallet or the vault's signer. A new
// phone starts as a wallet on Mutinynet. Each network and role keeps its own device, so switching
// never touches another network's wallet.
export type Network = 'signet' | 'bitcoin' | 'regtest';
export interface Rpc { url: string; user: string; password: string }
export interface DevicePrefs {
  network: Network; role: Role; rpc: Rpc;
  // This wallet's signer is a Ledger, reached over Bluetooth (a proof of concept).
  ledger: boolean;
}

export const networkNames: Record<Network, string> = { signet: 'Mutinynet', bitcoin: 'Mainnet', regtest: 'Regtest' };
const networkNotes: Record<Network, string> = {
  signet: 'Test bitcoin, 30-second blocks.',
  bitcoin: 'Real bitcoin. Experimental.',
  regtest: 'A local Bitcoin Core node.',
};

function Choice({ title, note, selected, onPress }: { title: string; note: string; selected: boolean; onPress: () => void }) {
  return <Pressable accessibilityRole="radio" accessibilityLabel={title} aria-checked={selected} onPress={onPress}
    style={({ pressed }) => ({ flexDirection: 'row', alignItems: 'center', paddingVertical: 12, paddingHorizontal: 4, opacity: pressed ? .6 : 1 })}>
    <View style={{ flex: 1 }}>
      <Text style={{ color: colors.text, fontSize: 14, fontWeight: '500' }}>{title}</Text>
      <Text style={[s.tiny, { color: colors.quiet, marginTop: 2 }]}>{note}</Text>
    </View>
    {selected && <Text style={{ color: colors.mint, fontSize: 14 }}>✓</Text>}
  </Pressable>;
}

export default function DeviceSettings({ prefs, onChange, onImport, ledgerAvailable = false }: {
  prefs: DevicePrefs;
  onChange: (prefs: DevicePrefs) => void;
  // This build can reach a Ledger over Bluetooth.
  ledgerAvailable?: boolean;
  // Opens the file import, for messages from the other device (no Bluetooth link yet).
  onImport?: () => void;
}) {
  const [open, setOpen] = useState<'network' | 'role' | 'ledger' | null>(null);
  const [picked, setPicked] = useState<Network>(prefs.network);
  const [rpc, setRpc] = useState<Rpc>(prefs.rpc);
  const signer = prefs.role === 'signer';
  const choose = (network: Network) => {
    setPicked(network);
    // Mutinynet needs nothing more; mainnet asks first and regtest needs its node.
    if (network === 'signet') { onChange({ ...prefs, network }); setOpen(null); }
  };
  return <View testID="device-settings" style={{ marginTop: 34 }}>
    <Text style={s.sheetEyebrow}>This device</Text>
    <View style={{ marginTop: 12 }}>
      <MenuRow title="Network" subtitle={networkNotes[prefs.network]} value={networkNames[prefs.network]}
        onPress={() => { setPicked(prefs.network); setRpc(prefs.rpc); setOpen(open === 'network' ? null : 'network'); }}/>
      {open === 'network' && <View style={{ paddingHorizontal: 8, paddingBottom: 8 }}>
        {(['signet', 'bitcoin', 'regtest'] as const).map(network => <Choice key={network} title={networkNames[network]}
          note={networkNotes[network]} selected={picked === network} onPress={() => choose(network)}/>)}
        {picked === 'bitcoin' && prefs.network !== 'bitcoin' && <>
          <Text style={[s.errorText, { marginTop: 8 }]}>Mainnet uses real bitcoin and public Electrum servers. This is experimental wallet software: don’t use funds you can’t afford to lose.</Text>
          <Button title="Use mainnet" danger onPress={() => { onChange({ ...prefs, network: 'bitcoin' }); setOpen(null); }}/>
        </>}
        {picked === 'regtest' && <>
          <Text style={s.label}>RPC URL (reachable from this device)</Text>
          <TextInput accessibilityLabel="RPC URL" style={s.input} value={rpc.url} onChangeText={url => setRpc({ ...rpc, url })} autoCapitalize="none" autoCorrect={false}/>
          <Text style={s.label}>RPC user</Text>
          <TextInput accessibilityLabel="RPC user" style={s.input} value={rpc.user} onChangeText={user => setRpc({ ...rpc, user })} autoCapitalize="none" autoCorrect={false}/>
          <Text style={s.label}>RPC password</Text>
          <TextInput accessibilityLabel="RPC password" style={s.input} value={rpc.password} onChangeText={password => setRpc({ ...rpc, password })} secureTextEntry/>
          <Button title={prefs.network === 'regtest' ? 'Save node' : 'Use regtest'} primary onPress={() => { onChange({ ...prefs, network: 'regtest', rpc }); setOpen(null); }}/>
        </>}
      </View>}
      <MenuRow title="Hardware wallet mode" subtitle={signer ? 'This device is your vault’s signer.' : 'Use this device as your vault’s signer.'}
        value={signer ? 'On' : 'Off'} onPress={() => setOpen(open === 'role' ? null : 'role')}/>
      {open === 'role' && <View style={{ paddingHorizontal: 8, paddingBottom: 8 }}>
        <Text style={[s.tiny, { color: colors.quiet, marginTop: 4 }]}>
          {signer ? 'This device goes back to being a wallet. Its signer stays on the device for when you turn this on again.'
            : 'This device becomes the second key that approves your vault’s policy. Use a separate device from your wallet. A phone is not a secure-element hardware wallet. Its wallet stays on the device for when you turn this off.'}
        </Text>
        <Button title={signer ? 'Turn off' : 'Turn on'} primary onPress={() => { onChange({ ...prefs, role: signer ? 'wallet' : 'signer' }); setOpen(null); }}/>
      </View>}
      {ledgerAvailable && !signer && <>
        <MenuRow title="Ledger signer" subtitle={prefs.ledger ? 'Your vault’s signer is a Ledger, over Bluetooth.' : 'Use a Ledger as your vault’s signer.'}
          value={prefs.ledger ? 'On' : 'Off'} onPress={() => setOpen(open === 'ledger' ? null : 'ledger')}/>
        {open === 'ledger' && <View style={{ paddingHorizontal: 8, paddingBottom: 8 }}>
          <Text style={[s.tiny, { color: colors.quiet, marginTop: 4 }]}>
            {prefs.ledger ? 'Vault requests go back to travelling as files, for a phone in hardware wallet mode.'
              : 'A proof of concept: test bitcoin only. It needs the Anzen app on a Ledger Flex or Stax, unlocked and open. A Ledger can’t protect this phone’s recovery backup yet, so keep its recovery phrase, and it can renew a vault of at most 16 deposits.'}
          </Text>
          <Button title={prefs.ledger ? 'Turn off' : 'Turn on'} primary onPress={() => { onChange({ ...prefs, ledger: !prefs.ledger }); setOpen(null); }}/>
        </View>}
      </>}
      {onImport && <MenuRow title="Import a message or backup" subtitle="A file from your other device." onPress={onImport}/>}
    </View>
  </View>;
}
