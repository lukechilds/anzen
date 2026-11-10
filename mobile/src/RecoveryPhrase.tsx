import React, { useState } from 'react';
import { Text, TextInput, View } from 'react-native';
import { Button } from './AnzenApp';
import { MenuRow } from './DarkSurfaces';
import { JsonObject } from './transport';
import { colors, styles as s } from './theme';

// This device's recovery phrase: shown on request, and imported in place of the device's own keys
// only before vault setup (Rust refuses later, and keeps the replaced keys on the device).
export default function RecoveryPhrase({ signer, network, hasKeys, canImport, busy, run, secure, onImported }: {
  signer: boolean;
  network: string;
  // Whether there are keys to show yet. A signer only makes its key when it first pairs.
  hasKeys: boolean;
  canImport: boolean;
  busy: boolean;
  run: (command: JsonObject, after?: (value: JsonObject) => void) => Promise<unknown>;
  secure: (action: () => unknown) => Promise<void>;
  onImported: (identity: JsonObject) => void;
}) {
  const [words, setWords] = useState<string[] | null>(null);
  const [importing, setImporting] = useState(false);
  const [typed, setTyped] = useState('');
  const keys = signer ? 'signer’s key' : 'wallet';
  const show = () => words ? setWords(null)
    : void secure(() => run({ method: 'recovery_phrase' }, value => setWords(String(value.words).split(' '))));
  const importPhrase = () => void run({ method: 'import_recovery_phrase', network, words: typed }, identity => {
    setTyped(''); setImporting(false); setWords(null); onImported(identity);
  });
  return <View testID="recovery-phrase" style={{ marginTop: 34 }}>
    <Text style={s.sheetEyebrow}>Recovery phrase</Text>
    <View style={{ marginTop: 12 }}>
      {hasKeys && <MenuRow title={words ? 'Hide recovery phrase' : 'Show recovery phrase'} subtitle={`The words that restore this ${keys}.`} onPress={show} disabled={busy}/>}
      {words && <View style={{ paddingHorizontal: 8, paddingBottom: 12 }}>
        <Text style={[s.errorText, { marginTop: 4 }]}>Anyone with these words can take your bitcoin. Write them down and keep them offline. Never share them or type them into a website.</Text>
        <View testID="recovery-phrase-words" style={{ flexDirection: 'row', flexWrap: 'wrap', marginTop: 12 }}>
          {words.map((word, index) => <View key={index} style={{ width: '50%', flexDirection: 'row', paddingVertical: 5 }}>
            <Text style={{ color: colors.faint, fontSize: 12, width: 24, textAlign: 'right', marginRight: 10, fontVariant: ['tabular-nums'] }}>{index + 1}</Text>
            <Text selectable={false} style={{ color: colors.text, fontSize: 14 }}>{word}</Text>
          </View>)}
        </View>
      </View>}
      {canImport && <MenuRow title="Import recovery phrase" subtitle="Only before vault setup." onPress={() => setImporting(!importing)} disabled={busy}/>}
      {canImport && importing && <View style={{ paddingHorizontal: 8, paddingBottom: 8 }}>
        <Text style={[s.tiny, { color: colors.quiet, marginTop: 4 }]}>
          {signer ? 'This signer’s key is replaced by the one these words restore. The current key stays on the device, set aside.'
            : 'This wallet is replaced by the one these words restore. The current wallet’s keys stay on the device, set aside. A wallet that has had transactions can’t be replaced.'}
        </Text>
        <Text style={s.label}>Recovery phrase words</Text>
        <TextInput accessibilityLabel="Recovery phrase words" style={[s.input, { minHeight: 96, textAlignVertical: 'top' }]} value={typed} onChangeText={setTyped}
          multiline autoCapitalize="none" autoCorrect={false} autoComplete="off" spellCheck={false} placeholder="12 or 24 words, separated by spaces" placeholderTextColor={colors.faint}/>
        <Button title="Import" primary disabled={busy || !typed.trim()} onPress={importPhrase}/>
      </View>}
    </View>
  </View>;
}
