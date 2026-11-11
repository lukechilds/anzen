// The browser emulator's settings rows: the question opens under the row, in the page. A device
// asks with the system's switch, alert and action sheet instead (Rows.tsx).
import React, { useState } from 'react';
import { Pressable, View } from 'react-native';
import { Button } from '../AnzenApp';
import { MenuRow } from '../DarkSurfaces';
import { colors, styles as s } from '../theme';
import { Text } from './Text';

export interface Option<Key extends string> { key: Key; label: string; note: string }

export function ToggleRow({ title, subtitle, value, question, onChange, disabled = false }: {
  title: string; subtitle: string; value: boolean; question: string; onChange: (value: boolean) => void; disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  return <>
    <MenuRow title={title} subtitle={subtitle} value={value ? 'On' : 'Off'} disabled={disabled} onPress={() => setOpen(!open)}/>
    {open && <View style={{ paddingHorizontal: 8, paddingBottom: 8 }}>
      <Text style={[s.tiny, { color: colors.quiet, marginTop: 4 }]}>{question}</Text>
      <Button title={value ? 'Turn off' : 'Turn on'} primary onPress={() => { setOpen(false); onChange(!value); }}/>
    </View>}
  </>;
}

function Choice({ title, note, selected, onPress }: { title: string; note: string; selected: boolean; onPress: () => void }) {
  return <Pressable accessibilityRole="radio" accessibilityLabel={title} aria-checked={selected} onPress={onPress}
    style={({ pressed }) => ({ flexDirection: 'row', alignItems: 'center', paddingVertical: 12, paddingHorizontal: 4, opacity: pressed ? .6 : 1 })}>
    <View style={{ flex: 1 }}>
      <Text style={{ color: colors.text, fontSize: 14, fontWeight: '500' }}>{title}</Text>
      <Text style={[s.tiny, { color: colors.quiet, marginTop: 2 }]}>{note}</Text>
    </View>
    {selected && <Text style={{ color: colors.mint, fontSize: 14 }}>✓</Text>}
  </Pressable>;
}

export function SelectRow<Key extends string>({ title, subtitle, options, selected, onSelect, picked = selected, open = false, onOpen, children }: {
  title: string; subtitle: string; options: Option<Key>[]; selected: Key; onSelect: (key: Key) => void;
  picked?: Key; open?: boolean; onOpen?: (open: boolean) => void; children?: React.ReactNode;
}) {
  return <>
    <MenuRow title={title} subtitle={subtitle} value={options.find(option => option.key === selected)?.label} onPress={() => onOpen?.(!open)}/>
    {open && <View style={{ paddingHorizontal: 8, paddingBottom: 8 }}>
      {options.map(option => <Choice key={option.key} title={option.label} note={option.note} selected={picked === option.key} onPress={() => onSelect(option.key)}/>)}
      {children}
    </View>}
  </>;
}

export function ConfirmRow({ title, subtitle, warning, note, action, danger = false, onConfirm, disabled = false }: {
  title: string; subtitle: string; warning: string; note?: string; action: string; danger?: boolean; onConfirm: () => void; disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  return <>
    <MenuRow title={title} subtitle={subtitle} onPress={() => setOpen(!open)} disabled={disabled}/>
    {open && <View style={{ paddingHorizontal: 8, paddingBottom: 8 }}>
      <Text style={[danger ? s.errorText : s.tiny, !danger && { color: colors.quiet }, { marginTop: 4 }]}>{warning}</Text>
      {!!note && <Text style={[s.tiny, { color: colors.quiet, marginTop: 8 }]}>{note}</Text>}
      <Button title={action} danger={danger} primary={!danger} disabled={disabled} onPress={() => { setOpen(false); onConfirm(); }}/>
    </View>}
  </>;
}

// In the page the warning and its button sit under the row; nothing pops up.
export function confirmDanger(_title: string, _message: string, _action: string): Promise<boolean> {
  return Promise.resolve(true);
}
export const asksInline = true;
