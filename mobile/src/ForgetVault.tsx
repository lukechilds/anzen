import React, { useState } from 'react';
import { Text, View } from 'react-native';
import { Button } from './AnzenApp';
import { MenuRow } from './DarkSurfaces';
import { JsonObject } from './transport';
import { colors, styles as s } from './theme';

// Forget this phone's vault, keeping its wallet, so a vault can be set up again with the same
// signer or another. Test networks only (Rust refuses on mainnet), and nothing is deleted: the
// vault's records are set aside on the device.
export default function ForgetVault({ busy, run, onForgotten }: {
  busy: boolean;
  run: (command: JsonObject, after?: (value: JsonObject) => void) => Promise<unknown>;
  onForgotten: (identity: JsonObject) => void;
}) {
  const [open, setOpen] = useState(false);
  return <View testID="forget-vault" style={{ marginTop: 34 }}>
    <Text style={s.sheetEyebrow}>Vault</Text>
    <View style={{ marginTop: 12 }}>
      <MenuRow title="Forget vault" subtitle="Set a vault up again, with this signer or another." onPress={() => setOpen(!open)} disabled={busy}/>
      {open && <View style={{ paddingHorizontal: 8, paddingBottom: 8 }}>
        <Text style={[s.errorText, { marginTop: 4 }]}>Move any bitcoin out of the vault first. This phone will no longer know the vault, its policy or its recovery backup.</Text>
        <Text style={[s.tiny, { color: colors.quiet, marginTop: 8 }]}>Your wallet and its recovery phrase stay. For test networks only.</Text>
        <Button title="Forget vault" danger disabled={busy} onPress={() => void run({ method: 'forget_vault' }, identity => { setOpen(false); onForgotten(identity); })}/>
      </View>}
    </View>
  </View>;
}
