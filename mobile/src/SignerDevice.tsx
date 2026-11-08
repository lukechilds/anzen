import React, { useState } from 'react';
import { View } from 'react-native';
import Svg, { Defs, LinearGradient, Path, RadialGradient, Stop } from 'react-native-svg';

const WIDEST = 250;

// Measured from a real device, 15 grid units to the millimetre: a 44 mm top, 48 mm straight
// sides, 30 mm edges angling in to a 23 mm base (so about 76 mm tall), and a 60 mm tall screen
// inside a thin bezel. The bends round off; the mark sits on the chin below the screen.
export const GRID_W = 660, GRID_H = 1141;
export const BODY = 'M52 0H608Q660 0 660 52V660Q660 720 639 776.2L515.1 1107.3Q502.5 1141 466.5 1141H193.5Q157.5 1141 144.9 1107.3L21 776.2Q0 720 0 660V52Q0 0 52 0Z';
export const GLASS = 'M52 11H608Q649 11 649 52V670Q649 718 632.2 763L584.5 890.4Q576.8 911 554.8 911H105.2Q83.2 911 75.5 890.4L27.8 763Q11 718 11 670V52Q11 11 52 11Z';
// The display content: the full width of the glass, ending inside the tapered lower edge.
const CONTENT = { x: 11, y: 11, w: 638, h: 882 };
const MARK_Y = 1026;

function Mark({ size }: { size: number }) {
  return <Svg width={size * 1.06} height={size} viewBox="25 30 190 180" accessibilityElementsHidden>
    <Path d="M120 30 L215 210 L169 210 L120 117.2 L71 210 L25 210 Z" fill="rgba(255,255,255,.28)"/>
  </Svg>;
}

// The hardware signer. `screen` lays out the display at the given size. The same component can
// fill a phone screen when an old phone becomes a signer.
export default function SignerDevice({ screen: render }: { screen: (width: number, height: number) => React.ReactNode }) {
  const [room, setRoom] = useState(WIDEST);
  const w = Math.min(WIDEST, room), u = w / GRID_W, h = Math.round(GRID_H * u);
  const mark = Math.round(w * .085);
  return <View style={{ flex: 1, alignSelf: 'stretch', alignItems: 'center', justifyContent: 'center' }} onLayout={event => setRoom(Math.floor(event.nativeEvent.layout.width) - 16)}>
    <View style={{ width: w, height: h }}>
      <View pointerEvents="none" style={{ position: 'absolute', filter: 'drop-shadow(0 30px 34px rgba(0,0,0,.55)) drop-shadow(0 8px 10px rgba(0,0,0,.4))' } as object}>
        <Svg width={w} height={h} viewBox={`0 0 ${GRID_W} ${GRID_H}`}>
          <Defs>
            <LinearGradient id="metal" x1="0" y1="0" x2="1" y2="1"><Stop offset="0" stopColor="#24242a"/><Stop offset=".5" stopColor="#16161a"/><Stop offset="1" stopColor="#101013"/></LinearGradient>
            <RadialGradient id="glare" cx="22%" cy="6%" r="70%"><Stop offset="0" stopColor="#fff" stopOpacity={.07}/><Stop offset=".6" stopColor="#fff" stopOpacity={0}/></RadialGradient>
          </Defs>
          {/* Anodised body with a bright machined edge, then the glass. */}
          <Path d={BODY} fill="url(#metal)" stroke="rgba(255,255,255,.16)" strokeWidth={3}/>
          <Path d={GLASS} fill="#000" stroke="rgba(255,255,255,.05)" strokeWidth={2}/>
          <Path d={GLASS} fill="url(#glare)"/>
        </Svg>
      </View>
      <View style={{ position: 'absolute', left: CONTENT.x * u, top: CONTENT.y * u, width: CONTENT.w * u, height: CONTENT.h * u, borderTopLeftRadius: 41 * u, borderTopRightRadius: 41 * u, overflow: 'hidden' }}>
        {render(CONTENT.w * u, CONTENT.h * u)}
      </View>
      <View pointerEvents="none" style={{ position: 'absolute', left: 0, right: 0, top: MARK_Y * u - mark / 2, alignItems: 'center' }}><Mark size={mark}/></View>
    </View>
  </View>;
}
