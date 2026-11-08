import React, { useEffect, useRef } from 'react';
import { Animated, Easing, Platform, Pressable, ScrollView, StyleSheet, Text, View } from 'react-native';
import { BlurView } from 'expo-blur';
import { Icon, IconName } from './DarkChrome';
import { colors, fonts, formatBtc } from './theme';

const c = { text: colors.text, secondary: colors.secondary, dim: colors.muted, muted: colors.quiet, faint: colors.faint };

export function IconButton({ label, name, onPress, disabled = false }: { label: string; name: IconName; onPress: () => void; disabled?: boolean }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={label} onPress={onPress} disabled={disabled}
    style={({ pressed }) => [d.iconButton, { opacity: disabled ? .35 : pressed ? .6 : 1 }]}><Icon name={name} size={16}/></Pressable>;
}

export function ActionPill({ title, label = title, icon, onPress, disabled = false }: { title: string; label?: string; icon: IconName; onPress: () => void; disabled?: boolean }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={label} onPress={onPress} disabled={disabled}
    style={({ pressed }) => [d.pill, { opacity: disabled ? .4 : 1, transform: [{ scale: pressed ? .96 : 1 }] }]}>
    <BlurView pointerEvents="none" tint="dark" intensity={30} style={[StyleSheet.absoluteFill, { borderRadius: 26 }]}/>
    {/* Wrapped so the icon stacks above the blur layer on the web, like the label. */}
    <View><Icon name={icon} size={14} strokeWidth={2.2} color="rgba(255,255,255,.64)"/></View><Text style={d.pillText}>{title}</Text>
  </Pressable>;
}

// A settings row: label, optional explanation, a value on the right and a faint chevron.
export function MenuRow({ title, subtitle, value, icon, onPress, disabled = false }: { title: string; subtitle?: string; value?: string; icon?: IconName; onPress: () => void; disabled?: boolean }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={title} onPress={onPress} disabled={disabled}
    style={({ pressed }) => [d.menuRow, { opacity: disabled ? .4 : pressed ? .6 : 1 }]}>
    {icon && <View style={d.avatar}><Icon name={icon} size={16}/></View>}
    <View style={{ flex: 1 }}><Text style={d.title}>{title}</Text>{subtitle && <Text style={d.subtitle}>{subtitle}</Text>}</View>
    {value && <Text style={d.rowValue}>{value}</Text>}<Icon name="chevron" size={12} color={c.faint}/>
  </Pressable>;
}

// A withdrawal route: what it is, how much it can move, and who must approve it.
export function OptionCard({ title, value, detail, icon, onPress }: { title: string; value: string; detail: string; icon: IconName; onPress: () => void }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={title} onPress={onPress}
    style={({ pressed }) => [d.option, { transform: [{ scale: pressed ? .97 : 1 }] }]}>
    <View style={d.optionIcon}><Icon name={icon} size={16}/></View>
    <View style={{ flex: 1 }}>
      <Text style={d.optionTitle}>{title}</Text><Text style={d.optionValue}>{value}</Text><Text style={[d.subtitle, { marginTop: 2 }]}>{detail}</Text>
    </View>
  </Pressable>;
}

