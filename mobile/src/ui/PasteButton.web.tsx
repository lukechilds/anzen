// Paste in the browser emulator: the browser asks the person before a page may read the clipboard.
import React from 'react';
import { Pressable } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { colors } from '../theme';
import { Text } from './Text';

export function PasteButton({ onPaste }: { onPaste: (text: string) => void }) {
  return <Pressable accessibilityRole="button" accessibilityLabel="Paste"
    onPress={() => void Clipboard.getStringAsync().then(text => { if (text) onPaste(text.trim()); }).catch(() => {})}
    style={({ pressed }) => ({ height: 30, paddingHorizontal: 12, borderRadius: 15, justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.10)', opacity: pressed ? .6 : 1 })}>
    <Text style={{ color: colors.text, fontSize: 12, fontWeight: '500' }}>Paste</Text>
  </Pressable>;
}
