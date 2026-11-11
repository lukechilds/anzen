import React, { useEffect, useRef, useState } from 'react';
import { Animated, Easing, Platform, StyleSheet, View } from 'react-native';
import { Text } from './ui/Text';
import { BlurView } from 'expo-blur';
import Svg, { Path } from 'react-native-svg';
import { colors } from './theme';

const native = Platform.OS !== 'web';
const SCAN_MS = 650, SUCCESS_MS = 450, FADE_MS = 200;

// The Face ID glyph: corner brackets around eyes, nose and a smile.
function Face({ color }: { color: string }) {
  return <Svg width={64} height={64} viewBox="0 0 64 64" fill="none" stroke={color} strokeWidth={3.4} strokeLinecap="round" strokeLinejoin="round">
    <Path d="M6 20V13a7 7 0 0 1 7-7h7M44 6h7a7 7 0 0 1 7 7v7M58 44v7a7 7 0 0 1-7 7h-7M20 58h-7a7 7 0 0 1-7-7v-7"/>
    <Path d="M22 24v5M42 24v5M32 24v12h-3M24 42a11 11 0 0 0 16 0"/>
  </Svg>;
}
function Tick({ color }: { color: string }) {
  return <Svg width={64} height={64} viewBox="0 0 64 64" fill="none" stroke={color} strokeWidth={3.6} strokeLinecap="round" strokeLinejoin="round">
    <Path d="M32 6a26 26 0 1 1 0 52 26 26 0 0 1 0-52z"/><Path d="m21 33 8 8 15-17"/>
  </Svg>;
}

// A simulated Face ID check, as iOS shows it: the glyph scans, becomes a tick, then fades.
// `cover` hides the screen behind it, for unlocking the app itself.
function FaceIdPrompt({ cover, onDone }: { cover: boolean; onDone: () => void }) {
  const shown = useRef(new Animated.Value(0)).current, scan = useRef(new Animated.Value(0)).current, success = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    const timing = (value: Animated.Value, toValue: number, duration: number, easing = Easing.inOut(Easing.quad)) =>
      Animated.timing(value, { toValue, duration, easing, useNativeDriver: native });
    Animated.sequence([
      timing(shown, 1, 160),
      Animated.loop(Animated.sequence([timing(scan, 1, SCAN_MS / 4), timing(scan, 0, SCAN_MS / 4)]), { iterations: 2 }),
      timing(success, 1, SUCCESS_MS * .5, Easing.out(Easing.back(2))),
      Animated.delay(SUCCESS_MS * .5),
      timing(shown, 0, FADE_MS),
    ]).start(() => onDone());
  }, []);
  const scale = scan.interpolate({ inputRange: [0, 1], outputRange: [1, 1.08] });
  return <Animated.View accessibilityLabel="Face ID" style={[StyleSheet.absoluteFill, { zIndex: 90, alignItems: 'center', justifyContent: 'center', opacity: cover ? 1 : shown }]}>
    {cover && <View style={[StyleSheet.absoluteFill, { backgroundColor: colors.background }]}/>}
    <Animated.View style={{ width: 150, height: 150, borderRadius: 34, overflow: 'hidden', alignItems: 'center', justifyContent: 'center', opacity: shown, backgroundColor: 'rgba(40,40,46,.72)' }}>
      <BlurView tint="dark" intensity={60} style={[StyleSheet.absoluteFill, { borderRadius: 34 }]}/>
      <Animated.View style={{ position: 'absolute', top: 34, opacity: success.interpolate({ inputRange: [0, 1], outputRange: [1, 0] }), transform: [{ scale }] }}><Face color={colors.text}/></Animated.View>
      <Animated.View style={{ position: 'absolute', top: 34, opacity: success, transform: [{ scale: success.interpolate({ inputRange: [0, 1], outputRange: [.6, 1] }) }] }}><Tick color={colors.text}/></Animated.View>
      <Text style={{ position: 'absolute', bottom: 18, color: colors.secondary, fontSize: 12.5, fontWeight: '500' }}>Face ID</Text>
    </Animated.View>
  </Animated.View>;
}

// Face ID for the emulator: `authenticate` plays the check and resolves once it has passed;
// render `prompt` over the screen. A device build would ask the operating system instead.
export function useSimulatedFaceId() {
  const [pending, setPending] = useState<{ cover: boolean; resolve: (passed: boolean) => void } | null>(null);
  const authenticate = (cover = false) => new Promise<boolean>(resolve => setPending({ cover, resolve }));
  const prompt = pending && <FaceIdPrompt cover={pending.cover} onDone={() => { pending.resolve(true); setPending(null); }}/>;
  return { authenticate, prompt };
}
