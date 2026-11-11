import React, { useEffect, useRef, useState } from 'react';
import { Animated, Platform, StyleSheet, View, ViewStyle } from 'react-native';
import { Text } from './ui/Text';
import Svg, { Circle, Defs, FeTurbulence, Filter, LinearGradient, Path, Pattern, RadialGradient, Rect, Stop } from 'react-native-svg';
import { colors } from './theme';

export type IconName = 'wallet' | 'vault' | 'vaultFine' | 'send' | 'receive' | 'refresh' | 'close' | 'settings' | 'chevron' | 'clock' | 'key' | 'phone' | 'copy' | 'scan' | 'face' | 'cloud' | 'lock';
// The app's glyphs. Stroke widths follow it: 2 for most icons, 2.2 for the send and
// receive arrows on pills, finer strokes inside the wallet and vault marks.
export function Icon({ name, size = 18, color = 'rgba(255,255,255,.68)', strokeWidth = 2 }: { name: IconName; size?: number; color?: string; strokeWidth?: number }) {
  return <Svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke={color} strokeWidth={strokeWidth} strokeLinecap="round" strokeLinejoin="round" accessibilityElementsHidden>
    {name === 'wallet' && <><Circle cx="9.3" cy="9.3" r="6.6" strokeWidth={2}/><Path d="M20.68 11.91A6.6 6.6 0 0 1 11.91 20.68" strokeWidth={2}/><Path d="M7.8 6.4v5.8M7.8 6.4h1.7a1.45 1.45 0 0 1 0 2.9H7.8M7.8 9.3h2a1.45 1.45 0 0 1 0 2.9h-2M8.9 5.3v1.1M8.9 12.2v1.1" strokeWidth={1.5}/></>}
    {name === 'vault' && <><Circle cx="12" cy="12" r="10.1" strokeWidth={1.5}/><Circle cx="12" cy="12" r="3.9" strokeWidth={1.3}/><Circle cx="12" cy="12" r="2.5" fill={color}/>{Array.from({ length: 11 }, (_, i) => <Path key={i} d="M12 4.2V5.4" rotation={(i + 1) * 30} origin="12,12" strokeWidth={1.1}/>)}<Path d="M11.15 4.2h1.7L12 6.2z" fill={color} strokeWidth={.6}/></>}
    {/* The finer dial shown in the centre of a vault deposit QR code. */}
    {name === 'vaultFine' && <><Circle cx="12" cy="12" r="10.25" strokeWidth={1.1}/>{Array.from({ length: 19 }, (_, i) => <Path key={i} d="M12 4.25V5.05" rotation={(i + 1) * 18} origin="12,12" strokeWidth={.5}/>)}<Path d="M11.35 4.25h1.3L12 5.7z" fill={color} strokeWidth={.5}/><Circle cx="12" cy="12" r="3.85" strokeWidth={1}/><Circle cx="12" cy="12" r="2.9" fill={color} stroke="none"/></>}
    {name === 'send' && <Path d="M12 19V6m-5.5 5.5L12 6l5.5 5.5"/>}
    {name === 'receive' && <Path d="M12 5v13m-5.5-5.5L12 18l5.5-5.5"/>}
    {name === 'refresh' && <Path d="M18.5 12a6.5 6.5 0 1 1-2-4.7M18.5 4.5V8H15"/>}
    {name === 'close' && <Path d="m6 6 12 12M18 6 6 18"/>}
    {name === 'chevron' && <Path d="m9 5 7 7-7 7"/>}
    {name === 'clock' && <><Circle cx="12" cy="12" r="8"/><Path d="M12 8v4l2.5 2"/></>}
    {name === 'key' && <Path d="M21 2l-2 2m-7.61 7.61a5.5 5.5 0 1 1-7.778 7.778 5.5 5.5 0 0 1 7.777-7.777zm0 0L15.5 7.5m0 0l3 3L22 7l-3-3m-3.5 3.5L19 4"/>}
    {name === 'phone' && <><Rect x="5" y="2" width="14" height="20" rx="3"/><Path d="M12 18h.01"/></>}
    {name === 'copy' && <><Rect x="9" y="9" width="11" height="11" rx="2.5"/><Path d="M5 15V6.5A2.5 2.5 0 0 1 7.5 4H15"/></>}
    {name === 'face' && <><Path d="M4 8V6.5A2.5 2.5 0 0 1 6.5 4H8M16 4h1.5A2.5 2.5 0 0 1 20 6.5V8M20 16v1.5a2.5 2.5 0 0 1-2.5 2.5H16M8 20H6.5A2.5 2.5 0 0 1 4 17.5V16"/><Path d="M9 9.5v1M15 9.5v1M12 9.5v3.5h-1M9.5 15.5a3.5 3.5 0 0 0 5 0"/></>}
    {name === 'lock' && <><Rect x="5" y="11" width="14" height="10" rx="2.5"/><Path d="M8 11V8a4 4 0 0 1 8 0v3"/></>}
    {name === 'cloud' && <Path d="M7.5 18.5h9.5a4 4 0 0 0 .6-7.95A5.5 5.5 0 0 0 7 9.6a4.5 4.5 0 0 0 .5 8.9z"/>}
    {name === 'scan' && <Path d="M4 8V6a2 2 0 0 1 2-2h2M16 4h2a2 2 0 0 1 2 2v2M20 16v2a2 2 0 0 1-2 2h-2M8 20H6a2 2 0 0 1-2-2v-2"/>}
    {name === 'settings' && <><Circle cx="12" cy="12" r="3"/><Path d="m9 3-.5 2-2 1-2-.5-2 3 1.5 1.5v3L2.5 15l2 3 2-.5 2 1 .5 2h4l.5-2 2-1 2 .5 2-3-1.5-1.5v-3L21 9l-2-3-2 .5-2-1-.5-2z"/></>}
  </Svg>;
}

