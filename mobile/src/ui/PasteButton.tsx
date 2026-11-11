// Paste from the clipboard, with the system's own paste button: iOS lets it read the clipboard
// without asking the person each time, where reading it from code raises a permission prompt.
import React from 'react';
import { Pressable } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { colors } from '../theme';
import { Text } from './Text';

export function PasteButton({ onPaste }: { onPaste: (text: string) => void }) {
  if (Clipboard.isPasteButtonAvailable) {
    return <Clipboard.ClipboardPasteButton accessibilityLabel="Paste" acceptedContentTypes={['plain-text']} displayMode="labelOnly" cornerStyle="capsule"
      backgroundColor="#34343a" foregroundColor="#ffffff" style={{ height: 34, width: 72 }}
      onPress={data => { if (data.type === 'text') onPaste(data.text.trim()); }}/>;
  }
  // Older systems: read the clipboard directly, which they confirm with the person.
  return <Pressable accessibilityRole="button" accessibilityLabel="Paste" hitSlop={8}
    onPress={() => void Clipboard.getStringAsync().then(text => { if (text) onPaste(text.trim()); }).catch(() => {})}
    style={({ pressed }) => ({ height: 32, paddingHorizontal: 13, borderRadius: 16, justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.12)', opacity: pressed ? .6 : 1 })}>
    <Text style={{ color: colors.text, fontSize: 12.5, fontWeight: '500' }}>Paste</Text>
  </Pressable>;
}
