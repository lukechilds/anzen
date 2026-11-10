import React, { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { MenuRow } from './DarkSurfaces';
import { JsonObject } from './transport';
import { colors, styles as s } from './theme';

// Public descriptors, for watching this wallet or the vault in another wallet such as Sparrow or
// Bitcoin Core. They can't spend, but they reveal every address and transaction.
export default function Descriptors({ kind, busy, run }: {
  kind: 'wallet' | 'vault';
  busy: boolean;
  run: (command: JsonObject, after?: (value: JsonObject) => void) => Promise<unknown>;
}) {
  const [shown, setShown] = useState<{ label: string; text: string }[] | null>(null);
  const toggle = () => shown ? setShown(null) : void run({ method: 'descriptors' }, value => setShown(kind === 'wallet'
    ? [{ label: 'Wallet', text: value.wallet }]
    : [{ label: 'Vault', text: value.vault.vault }, { label: 'Connectors', text: value.vault.connectors }]));
  const title = kind === 'wallet' ? 'Export wallet descriptor' : 'Export vault descriptors';
  return <View testID={`${kind}-descriptors`}>
    <MenuRow title={shown ? (kind === 'wallet' ? 'Hide wallet descriptor' : 'Hide vault descriptors') : title}
      subtitle={kind === 'wallet' ? 'Watch this wallet in Sparrow or Bitcoin Core' : 'Watch the vault in Bitcoin Core'} onPress={toggle} disabled={busy}/>
    {shown && <View style={{ paddingHorizontal: 8, paddingBottom: 12 }}>
      <Text style={[s.tiny, { color: colors.quiet, marginTop: 4 }]}>
        {kind === 'wallet' ? 'Receive and change addresses in one descriptor. It can’t spend, but anyone who has it can see this wallet’s balance and every transaction.'
          : 'Spending still needs your phone and signer, but anyone who has these can see the vault’s balance and every transaction. They change when you replace your phone.'}
      </Text>
      {shown.map(({ label, text }) => <Descriptor key={label} label={label} text={text} multiple={shown.length > 1}/>)}
    </View>}
  </View>;
}

function Descriptor({ label, text, multiple }: { label: string; text: string; multiple: boolean }) {
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState('');
  const copy = async () => {
    setError('');
    try {
      if (!await Clipboard.setStringAsync(text)) throw new Error();
      setCopied(true);
    } catch { setError('Could not copy. Select the descriptor to copy it manually.'); }
  };
  return <View style={{ marginTop: 12 }}>
    {multiple && <Text style={s.label}>{label}</Text>}
    <Text selectable testID={`descriptor-${label.toLowerCase()}`} style={[s.mono, { color: colors.text }]}>{text}</Text>
    <Pressable accessibilityRole="button" accessibilityLabel={`Copy ${label.toLowerCase()} descriptor`} onPress={() => void copy()}
      style={({ pressed }) => ({ alignSelf: 'flex-start', marginTop: 8, paddingVertical: 6, opacity: pressed ? .6 : 1 })}>
      <Text style={{ color: colors.mint, fontSize: 12.5, fontWeight: '500' }}>{copied ? 'Copied' : 'Copy'}</Text>
    </Pressable>
    {!!error && <Text accessibilityRole="alert" style={s.errorText}>{error}</Text>}
  </View>;
}
