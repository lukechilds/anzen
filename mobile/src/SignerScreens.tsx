import React, { useRef, useState } from 'react';
import { Animated, Easing, Platform, Pressable, ScrollView, Text, View } from 'react-native';
import Svg, { Path } from 'react-native-svg';
import { colors, fonts } from './theme';

const native = Platform.OS !== 'web';
const HOLD_MS = 1200;

// One fact per screen, as on a hardware wallet: a small label, the thing itself, and a note.
export function Page({ eyebrow, title, value, testID, children }: {
  eyebrow?: string; title?: string; value?: string; testID?: string; children?: React.ReactNode;
}) {
  return <View style={{ alignItems: 'center' }}>
    {!!eyebrow && <Text style={{ color: colors.faint, fontSize: 9, letterSpacing: 2.5, textTransform: 'uppercase', textAlign: 'center', marginBottom: 10 }}>{eyebrow}</Text>}
    {!!title && <Text style={{ color: colors.text, fontSize: 18, lineHeight: 23, fontWeight: '500', letterSpacing: -.3, textAlign: 'center' }}>{title}</Text>}
    {!!value && <Text testID={testID} style={{ color: colors.text, fontSize: 25, lineHeight: 31, fontWeight: '500', fontFamily: fonts.number, fontVariant: ['tabular-nums'], textAlign: 'center', marginTop: title ? 10 : 0 }}>{value}</Text>}
    {children}
  </View>;
}
export function Note({ children, warning = false }: { children: React.ReactNode; warning?: boolean }) {
  return <Text style={{ color: warning ? colors.red : colors.quiet, fontSize: 11, lineHeight: 16, textAlign: 'center', marginTop: 8 }}>{children}</Text>;
}
// Addresses, keys and fingerprints: centred, wrapping anywhere, to compare with the phone.
export function Code({ children }: { children: string }) {
  return <Text selectable style={{ color: colors.secondary, fontFamily: fonts.mono, fontSize: 10, lineHeight: 15, letterSpacing: .4, textAlign: 'center', marginTop: 8, ...(Platform.OS === 'web' ? { wordBreak: 'break-all' } as object : {}) }}>{children}</Text>;
}

function Glyph({ name, color = colors.text }: { name: 'close' | 'next' | 'check'; color?: string }) {
  return <Svg width={16} height={16} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={2.2} strokeLinecap="round" strokeLinejoin="round">
    {name === 'close' && <Path d="m6.5 6.5 11 11M17.5 6.5l-11 11"/>}
    {name === 'next' && <Path d="M5 12h13m-5-5.5 5.5 5.5-5.5 5.5"/>}
    {name === 'check' && <Path d="m5 12.5 4.5 4.5L19 7.5"/>}
  </Svg>;
}

function RoundButton({ label, glyph, onPress, disabled, tint }: { label: string; glyph: 'close' | 'next' | 'check'; onPress: () => void; disabled: boolean; tint?: string }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={label} disabled={disabled} onPress={onPress}
    style={({ pressed }) => ({ width: 42, height: 42, borderRadius: 21, alignItems: 'center', justifyContent: 'center', opacity: disabled ? .35 : 1,
      backgroundColor: tint ? 'rgba(167,227,187,.14)' : 'rgba(255,255,255,.08)', transform: [{ scale: pressed ? .92 : 1 }],
      boxShadow: tint ? `0 0 0 1px ${tint}` : 'inset 0 1px 0 rgba(255,255,255,.12), 0 0 0 .5px rgba(255,255,255,.06)' })}>
    <Glyph name={glyph} color={tint ?? colors.text}/>
  </Pressable>;
}

// Signing needs a deliberate press and hold, as on a hardware wallet: letting go early cancels.
function HoldButton({ label, onConfirm, disabled }: { label: string; onConfirm: () => void; disabled: boolean }) {
  const fill = useRef(new Animated.Value(0)).current;
  const done = useRef(false);
  const start = () => {
    done.current = false;
    Animated.timing(fill, { toValue: 1, duration: HOLD_MS, easing: Easing.linear, useNativeDriver: native }).start(({ finished }) => {
      if (finished && !done.current) { done.current = true; onConfirm(); fill.setValue(0); }
    });
  };
  const stop = () => { if (!done.current) { fill.stopAnimation(); Animated.timing(fill, { toValue: 0, duration: 180, useNativeDriver: native }).start(); } };
  return <Pressable accessibilityRole="button" accessibilityLabel={label} accessibilityHint="Press and hold to approve" disabled={disabled}
    accessibilityActions={[{ name: 'activate' }]} onAccessibilityAction={() => { if (!disabled) onConfirm(); }}
    onPressIn={start} onPressOut={stop} {...({ dataSet: { hold: 'true' } } as object)}
    style={{ width: 42, height: 42, borderRadius: 21, alignItems: 'center', justifyContent: 'center', overflow: 'hidden', opacity: disabled ? .35 : 1,
      backgroundColor: 'rgba(167,227,187,.12)', boxShadow: `0 0 0 1px ${colors.mint}` }}>
    <Animated.View style={{ position: 'absolute', width: 42, height: 42, borderRadius: 21, backgroundColor: colors.mint, transform: [{ scale: fill }] }}/>
    <Glyph name="check" color={colors.text}/>
  </Pressable>;
}

export interface Review {
  pages: React.ReactNode[];
  reject: { label: string; onPress: () => void };
  confirm: { label: string; onPress: () => void; hold?: boolean };
}

// A review, page by page: ✕ declines from any page, → moves on, and the last page asks for the
// approval. The dots jump between pages.
export function Pager({ review, busy, inset }: { review: Review; busy: boolean; inset: number }) {
  const [index, setIndex] = useState(0);
  const { pages, reject, confirm } = review;
  const last = index >= pages.length - 1;
  return <View style={{ flex: 1 }}>
    <ScrollView style={{ flex: 1 }} showsVerticalScrollIndicator={false} contentContainerStyle={{ flexGrow: 1, justifyContent: 'center', paddingHorizontal: inset, paddingTop: inset, paddingBottom: 8 }}>
      {pages[Math.min(index, pages.length - 1)]}
    </ScrollView>
    <Text style={{ color: last ? colors.mint : colors.faint, fontSize: 9.5, letterSpacing: .3, textAlign: 'center', marginBottom: 8 }}>
      {last ? (confirm.hold ? 'Hold ✓ to approve' : 'Tap ✓ to continue') : `${index + 1} of ${pages.length}`}
    </Text>
    <View style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 16, paddingBottom: inset }}>
      <RoundButton label={reject.label} glyph="close" onPress={reject.onPress} disabled={busy}/>
      <View style={{ flexDirection: 'row', gap: 5 }}>
        {pages.map((_, i) => <Pressable key={i} accessibilityRole="button" accessibilityLabel={`Page ${i + 1} of ${pages.length}`} onPress={() => setIndex(i)} hitSlop={6}
          style={{ width: i === index ? 12 : 5, height: 5, borderRadius: 3, backgroundColor: i === index ? colors.text : 'rgba(255,255,255,.25)' }}/>)}
      </View>
      {!last ? <RoundButton label="Next" glyph="next" onPress={() => setIndex(index + 1)} disabled={busy}/>
        : confirm.hold ? <HoldButton label={confirm.label} onConfirm={confirm.onPress} disabled={busy}/>
        : <RoundButton label={confirm.label} glyph="check" tint={colors.mint} onPress={confirm.onPress} disabled={busy}/>}
    </View>
  </View>;
}
