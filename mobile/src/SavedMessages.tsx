import React, { useEffect, useRef, useState } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { Deliver, DeviceTransport, ProtocolMessage } from './transport';
import { colors, styles as s } from './theme';

type Item = { id: string; kind: ProtocolMessage['kind'] };

// Mount only in the private vault/signer context. Rust owns the records and validates
// their device binding; this component only lets the user select and share a copy.
export default function SavedMessages({ transport, deliver }: {
  transport: DeviceTransport;
  deliver: Deliver;
}) {
  const [open, setOpen] = useState(false);
  const [items, setItems] = useState<Item[]>([]);
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState('');
  const [error, setError] = useState('');
  const alive = useRef(true);
  useEffect(() => { alive.current = true; return () => { alive.current = false; }; }, []);

  async function show() {
    if (busy) return;
    setBusy(true); setError(''); setStatus('');
    try {
      const result = await transport.execute({ method: 'outbox' });
      if (alive.current) { setItems(result.messages); setOpen(true); }
    } catch (e) { if (alive.current) setError(String(e)); }
    finally { if (alive.current) setBusy(false); }
  }
  async function resend(id: string) {
    if (busy) return;
    setBusy(true); setError(''); setStatus('');
    try {
      const message = await transport.execute({ method: 'read_outbox', id });
      if (!alive.current) return;
      const result = await deliver(message as ProtocolMessage);
      if (alive.current) setStatus(result === 'offered'
        ? 'File offered for sharing. Check that the other device received it; the saved copy is unchanged.'
        : 'Copy shared. The other device must still open and validate it.');
    } catch (e) { if (alive.current) setError(`Sharing could not be confirmed: ${String(e)}. The saved copy is unchanged.`); }
    finally { if (alive.current) setBusy(false); }
  }
  const button = (title: string, onPress: () => void) => <Pressable accessibilityRole="button" accessibilityLabel={title}
    disabled={busy} onPress={onPress} style={[s.button, busy && { opacity: .4 }]}>
    <Text style={s.buttonText}>{title}</Text>
  </Pressable>;
  return <View testID="saved-messages">
    {/* A fallback for carrying messages as files, so it stays out of the way. */}
    {!open ? <Pressable accessibilityRole="button" accessibilityLabel="Saved messages" disabled={busy} onPress={show} style={({ pressed }) => ({ alignSelf: 'center', paddingVertical: 10, marginTop: 18, opacity: busy ? .4 : pressed ? .6 : 1 })}>
      <Text style={{ color: colors.faint, fontSize: 11.5 }}>Saved messages</Text>
    </Pressable> : <>
      <Text style={s.heading}>Saved protocol messages</Text>
      <Text style={s.subtext}>Copies of previously created proposals and approvals. Sharing one does not sign again or broadcast a transaction. An old message may no longer be usable.</Text>
      {items.length === 0 && <Text style={s.tiny}>No saved messages on this device.</Text>}
      {items.map(item => <View key={item.id} testID={`saved-message-${item.kind}`} style={s.notice}>
        <Text style={s.noticeText}>{item.kind.replaceAll('-', ' ')}</Text>
        <Text selectable style={s.mono}>{item.id}</Text>
        {button(`Resend ${item.kind.replaceAll('-', ' ')} ${item.id.slice(0, 8)}`, () => resend(item.id))}
      </View>)}
      {button('Refresh saved messages', show)}
      {button('Close saved messages', () => { setOpen(false); setItems([]); setStatus(''); setError(''); })}
    </>}
    {busy && <ActivityIndicator color={colors.mint} style={{ margin: 12 }} />}
    {!!status && <Text style={s.subtext}>{status}</Text>}
    {!!error && <Text accessibilityRole="alert" style={s.errorText}>{error}</Text>}
  </View>;
}
