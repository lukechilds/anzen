import React, { useRef, useState } from 'react';
import { View } from 'react-native';
import { Text, TextInput } from './ui/Text';
import { Option, SelectRow, ToggleRow, asksInline, confirmDanger } from './ui/Rows';
import { Button } from './AnzenApp';
import { MenuRow } from './DarkSurfaces';
import { Role } from './transport';
import { styles as s } from './theme';

// How this phone runs: which network, and whether it is the wallet or the vault's signer. A new
// phone starts as a wallet on Mutinynet. Each network and role keeps its own device, so switching
// never touches another network's wallet.
export type Network = 'signet' | 'bitcoin' | 'regtest';
export interface Rpc { url: string; user: string; password: string }
export interface DevicePrefs {
  network: Network; role: Role; rpc: Rpc;
  // This wallet's signer is a Ledger, reached over Bluetooth (a proof of concept).
  ledger: boolean;
  // Or a Trezor (also a proof of concept). At most one of the two.
  trezor: boolean;
}

export const networkNames: Record<Network, string> = { signet: 'Mutinynet', bitcoin: 'Mainnet', regtest: 'Regtest' };
const networks: Option<Network>[] = [
  { key: 'signet', label: 'Mutinynet', note: 'Test bitcoin, 30-second blocks.' },
  { key: 'bitcoin', label: 'Mainnet', note: 'Real bitcoin. Experimental.' },
  { key: 'regtest', label: 'Regtest', note: 'A local Bitcoin Core node.' },
];
const MAINNET_WARNING = 'Mainnet uses real bitcoin and public Electrum servers. This is experimental wallet software: don’t use funds you can’t afford to lose.';

