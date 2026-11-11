// The browser emulator's tab bar: a glass pill floating over the screen, drawn in the page. A
// device shows the system's tab bar instead (Tabs.tsx).
import React from 'react';
import { Pressable, StyleSheet, View } from 'react-native';
import { BlurView } from 'expo-blur';
import { Icon } from '../DarkChrome';
import { colors, styles as s } from '../theme';
import { Text } from './Text';

export type TabKey = 'wallet' | 'vault';

export function TabBar({ tab, onChange, hidden, children }: {
  tab: TabKey; onChange: (tab: TabKey) => void; hidden: boolean; children: React.ReactNode;
}) {
  return <>
    {children}
    {!hidden && <View style={s.tabs}><BlurView pointerEvents="none" tint="dark" intensity={48} style={[StyleSheet.absoluteFill, { borderRadius: 30 }]}/>
      {(['wallet', 'vault'] as const).map(value => <Pressable key={value} accessibilityRole="tab" accessibilityLabel={value === 'wallet' ? 'Wallet' : 'Vault'} aria-selected={tab === value}
        onPress={() => onChange(value)} style={[s.tab, tab === value && s.selectedTab]}>
        <Icon name={value} size={16} color={tab === value ? colors.text : colors.faint}/><Text style={[s.tabText, tab === value && s.selectedText]}>{value === 'wallet' ? 'Wallet' : 'Vault'}</Text>
      </Pressable>)}
    </View>}
  </>;
}

// The pill floats 28 points above the bottom and is 60 tall.
export const TAB_BAR_CLEARANCE = 104;