export function DarkSheet({ children, onClose, label }: { children: React.ReactNode; onClose: () => void; label: string }) {
  const dialog = useRef<View>(null), close = useRef(onClose);
  close.current = onClose;
  // Slides up and fades its scrim in.
  const shown = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    Animated.timing(shown, { toValue: 1, duration: 320, easing: Easing.bezier(.3, .9, .3, 1), useNativeDriver: Platform.OS !== 'web' }).start();
  }, [shown]);
  useEffect(() => {
    if (Platform.OS !== 'web') return;
    const element = dialog.current as unknown as HTMLElement;
    const previous = document.activeElement as HTMLElement | null;
    const controls = () => Array.from(element.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled]), textarea:not([disabled]), [tabindex="0"]')).filter(item => item.getClientRects().length);
    controls()[0]?.focus();
    const keydown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); close.current(); }
      if (event.key !== 'Tab') return;
      const items = controls(), first = items[0], last = items.at(-1);
      if (event.shiftKey && document.activeElement === first) { event.preventDefault(); last?.focus(); }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus(); }
    };
    element.addEventListener('keydown', keydown);
    return () => { element.removeEventListener('keydown', keydown); if (element.contains(document.activeElement) || document.activeElement === document.body) previous?.focus(); };
  }, []);
  const slide = shown.interpolate({ inputRange: [0, 1], outputRange: [700, 0] });
  return <View style={d.sheetLayer} accessibilityViewIsModal>
    <Animated.View style={[d.scrim, { opacity: shown }]}><Pressable accessible={false} focusable={false} onPress={onClose} style={StyleSheet.absoluteFill}/></Animated.View>
    <Animated.View ref={dialog} style={[d.sheet, { transform: [{ translateY: slide }] }]} role="dialog" accessibilityLabel={label} onAccessibilityEscape={onClose}>
      <BlurView tint="dark" intensity={70} style={[StyleSheet.absoluteFill, { borderRadius: 38 }]}/>
      <View style={d.grab}/>
      {/* Kept for screen readers and pointer users; it does not push the sheet's content down. */}
      <View style={{ position: 'absolute', right: 14, top: 10, zIndex: 1 }}><IconButton label={`Close ${label}`} name="close" onPress={onClose}/></View>
      <ScrollView keyboardShouldPersistTaps="handled" showsVerticalScrollIndicator={false} contentContainerStyle={d.sheetContent}>{children}</ScrollView>
    </Animated.View>
  </View>;
}

export interface Activity {
  txid: string; kind: 'received' | 'sent' | 'self_transfer'; balance_change_sats: number;
  confirmed: boolean; timestamp: number | null; fee_sats: number | null;
}
export function WalletHistory({ items, loaded, onSelect }: { items: Activity[]; loaded: boolean; onSelect: (entry: Activity) => void }) {
  let previousMonth = '';
  return <View style={d.list}>
    {items.length === 0 && loaded && <Text style={[d.subtitle, { textAlign: 'center', marginTop: 40 }]}>No payments yet</Text>}
    {items.map(entry => {
      const date = entry.timestamp === null ? null : new Date(entry.timestamp * 1000);
      const year = date && date.getFullYear() !== new Date().getFullYear() ? { year: 'numeric' as const } : {};
      const month = !entry.confirmed ? 'Pending' : date?.toLocaleDateString('en-US', { month: 'long', ...year }) || 'Confirmed';
      const showMonth = previousMonth !== month; previousMonth = month;
      return <React.Fragment key={entry.txid}>
        {showMonth && <Text style={d.month}>{month}</Text>}
        <Pressable accessibilityRole="button" accessibilityLabel={`Transaction ${entry.txid}`} onPress={() => onSelect(entry)} style={[d.transaction, !entry.confirmed && d.pending]}>
          <View style={[d.avatar, !entry.confirmed && { opacity: .8 }]}><Icon name={entry.kind === 'received' ? 'receive' : entry.kind === 'sent' ? 'send' : 'refresh'} size={16}/></View>
          <View style={{ flex: 1, minWidth: 0 }}><Text style={[d.title, !entry.confirmed && { color: c.dim }]}>{entry.kind === 'received' ? 'Bitcoin received' : entry.kind === 'sent' ? 'Bitcoin sent' : 'Between your addresses'}</Text>
            <Text style={d.subtitle}>{date?.toLocaleDateString('en-US', { month: 'short', day: 'numeric' }) || 'Date unavailable'} · {entry.txid.slice(0, 6)}…</Text></View>
          <Text style={[d.value, !entry.confirmed ? { color: c.muted } : entry.balance_change_sats > 0 && { color: colors.mint }]}>{entry.balance_change_sats < 0 ? '−' : '+'}₿{formatBtc(Math.abs(entry.balance_change_sats))}</Text>
        </Pressable>
      </React.Fragment>;
    })}
  </View>;
}

