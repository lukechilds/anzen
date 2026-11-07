import React, { useEffect, useRef } from 'react';
import { ActivityIndicator, Animated, Easing, Platform, Pressable, Text, View } from 'react-native';
import Svg, { Path, Rect } from 'react-native-svg';
import { DarkSheet } from './DarkSurfaces';
import { colors, glass, styles as s } from './theme';

const native = Platform.OS !== 'web';

// What the phone is doing with its signer, for the sheet's words. Rust does the work.
export type TapPurpose = 'setup' | 'approve' | 'finish' | 'recover' | 'replace';
export type TapPhase = 'waiting' | 'working' | 'review' | 'done' | 'failed';
export interface TapState { purpose: TapPurpose; phase: TapPhase; step?: string; error?: string }

const waiting: Record<TapPurpose, string> = {
  setup: 'Your vault is ready in a moment.',
  approve: 'Then approve on your signer.',
  finish: 'Your signer switches to the new vault.',
  recover: 'Your signer shares your vault’s public details.',
  replace: 'Your new signer replaces the lost one.',
};

// A phone with a signer coming towards it: the signer drifts in while waiting, rests against
// the phone once connected, and the waves between them turn mint.
export function TapArt({ phase, size = 1 }: { phase: TapPhase | 'idle'; size?: number }) {
  const approach = useRef(new Animated.Value(0)).current;
  const pulse = useRef(new Animated.Value(0)).current;
  const linked = phase === 'working' || phase === 'review' || phase === 'done';
  useEffect(() => {
    approach.stopAnimation();
    if (linked) {
      Animated.spring(approach, { toValue: 1, friction: 7, tension: 60, useNativeDriver: native }).start();
      return;
    }
    const loop = Animated.loop(Animated.sequence([
      Animated.timing(approach, { toValue: .55, duration: 1100, easing: Easing.inOut(Easing.cubic), useNativeDriver: native }),
      Animated.timing(approach, { toValue: 0, duration: 1100, easing: Easing.inOut(Easing.cubic), useNativeDriver: native }),
    ]));
    loop.start();
    return () => loop.stop();
  }, [linked, approach]);
  useEffect(() => {
    const loop = Animated.loop(Animated.timing(pulse, { toValue: 1, duration: 1600, easing: Easing.out(Easing.quad), useNativeDriver: native }));
    loop.start();
    return () => loop.stop();
  }, [pulse]);
  const tint = linked ? colors.mint : 'rgba(255,255,255,.5)';
  const signerX = approach.interpolate({ inputRange: [0, 1], outputRange: [34 * size, 0] });
  return <View pointerEvents="none" accessibilityElementsHidden importantForAccessibility="no-hide-descendants" style={{ width: 200 * size, height: 150 * size, alignSelf: 'center' }}>
    <Svg width={84 * size} height={140 * size} viewBox="0 0 84 140" style={{ position: 'absolute', left: 38 * size, top: 5 * size }}>
      <Rect x={2} y={2} width={80} height={136} rx={17} fill="rgba(255,255,255,.04)" stroke="rgba(255,255,255,.34)" strokeWidth={1.6}/>
      <Rect x={30} y={9} width={24} height={6} rx={3} fill="rgba(255,255,255,.22)"/>
    </Svg>
    {[0, 1, 2].map(i => <Animated.View key={i} style={{ position: 'absolute', left: 96 * size, top: 48 * size, opacity: pulse.interpolate({ inputRange: [0, 1], outputRange: i === 0 ? [.9, .25] : i === 1 ? [.5, .9] : [.2, .6] }) }}>
      <Svg width={40 * size} height={56 * size} viewBox="0 0 40 56">
        <Path d={`M${6 + i * 9} ${16 - i * 6}a${14 + i * 7} ${14 + i * 7} 0 0 1 0 ${24 + i * 12}`} stroke={tint} strokeWidth={1.8} strokeLinecap="round" fill="none"/>
      </Svg>
    </Animated.View>)}
    <Animated.View style={{ position: 'absolute', left: 104 * size, top: 34 * size, transform: [{ translateX: signerX }, { rotate: '-8deg' }] }}>
      <Svg width={56 * size} height={86 * size} viewBox="0 0 56 86">
        <Rect x={2} y={2} width={52} height={82} rx={13} fill="#121215" stroke={linked ? colors.mint : 'rgba(255,255,255,.7)'} strokeWidth={1.8}/>
        <Rect x={17} y={30} width={22} height={22} rx={11} fill="none" stroke={tint} strokeWidth={1.4}/>
        {phase === 'done' && <Path d="M22 41.5l4 4 8-9" stroke={colors.mint} strokeWidth={2} strokeLinecap="round" strokeLinejoin="round" fill="none"/>}
      </Svg>
    </Animated.View>
  </View>;
}

