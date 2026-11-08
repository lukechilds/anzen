import React, { useMemo, useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import Svg, { Path, Rect } from 'react-native-svg';
import { create } from 'qrcode';
import * as Clipboard from 'expo-clipboard';
import { Icon } from './DarkChrome';
import { colors, fonts, glass, styles as s } from './theme';

const QR_SIZE = 182;

// QR style: rounded dot modules, solid finder squares and a cleared centre for the
// badge. Quartile error correction keeps the code readable with the centre removed; the
// `bitcoin:` URI for a Taproot address fits version 6, which has no alignment pattern there.
function useQr(address: string) {
  return useMemo(() => {
    const { modules } = create(`bitcoin:${address}`, { errorCorrectionLevel: 'Q' });
    const n = modules.size;
    const finder = (x: number, y: number) => (x < 7 && y < 7) || (x >= n - 7 && y < 7) || (x < 7 && y >= n - 7);
    const hole = Math.round(n * .2) | 1, low = (n - hole) / 2, high = low + hole;
    const dots: { x: number; y: number }[] = [];
    for (let y = 0; y < n; y++) for (let x = 0; x < n; x++) {
      const inHole = x >= low && x < high && y >= low && y < high;
      if (modules.get(y, x) && !finder(x, y) && !inHole) dots.push({ x, y });
    }
    return { n, dots, finders: [[0, 0], [n - 7, 0], [0, n - 7]] };
  }, [address]);
}

// Highlights the parts people compare: the six-character bech32m checksum, and on a vault the
// vanity "vault" prefix after the separator. Two lines of equal length.
export function AddressLines({ address, vault, testID, size = 11.5 }: { address: string; vault: boolean; testID: string; size?: number }) {
  const program = address.lastIndexOf('1') + 2;
  const prefix = vault && address.slice(program, program + 5) === 'vault';
  const bold = (i: number) => i >= address.length - 6 || (prefix && i >= program && i < program + 5);
  const middle = Math.ceil(address.length / 2);
  const runs = (from: number, to: number) => {
    const out: { text: string; bold: boolean }[] = [];
    for (let i = from; i < to; i++) {
      const last = out.at(-1);
      if (last && last.bold === bold(i)) last.text += address[i]; else out.push({ text: address[i], bold: bold(i) });
    }
    return out;
  };
  return <View testID={testID} style={{ marginTop: 18, alignItems: 'center' }}>
    {[[0, middle], [middle, address.length]].map(([from, to]) => <Text key={from} selectable style={{ fontFamily: fonts.mono, fontSize: size, letterSpacing: size * .043, lineHeight: size * 1.74, color: colors.quiet, textAlign: 'center' }}>
      {runs(from, to).map((run, i) => <Text key={i} style={run.bold ? { color: colors.text, fontWeight: '500' } : undefined}>{run.text}</Text>)}
    </Text>)}
  </View>;
}

export default function AddressCard({ address, vault = false }: { address: string; vault?: boolean }) {
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState('');
  const qr = useQr(address);
  async function copy() {
    setError('');
    try {
      if (!await Clipboard.setStringAsync(address)) throw new Error('Clipboard access was not granted');
      setCopied(true);
    } catch { setError('Could not copy. Select the address below to copy it manually.'); }
  }
  return <View style={{ alignItems: 'center' }}>
    {/* The padding is the code's quiet zone: scanners need a dark margin around it. */}
    <View testID="receive-qr" style={{ marginTop: 18, padding: 12 }} accessibilityLabel="Bitcoin receiving address QR code">
      <Svg width={QR_SIZE} height={QR_SIZE} viewBox={`0 0 ${qr.n} ${qr.n}`} fill={colors.text}>
        {qr.dots.map(({ x, y }) => <Rect key={`${x}-${y}`} x={x + .07} y={y + .07} width={.86} height={.86} rx={.26}/>)}
        {qr.finders.map(([x, y]) => <React.Fragment key={`${x}-${y}`}>
          <Path fillRule="evenodd" d={`M${x} ${y}h7v7h-7z M${x + 1} ${y + 1}h5v5h-5z`}/>
          <Rect x={x + 2} y={y + 2} width={3} height={3} rx={.4}/>
        </React.Fragment>)}
      </Svg>
      <View pointerEvents="none" style={{ position: 'absolute', inset: 0, alignItems: 'center', justifyContent: 'center' }}>
        <Icon name={vault ? 'vaultFine' : 'wallet'} size={29} color={colors.text}/>
      </View>
    </View>
    <AddressLines address={address} vault={vault} testID={vault ? 'vault-address' : 'wallet-address'}/>
    <Pressable accessibilityRole="button" accessibilityLabel="Copy address" onPress={copy}
      style={({ pressed }) => ({ height: 40, marginTop: 16, paddingHorizontal: 22, borderRadius: 20, flexDirection: 'row', alignItems: 'center', gap: 7, backgroundColor: 'rgba(255,255,255,.08)', boxShadow: glass, transform: [{ scale: pressed ? .95 : 1 }] })}>
      <Icon name="copy" size={13} color="rgba(255,255,255,.64)"/>
      <Text style={{ color: colors.text, fontSize: 12.5, fontWeight: '500' }}>{copied ? 'Copied' : 'Copy address'}</Text>
    </Pressable>
    {!!error && <Text accessibilityRole="alert" style={s.errorText}>{error}</Text>}
  </View>;
}