export function DarkBackground() {
  // Clip the oversized glow here: overflowing decoration would otherwise widen the screen's
  // scrollable area, and focusing a sheet's edge control would scroll the whole screen sideways.
  return <View pointerEvents="none" style={[StyleSheet.absoluteFill, { overflow: 'hidden' }]}>
    <Svg width={460} height={320} viewBox="0 0 460 320" style={{ position: 'absolute', top: -90, left: '50%', marginLeft: -230 }}>
      <Defs><RadialGradient id="glow"><Stop offset="0" stopColor="white" stopOpacity={.045}/><Stop offset=".72" stopColor="white" stopOpacity={0}/></RadialGradient></Defs>
      <Rect width="460" height="320" fill="url(#glow)"/>
    </Svg>
    <View style={{ position: 'absolute', top: 70, left: '50%', marginLeft: -95, opacity: .05 }}>
      <Svg width={190} height={190} viewBox="25 30 190 180"><Path d="M120 30 L215 210 L169 210 L120 117.2 L71 210 L25 210 Z" fill="white"/></Svg>
    </View>
  </View>;
}

// On a device, what scrolls up the screen fades out under the status bar and the buttons at the top,
// instead of running into them. Only once something has scrolled: at rest the screen is untouched.
// (The browser blurs the same strip, in DarkEffects.)
export function TopFade({ height, solid, shown }: { height: number; solid: number; shown: boolean }) {
  const opacity = useRef(new Animated.Value(0)).current;
  useEffect(() => {
    Animated.timing(opacity, { toValue: shown ? 1 : 0, duration: 160, useNativeDriver: true }).start();
  }, [shown]);
  return <Animated.View pointerEvents="none" style={{ position: 'absolute', top: 0, left: 0, right: 0, height, zIndex: 6, opacity }}>
    <Svg width="100%" height={height} accessibilityElementsHidden>
      <Defs><LinearGradient id="top-fade" x1="0" y1="0" x2="0" y2="1">
        <Stop offset="0" stopColor={colors.background} stopOpacity={1}/>
        <Stop offset={solid / height} stopColor={colors.background} stopOpacity={.94}/>
        <Stop offset="1" stopColor={colors.background} stopOpacity={0}/>
      </LinearGradient></Defs>
      <Rect width="100%" height={height} fill="url(#top-fade)"/>
    </Svg>
  </Animated.View>;
}

