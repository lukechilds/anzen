// A Trezor as the vault's signer, over Bluetooth. Rust owns the connection: Trezor's encrypted
// channel, the Anzen app it uploads to the Trezor, and the app's protocol messages turned into
// its requests. This only fits it to the link the signer flows speak to, and asks for the pairing
// code the first time. A proof of concept, like the Trezor app.
import { Alert } from 'react-native';
import { DECLINED } from './Tap';
import { ProtocolMessage, Tap } from './transport';

type Call = (request: string) => Promise<string>;

// The six digits a Trezor shows when a phone first pairs with it. Resolves null if the person
// backs out. A system prompt for now: it is asked once per Trezor.
function askForCode(name: string): Promise<string | null> {
  return new Promise(resolve => Alert.prompt(
    `Pair with ${name}`,
    'Enter the code your Trezor shows.',
    [
      { text: 'Cancel', style: 'cancel', onPress: () => resolve(null) },
      { text: 'Pair', onPress: (code?: string) => resolve((code ?? '').replace(/\D/g, '')) },
    ],
    'plain-text', '', 'number-pad',
  ));
}

export function trezorTap(call: Call): Tap {
  const ask = async (request: object) => {
    const response = JSON.parse(await call(JSON.stringify(request)));
    if (response.error) throw new Error(response.error);
    return response.ok;
  };
  return {
    hint: 'Unlock your Trezor and keep it close.',
    connect: async () => {
      const connected = await ask({ method: 'connect' });
      if (connected.pairing) {
        const code = await askForCode(connected.name ?? 'your Trezor');
        if (!code) {
          await ask({ method: 'disconnect' });
          throw new Error('Pairing was cancelled.');
        }
        await ask({ method: 'pair', code });
      }
      return {
        request: async message => {
          const answer = await ask({ method: 'request', message });
          if (answer.declined) throw new Error(DECLINED);
          return answer as ProtocolMessage;
        },
        // The link stays open for the next request; Rust reuses it or reconnects as needed.
        close: () => {},
      };
    },
    // A phone is never a Trezor's signer side.
    answer: () => () => {},
  };
}
