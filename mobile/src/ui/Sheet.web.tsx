// The browser emulator's sheet: drawn in the page, inside the emulated phone's frame. A device
// presents a native sheet instead (Sheet.tsx).
import React, { useEffect, useRef } from 'react';
import { Animated, Easing, Pressable, ScrollView, StyleSheet, View } from 'react-native';
import { BlurView } from 'expo-blur';
import { IconButton } from '../DarkSurfaces';
import { colors } from '../theme';

// In the page a sheet is just a layer over the screen it shares a frame with.
export function SheetHost({ children, sheets }: { children: React.ReactNode; sheets: React.ReactNode }) {
  return <>{children}{sheets}</>;
}

export function Sheet({ children, onClose, label }: { children: React.ReactNode; onClose: () => void; label: string }) {
  const dialog = useRef<View>(null), close = useRef(onClose);
  close.current = onClose;
  // Slides up and fades its scrim in.
  const shown = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    Animated.timing(shown, { toValue: 1, duration: 320, easing: Easing.bezier(.3, .9, .3, 1), useNativeDriver: false }).start();
  }, [shown]);
  // Keyboard users stay inside the sheet until it closes, and Escape closes it.
  useEffect(() => {
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
  return <View style={d.layer} accessibilityViewIsModal>
    <Animated.View style={[d.scrim, { opacity: shown }]}><Pressable accessible={false} focusable={false} onPress={onClose} style={StyleSheet.absoluteFill}/></Animated.View>
    <Animated.View ref={dialog} style={[d.sheet, { transform: [{ translateY: slide }] }]} role="dialog" accessibilityLabel={label}>
      <BlurView tint="dark" intensity={70} style={[StyleSheet.absoluteFill, { borderRadius: 38 }]}/>
      <View style={d.grab}/>
      {/* Kept for screen readers and pointer users; it does not push the sheet's content down. */}
      <View style={{ position: 'absolute', right: 14, top: 10, zIndex: 1 }}><IconButton label={`Close ${label}`} name="close" onPress={onClose}/></View>
      <ScrollView keyboardShouldPersistTaps="handled" showsVerticalScrollIndicator={false} contentContainerStyle={d.content}>{children}</ScrollView>
    </Animated.View>
  </View>;
}

const d = StyleSheet.create({
  layer: { position: 'absolute', inset: 0, zIndex: 50, justifyContent: 'flex-end' },
  scrim: { position: 'absolute', inset: 0, backgroundColor: 'rgba(0,0,0,.55)' },
  sheet: { margin: 10, maxHeight: '86%', borderRadius: 38, overflow: 'hidden', paddingTop: 12, backgroundColor: 'rgba(24,24,28,.72)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.14), 0 0 0 .5px rgba(255,255,255,.08), 0 -12px 44px rgba(0,0,0,.5)' },
  grab: { width: 36, height: 4, borderRadius: 2, backgroundColor: colors.faint, marginTop: 4, alignSelf: 'center' },
  content: { paddingHorizontal: 22, paddingBottom: 26 },
});
