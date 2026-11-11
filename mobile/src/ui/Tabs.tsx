// The wallet and vault tabs, as iOS draws them: the system tab bar (a floating glass bar on
// recent iOS), with its own selection, accessibility and safe-area handling. The browser emulator
// draws its own (Tabs.web.tsx).
import React, { useState } from 'react';
import { NativeSyntheticEvent } from 'react-native';
import { Tabs, TabSelectedEvent } from 'react-native-screens';
import { colors } from '../theme';
import { haptics } from './haptics';

export type TabKey = 'wallet' | 'vault';
const tabs: { key: TabKey; title: string; icon: string; selected: string }[] = [
  { key: 'wallet', title: 'Wallet', icon: 'bitcoinsign.circle', selected: 'bitcoinsign.circle.fill' },
  { key: 'vault', title: 'Vault', icon: 'lock.shield', selected: 'lock.shield.fill' },
];

// `children` is the selected tab's screen; the other tab holds nothing while it is away, so the
// vault is never laid out behind the wallet. With `hidden` there is no bar, only the screen.
export function TabBar({ tab, onChange, hidden, children }: {
  tab: TabKey; onChange: (tab: TabKey) => void; hidden: boolean; children: React.ReactNode;
}) {
  // The native side owns which tab is selected. Each change it reports carries a number; a
  // request names the number it was based on, so a tap that raced it is not undone.
  const [provenance, setProvenance] = useState(0);
  return <Tabs.Host navStateRequest={{ selectedScreenKey: tab, baseProvenance: provenance }} tabBarHidden={hidden} colorScheme="dark"
    nativeContainerStyle={{ backgroundColor: colors.background }} ios={{ tabBarTintColor: colors.text, tabBarMinimizeBehavior: 'never' }}
    onTabSelected={({ nativeEvent }: NativeSyntheticEvent<TabSelectedEvent>) => {
      setProvenance(nativeEvent.provenance);
      const selected = nativeEvent.selectedScreenKey as TabKey;
      if (nativeEvent.actionOrigin === 'user') haptics.select();
      if (selected !== tab) onChange(selected);
    }}>
    {tabs.map(({ key, title, icon, selected }) => <Tabs.Screen key={key} screenKey={key} title={title} tabBarItemTestID={`tab-${key}`}
      style={{ backgroundColor: colors.background }}
      ios={{ icon: { type: 'sfSymbol', name: icon }, selectedIcon: { type: 'sfSymbol', name: selected }, overrideScrollViewContentInsetAdjustmentBehavior: false }}>
      {tab === key ? children : null}
    </Tabs.Screen>)}
  </Tabs.Host>;
}

// Room the system bar takes at the bottom of a screen, beyond the safe area: content scrolls
// under it, so the last of it needs this much clearance.
export const TAB_BAR_CLEARANCE = 64;