function Pill({ title, onPress, primary = false }: { title: string; onPress: () => void; primary?: boolean }) {
  return <Pressable accessibilityRole="button" accessibilityLabel={title} onPress={onPress}
    style={({ pressed }) => ({ height: 44, paddingHorizontal: 26, borderRadius: 22, alignItems: 'center', justifyContent: 'center', backgroundColor: primary ? 'rgba(255,255,255,.14)' : 'rgba(255,255,255,.07)', boxShadow: glass, transform: [{ scale: pressed ? .96 : 1 }] })}>
    <Text style={{ color: colors.text, fontSize: 13, fontWeight: '500' }}>{title}</Text>
  </Pressable>;
}

// The signer's user said no. Shown as an outcome, not as a failure to retry.
export const DECLINED = 'Declined on your signer.';

export function TapSheet({ state, onCancel, onRetry, onDone }: { state: TapState; onCancel: () => void; onRetry: () => void; onDone: () => void }) {
  const { purpose, phase } = state;
  const declined = phase === 'failed' && state.error === DECLINED;
  const title = declined ? 'Declined on your signer' : phase === 'failed' ? 'Couldn’t finish'
    : phase === 'done' ? (purpose === 'setup' ? 'Your vault is ready' : 'Done')
    : phase === 'review' ? 'Approve on your signer'
    : phase === 'working' ? 'Connected'
    : purpose === 'replace' ? 'Hold your new signer to your phone' : 'Hold your signer to your phone';
  const detail = declined ? 'Nothing was signed.' : phase === 'failed' ? state.error
    : phase === 'done' ? (purpose === 'setup' ? 'Two keys now protect your savings: this phone and your signer. Your recovery backup is saved.' : '')
    : phase === 'review' ? 'Check the details on your signer’s screen.'
    : phase === 'working' ? state.step
    : waiting[purpose];
  const close = phase === 'done' ? onDone : onCancel;
  return <DarkSheet label="Connect signer" onClose={close}>
    <View testID="tap-sheet" style={{ alignItems: 'center', paddingTop: 30 }}>
      <TapArt phase={phase}/>
      <Text accessibilityRole="header" style={{ color: colors.text, fontSize: 19, fontWeight: '500', letterSpacing: -.3, marginTop: 26, textAlign: 'center' }}>{title}</Text>
      {!!detail && <Text accessibilityRole={phase === 'failed' && !declined ? 'alert' : undefined} style={[phase === 'failed' && !declined ? s.errorText : s.subtext, { textAlign: 'center', marginTop: 8, marginBottom: 0, paddingHorizontal: 8 }]}>{detail}</Text>}
      {(phase === 'working' || phase === 'review') && <ActivityIndicator color={colors.mint} style={{ marginTop: 18 }}/>}
      <View style={{ flexDirection: 'row', gap: 10, marginTop: 26 }}>
        {phase === 'done' ? <Pill title="Done" primary onPress={onDone}/>
          : declined ? <Pill title="Close" primary onPress={onCancel}/>
          : phase === 'failed' ? <><Pill title="Cancel" onPress={onCancel}/><Pill title="Try again" primary onPress={onRetry}/></>
          : <Pill title="Cancel" onPress={onCancel}/>}
      </View>
    </View>
  </DarkSheet>;
}
