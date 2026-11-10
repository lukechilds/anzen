// A Ledger as the vault's signer, over Bluetooth. Rust owns the connection and translates the
// app's protocol messages into the Ledger app's commands; this only fits it to the link the
// signer flows speak to. A proof of concept, like the Ledger app.
import { DECLINED } from './Tap';
import { ProtocolMessage, Tap } from './transport';

type Call = (request: string) => Promise<string>;

export function ledgerTap(call: Call): Tap {
  const ask = async (request: object) => {
    const response = JSON.parse(await call(JSON.stringify(request)));
    if (response.error) throw new Error(response.error);
    return response.ok;
  };
  return {
    hint: 'Unlock your Ledger and open the Anzen app.',
    connect: async () => {
      await ask({ method: 'connect' });
      return {
        request: async message => {
          const answer = await ask({ method: 'request', message });
          if (answer.declined) throw new Error(DECLINED);
          return answer as ProtocolMessage;
        },
        // The link stays open for the next request: the Ledger's pairing screen, on a first
        // connection, needs it to outlast this one. Rust reuses it or reconnects as needed.
        close: () => {},
      };
    },
    // A phone is never a Ledger's signer side.
    answer: () => () => {},
  };
}
