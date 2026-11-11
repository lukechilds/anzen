import React from 'react';
import { View } from 'react-native';
import { Text } from './ui/Text';
import { ConfirmRow } from './ui/Rows';
import { JsonObject } from './transport';
import { styles as s } from './theme';

// Forget this phone's vault, keeping its wallet, so a vault can be set up again with the same
// signer or another. Test networks only (Rust refuses on mainnet), and nothing is deleted: the
// vault's records are set aside on the device.
export default function ForgetVault({ busy, run, onForgotten }: {
  busy: boolean;
  run: (command: JsonObject, after?: (value: JsonObject) => void) => Promise<unknown>;
  onForgotten: (identity: JsonObject) => void;
}) {
  return <View testID="forget-vault" style={{ marginTop: 34 }}>
    <Text style={s.sheetEyebrow}>Vault</Text>
    <View style={{ marginTop: 12 }}>
      <ConfirmRow title="Forget vault" subtitle="Set a vault up again, with this signer or another." disabled={busy} danger action="Forget vault"
        warning="Move any bitcoin out of the vault first. This phone will no longer know the vault, its policy or its recovery backup."
        note="Your wallet and its recovery phrase stay. For test networks only."
        onConfirm={() => void run({ method: 'forget_vault' }, onForgotten)}/>
    </View>
  </View>;
}