export const d = StyleSheet.create({
  iconButton: { width: 34, height: 34, borderRadius: 17, alignItems: 'center', justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.06)', boxShadow: '0 0 0 .5px rgba(255,255,255,.14), 0 4px 12px rgba(0,0,0,.4)' },
  pill: { flex: 1, height: 52, borderRadius: 26, flexDirection: 'row', gap: 8, alignItems: 'center', justifyContent: 'center', overflow: 'hidden', backgroundColor: 'rgba(255,255,255,.08)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.16), inset 0 -1px 0 rgba(255,255,255,.03), 0 0 0 .5px rgba(255,255,255,.07), 0 12px 26px rgba(0,0,0,.45)' },
  pillText: { color: c.text, fontSize: 14, fontWeight: '500' },
  menuRow: { minHeight: 52, paddingVertical: 15, flexDirection: 'row', gap: 12, alignItems: 'center' },
  rowValue: { color: c.muted, fontSize: 12, marginRight: 2 },
  option: { flexDirection: 'row', alignItems: 'center', gap: 14, paddingVertical: 14, paddingHorizontal: 16, borderRadius: 24, backgroundColor: 'rgba(255,255,255,.07)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.12), 0 0 0 .5px rgba(255,255,255,.05)' },
  optionIcon: { width: 38, height: 38, borderRadius: 19, alignItems: 'center', justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.07)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.10)' },
  optionTitle: { color: c.text, fontSize: 13.5, fontWeight: '500' },
  optionValue: { color: c.dim, fontSize: 12, marginTop: 3, fontFamily: fonts.number, fontVariant: ['tabular-nums'] },
  title: { color: c.secondary, fontSize: 13.5, fontWeight: '400' },
  subtitle: { color: c.muted, fontSize: 11.5, marginTop: 3 },
  avatar: { width: 40, height: 40, borderRadius: 20, backgroundColor: 'rgba(255,255,255,.06)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.10), 0 0 0 .5px rgba(255,255,255,.05)', alignItems: 'center', justifyContent: 'center' },
  list: { marginTop: 22, marginHorizontal: -6, paddingTop: 12 },
  month: { color: c.muted, fontSize: 11.5, fontWeight: '500', paddingTop: 20, paddingBottom: 4 },
  transaction: { flexDirection: 'row', alignItems: 'center', gap: 13, paddingVertical: 10 },
  // The dashed card bleeds into the margin so its avatar lines up with the rows around it.
  pending: { borderWidth: 1, borderStyle: 'dashed', borderColor: 'rgba(255,255,255,.18)', borderRadius: 14, paddingVertical: 9, paddingHorizontal: 14, marginHorizontal: -14, marginBottom: 6 },
  value: { color: c.secondary, fontFamily: fonts.number, fontSize: 13, fontWeight: '500', fontVariant: ['tabular-nums'] },
  empty: { paddingVertical: 45, alignItems: 'center' },
  sheetLayer: { position: 'absolute', inset: 0, zIndex: 50, justifyContent: 'flex-end' },
  scrim: { position: 'absolute', inset: 0, backgroundColor: 'rgba(0,0,0,.55)' },
  sheet: { margin: 10, maxHeight: '86%', borderRadius: 38, overflow: 'hidden', paddingTop: 12, backgroundColor: 'rgba(24,24,28,.72)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.14), 0 0 0 .5px rgba(255,255,255,.08), 0 -12px 44px rgba(0,0,0,.5)' },
  grab: { width: 36, height: 4, borderRadius: 2, backgroundColor: c.faint, marginTop: 4, alignSelf: 'center' },
  sheetContent: { paddingHorizontal: 22, paddingBottom: 26 },
});
