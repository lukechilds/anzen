// Settings rows that ask before they act, with the system's own controls: a switch, an alert,
// an action sheet. The browser emulator opens the question under the row instead (Rows.web.tsx).
import React from 'react';
import { ActionSheetIOS, Alert, Platform, Switch, View } from 'react-native';
import { MenuRow, d } from '../DarkSurfaces';
import { colors } from '../theme';
import { Text } from './Text';

export interface Option<Key extends string> { key: Key; label: string; note: string }

// On or off, confirmed in an alert that says what changing it does. The switch only moves once
// the person agrees.
export function ToggleRow({ title, subtitle, value, question, onChange, disabled = false }: {
  title: string; subtitle: string; value: boolean;
  // What turning it on (or, when it is on, off) does.
  question: string; onChange: (value: boolean) => void; disabled?: boolean;
}) {
  const ask = () => Alert.alert(title, question, [
    { text: 'Cancel', style: 'cancel' },
    { text: value ? 'Turn off' : 'Turn on', onPress: () => onChange(!value) },
  ]);
  return <View style={[d.menuRow, { opacity: disabled ? .4 : 1 }]}>
    <View style={{ flex: 1 }}><Text style={d.title}>{title}</Text><Text style={d.subtitle}>{subtitle}</Text></View>
    <Switch accessibilityLabel={title} value={value} onValueChange={ask} disabled={disabled}
      trackColor={{ true: 'rgba(167,227,187,.55)', false: 'rgba(255,255,255,.14)' }} ios_backgroundColor="rgba(255,255,255,.14)"/>
  </View>;
}

// One of a few, chosen from the system's action sheet. `children` holds anything a choice needs
// before it applies (a node's address, say), shown under the row.
export function SelectRow<Key extends string>({ title, subtitle, options, selected, onSelect, children }: {
  title: string; subtitle: string; options: Option<Key>[]; selected: Key; onSelect: (key: Key) => void;
  // Web only: which choice the open list highlights, and whether it is open.
  picked?: Key; open?: boolean; onOpen?: (open: boolean) => void;
  children?: React.ReactNode;
}) {
  const choose = () => {
    const labels = options.map(option => option.label);
    if (Platform.OS === 'ios') {
      ActionSheetIOS.showActionSheetWithOptions({ title, options: [...labels, 'Cancel'], cancelButtonIndex: labels.length, userInterfaceStyle: 'dark' },
        index => { if (index < options.length) onSelect(options[index].key); });
    } else {
      Alert.alert(title, undefined, [...options.map(option => ({ text: option.label, onPress: () => onSelect(option.key) })), { text: 'Cancel', style: 'cancel' as const }]);
    }
  };
  return <>
    <MenuRow title={title} subtitle={subtitle} value={options.find(option => option.key === selected)?.label} onPress={choose}/>
    {children}
  </>;
}

// Something to confirm, asked in an alert: the action in words, marked destructive where it is.
export function ConfirmRow({ title, subtitle, warning, note, action, danger = false, onConfirm, disabled = false }: {
  title: string; subtitle: string; warning: string; note?: string; action: string; danger?: boolean; onConfirm: () => void; disabled?: boolean;
}) {
  return <MenuRow title={title} subtitle={subtitle} disabled={disabled} onPress={() => Alert.alert(`${title}?`, note ? `${warning}\n\n${note}` : warning, [
    { text: 'Cancel', style: 'cancel' },
    { text: action, style: danger ? 'destructive' : 'default', onPress: onConfirm },
  ])}/>;
}

// Ask before something that cannot be taken back. Resolves true if the person goes ahead.
export function confirmDanger(title: string, message: string, action: string): Promise<boolean> {
  return new Promise(resolve => Alert.alert(title, message, [
    { text: 'Cancel', style: 'cancel', onPress: () => resolve(false) },
    { text: action, style: 'destructive', onPress: () => resolve(true) },
  ], { cancelable: true, onDismiss: () => resolve(false) }));
}
// The system asks in an alert, so nothing is asked in the page.
export const asksInline = false;