export default function DeviceSettings({ prefs, onChange, onImport, ledgerAvailable = false, trezorAvailable = false }: {
  prefs: DevicePrefs;
  onChange: (prefs: DevicePrefs) => void;
  // This build can reach a Ledger over Bluetooth.
  ledgerAvailable?: boolean;
  // This build can reach a Trezor.
  trezorAvailable?: boolean;
  // Opens the file import, for messages from the other device (no Bluetooth link yet).
  onImport?: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [picked, setPicked] = useState<Network>(prefs.network);
  const [rpc, setRpc] = useState<Rpc>(prefs.rpc);
  const signer = prefs.role === 'signer';
  // Return moves through the node's fields, and applies them from the last.
  const userInput = useRef<React.ComponentRef<typeof TextInput>>(null), passwordInput = useRef<React.ComponentRef<typeof TextInput>>(null);
  const useNode = () => { onChange({ ...prefs, network: 'regtest', rpc }); setOpen(false); setPicked('regtest'); };
  const choose = async (network: Network) => {
    setPicked(network);
    // Mutinynet needs nothing more; mainnet asks first and regtest needs its node.
    if (network === 'signet') { onChange({ ...prefs, network }); setOpen(false); }
    // A device asks in an alert; the emulator shows the warning under the row.
    if (network === 'bitcoin' && prefs.network !== 'bitcoin' && !asksInline) {
      if (await confirmDanger('Use mainnet?', MAINNET_WARNING, 'Use mainnet')) onChange({ ...prefs, network: 'bitcoin' });
      else setPicked(prefs.network);
    }
  };
  return <View testID="device-settings" style={{ marginTop: 34 }}>
    <Text style={s.sheetEyebrow}>This device</Text>
    <View style={{ marginTop: 12 }}>
      <SelectRow title="Network" subtitle={networks.find(network => network.key === prefs.network)!.note} options={networks} selected={prefs.network} onSelect={network => void choose(network)}
        picked={picked} open={open} onOpen={next => { setPicked(prefs.network); setRpc(prefs.rpc); setOpen(next); }}>
        {asksInline && picked === 'bitcoin' && prefs.network !== 'bitcoin' && <>
          <Text style={[s.errorText, { marginTop: 8 }]}>{MAINNET_WARNING}</Text>
          <Button title="Use mainnet" danger onPress={() => { onChange({ ...prefs, network: 'bitcoin' }); setOpen(false); }}/>
        </>}
        {picked === 'regtest' && (asksInline || prefs.network !== 'regtest' || open) && <View style={!asksInline && { paddingHorizontal: 8, paddingBottom: 8 }}>
          <Text style={s.label}>RPC URL (reachable from this device)</Text>
          <TextInput accessibilityLabel="RPC URL" style={s.input} value={rpc.url} onChangeText={url => setRpc({ ...rpc, url })} autoCapitalize="none" autoCorrect={false} spellCheck={false} autoComplete="off" keyboardType="url" returnKeyType="next" submitBehavior="submit" onSubmitEditing={() => userInput.current?.focus()}/>
          <Text style={s.label}>RPC user</Text>
          <TextInput ref={userInput} accessibilityLabel="RPC user" style={s.input} value={rpc.user} onChangeText={user => setRpc({ ...rpc, user })} autoCapitalize="none" autoCorrect={false} spellCheck={false} autoComplete="off" returnKeyType="next" submitBehavior="submit" onSubmitEditing={() => passwordInput.current?.focus()}/>
          <Text style={s.label}>RPC password</Text>
          <TextInput ref={passwordInput} accessibilityLabel="RPC password" style={s.input} value={rpc.password} onChangeText={password => setRpc({ ...rpc, password })} secureTextEntry autoComplete="off" returnKeyType="done" onSubmitEditing={useNode}/>
          <Button title={prefs.network === 'regtest' ? 'Save node' : 'Use regtest'} primary onPress={useNode}/>
          {!asksInline && prefs.network !== 'regtest' && <Button title="Cancel" onPress={() => setPicked(prefs.network)}/>}
        </View>}
      </SelectRow>
      {/* A regtest node's address can change without changing network. */}
      {!asksInline && prefs.network === 'regtest' && picked === 'regtest' && !open && <MenuRow title="Regtest node" subtitle={prefs.rpc.url} onPress={() => { setRpc(prefs.rpc); setOpen(true); }}/>}
      <ToggleRow title="Hardware wallet mode" subtitle={signer ? 'This device is your vault’s signer.' : 'Use this device as your vault’s signer.'} value={signer}
        question={signer ? 'This device goes back to being a wallet. Its signer stays on the device for when you turn this on again.'
          : 'This device becomes the second key that approves your vault’s policy. Use a separate device from your wallet. A phone is not a secure-element hardware wallet. Its wallet stays on the device for when you turn this off.'}
        onChange={on => onChange({ ...prefs, role: on ? 'signer' : 'wallet' })}/>
      {ledgerAvailable && !signer && <ToggleRow title="Ledger signer" subtitle={prefs.ledger ? 'Your vault’s signer is a Ledger, over Bluetooth.' : 'Use a Ledger as your vault’s signer.'} value={prefs.ledger}
        question={prefs.ledger ? 'Vault requests go back to travelling as files, for a phone in hardware wallet mode.'
          : 'A proof of concept: test bitcoin only. It needs the Anzen app on a Ledger Flex or Stax, unlocked and open. A Ledger can’t protect this phone’s recovery backup yet, so keep its recovery phrase, and it can renew a vault of at most 16 deposits.'}
        onChange={on => onChange({ ...prefs, ledger: on, trezor: false })}/>}
      {trezorAvailable && !signer && <ToggleRow title="Trezor signer" subtitle={prefs.trezor ? 'Your vault’s signer is a Trezor, over Bluetooth.' : 'Use a Trezor as your vault’s signer.'} value={prefs.trezor}
        question={prefs.trezor ? 'Vault requests go back to travelling as files, for a phone in hardware wallet mode.'
          : 'A proof of concept: test bitcoin only. It needs a Trezor Safe 7 running Anzen’s firmware; this app loads the Anzen app onto it. A Trezor can’t protect this phone’s recovery backup yet, so keep its recovery phrase, and it can renew a vault of at most 64 deposits.'}
        onChange={on => onChange({ ...prefs, trezor: on, ledger: false })}/>}
      {onImport && <MenuRow title="Import a message or backup" subtitle="A file from your other device." onPress={onImport}/>}
    </View>
  </View>;
}