// The reference's browser-only finishing layers. Native keeps its OS status/safe area;
// feTurbulence and CSS mask/backdrop filters are not supported by the native SVG renderer.
export function DarkEffects() {
  if (Platform.OS !== 'web') return null;
  return <>
    <View pointerEvents="none" style={{ position: 'absolute', top: 0, left: 0, right: 0, height: 118, zIndex: 6 }}>
      {[[2, 58, 90], [6, 35, 62], [12, 22, 48]].map(([blur, start, end]) => {
        const mask = `linear-gradient(to bottom,#000 ${start}%,transparent ${end}%)`;
        const effect: ViewStyle & React.CSSProperties = { position: 'absolute', top: 0, left: 0, right: 0, bottom: 0,
          backdropFilter: `blur(${blur}px)`, WebkitBackdropFilter: `blur(${blur}px)`, maskImage: mask, WebkitMaskImage: mask };
        return <View key={blur} style={effect}/>;
      })}
    </View>
    <View pointerEvents="none" style={[StyleSheet.absoluteFill, { zIndex: 60, opacity: .04 }]}>
      <Svg width="100%" height="100%" accessibilityElementsHidden>
        <Defs>
          <Filter id="dark-noise"><FeTurbulence type="fractalNoise" baseFrequency={.85} numOctaves={2}/></Filter>
          <Pattern id="dark-grain" width={140} height={140} patternUnits="userSpaceOnUse">
            <Rect width={140} height={140} filter="url(#dark-noise)" opacity={.55}/>
          </Pattern>
        </Defs>
        <Rect width="100%" height="100%" fill="url(#dark-grain)"/>
      </Svg>
    </View>
  </>;
}

// Device chrome exists only in the browser playground. Native uses the actual OS status bar.
// The status bar's clock, as an iPhone shows it: hours and minutes, no AM/PM.
function useClock() {
  const read = () => new Date().toLocaleTimeString([], { hour: 'numeric', minute: '2-digit' }).replace(/\s?[AP]M$/i, '');
  const [time, setTime] = useState(read);
  useEffect(() => {
    let timer: ReturnType<typeof setTimeout>;
    const tick = () => { setTime(read()); timer = setTimeout(tick, 60_000 - Date.now() % 60_000); };
    timer = setTimeout(tick, 60_000 - Date.now() % 60_000);
    return () => clearTimeout(timer);
  }, []);
  return time;
}

export function DeviceStatus() {
  const [width, setWidth] = useState(355);
  const time = useClock();
  if (Platform.OS !== 'web') return null;
  const island = Math.min(122, width - 148);
  return <View pointerEvents="none" onLayout={event => setWidth(event.nativeEvent.layout.width)} style={{ height: 44, zIndex: 20, paddingTop: 24, paddingHorizontal: width < 300 ? 20 : 32, flexDirection: 'row', justifyContent: 'space-between' }}>
    <Text style={{ color: 'rgba(255,255,255,.92)', fontSize: width < 300 ? 13 : 15, fontWeight: '600', lineHeight: 18 }}>{time}</Text>
    <View style={{ position: 'absolute', top: 16, left: '50%', marginLeft: -island / 2, width: island, height: 34, borderRadius: 17, backgroundColor: '#000', borderWidth: .5, borderColor: '#ffffff0f' }}/>
    <Svg width={45} height={16} viewBox="0 0 45 16" fill="rgba(255,255,255,.92)">
      {[4, 6, 8, 10].map((height, i) => <Rect key={i} x={i * 4} y={12 - height} width={2.5} height={height} rx={1}/>)}
      {/* A 23×11.5 battery outline with a 72% fill and a dimmer terminal. */}
      <Rect x={20} y={1.75} width={21.5} height={10} rx={3.25} fill="none" stroke="rgba(255,255,255,.55)" strokeWidth={1.5}/><Rect x={22.25} y={4} width={12.25} height={5.5} rx={1.5}/><Rect x={43.25} y={4.75} width={1.5} height={4} rx={.7} fill="rgba(255,255,255,.55)"/>
    </Svg>
  </View>;
}
