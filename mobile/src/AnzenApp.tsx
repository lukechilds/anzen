import React, { useEffect, useRef, useState } from 'react';
import SavedMessages from './SavedMessages';
import { parseEncryptedBackup } from './fileExchange';
import { ActivityIndicator, AppState, Platform, Pressable, ScrollView, StyleSheet, Text, TextInput, View } from 'react-native';
import { BlurView } from 'expo-blur';
import { DarkBackground, DarkEffects, DeviceStatus, Icon } from './DarkChrome';
import { ActionPill, Activity, DarkSheet, IconButton, MenuRow, WalletHistory } from './DarkSurfaces';
import AddressCard, { AddressLines } from './AddressCard';
import VaultSummary from './VaultSummary';
import { VaultSettings, WithdrawalOptions } from './VaultControls';
import { PolicyEditor } from './VaultControls';
import { DECLINED, TapArt, TapPurpose, TapSheet, TapState } from './Tap';
import { colors, fonts, formatBtc, styles as s } from './theme';
import { Deliver, DeviceTransport, JsonObject, ProtocolMessage, SignerLink, Tap, TransferResult, signerBound } from './transport';

type Props = {
  network?: 'regtest' | 'bitcoin' | 'signet';
  transport: DeviceTransport;
  incoming: ProtocolMessage | null;
  dismissIncoming: () => void;
  deliver: Deliver;
  // Holding the phone and signer together. Without it, messages travel as files.
  tap?: Tap;
  saveBackup?: (backup: JsonObject) => Promise<TransferResult>;
  cloudBackup?: JsonObject | null;
};
type Kind = ProtocolMessage['kind'];
type ReviewKind = 'policy' | 'sweep' | 'backup' | 'offline-action' | 'restore' | 'rotation' | 'rotation-adoption' | 'signer-replacement' | 'signer-adoption' | 'share-vault';
// These finish only after the phone's transaction confirms, so the phone offers them later.
const deferred: Kind[] = ['phone-rotation-completed', 'signer-replacement-completed'];
const purposeOf = (kind: Kind): TapPurpose => kind === 'signer-hello' ? 'setup'
  : kind === 'recovery-vault-request' ? 'recover' : deferred.includes(kind) ? 'finish' : 'approve';
const message = (error: unknown) => error instanceof Error ? error.message : String(error);
const btc = (sats: number = 0) => formatBtc(sats);

function Balance({ sats }: { sats?: number }) {
  const [width, setWidth] = useState(0);
  const text = sats === undefined ? '₿—' : `₿${btc(sats)}`;
  const digits = text.slice(1);
  // React Native Web does not implement native Text's adjustsFontSizeToFit. Size tabular
  // digits conservatively on both platforms so a long amount is never clipped/ellipsized.
  const fontSize = width ? Math.min(46, width / (text.length * .66)) : 46;
  return <View style={{ width: '100%', alignItems: 'center' }} onLayout={event => setWidth(event.nativeEvent.layout.width)}>
    {/* A little space after the ₿, as in the mockup; the text content stays "₿0.1000". */}
    <Text testID="bitcoin-balance" style={[s.balance, { fontSize, textAlign: 'center' }]}><Text style={{ marginRight: 3 }}>₿</Text>{digits}</Text>
  </View>;
}

// Replaces a refresh button: appears only when live updates need the user. A failed background
// refresh shows why with Retry; a regtest wallet left far behind by time travel offers Catch up.
function SyncStatus({ error, behind, busy, onSync }: { error?: string; behind?: number | null; busy: boolean; onSync: () => void }) {
  if (!error && !behind) return null;
  const action = error ? 'Retry' : 'Catch up';
  return <View testID="sync-status" style={{ flexDirection: 'row', alignItems: 'center', justifyContent: 'center', gap: 10, marginTop: 10, paddingHorizontal: 12 }}>
    <Text style={[s.tiny, { flexShrink: 1, textAlign: 'center' }]} numberOfLines={3}>
      {error ? `Couldn’t update · ${error}` : `${behind!.toLocaleString('en-US')} blocks behind`}
    </Text>
    <Pressable accessibilityRole="button" accessibilityLabel={action} disabled={busy} onPress={onSync} style={({ pressed }) => ({ opacity: busy ? .4 : pressed ? .7 : 1 })}>
      <Text style={[s.tiny, { color: colors.mint, fontWeight: '600' }]}>{action}</Text>
    </Pressable>
  </View>;
}

export function Button({ title, onPress, primary = false, danger = false, disabled = false }: {
  title: string; onPress: () => void; primary?: boolean; danger?: boolean; disabled?: boolean;
}) {
  return <Pressable accessibilityRole="button" accessibilityLabel={title} disabled={disabled} onPress={onPress}
    style={({ pressed }) => [s.button, primary && s.primary, danger && s.danger, { opacity: disabled ? .4 : pressed ? .7 : 1 }]}>
    <Text style={[s.buttonText, primary && s.primaryText, danger && s.dangerText]}>{title}</Text>
  </Pressable>;
}
function Field({ label, value, onChange, numeric = false }: { label: string; value: string; onChange: (s: string) => void; numeric?: boolean }) {
  return <><Text style={s.label}>{label}</Text><TextInput accessibilityLabel={label} value={value} onChangeText={onChange}
    style={s.input} keyboardType={numeric ? 'number-pad' : 'default'} autoCapitalize="none" autoCorrect={false} /></>;
}

export default function AnzenApp({ transport, incoming, dismissIncoming, deliver, tap, saveBackup, cloudBackup, network = 'regtest' }: Props) {
  const signer = transport.role === 'signer';
  const [identity, setIdentity] = useState<JsonObject | null>(null);
  const [sessionLoaded, setSessionLoaded] = useState(false);
  const [tab, setTab] = useState<'wallet' | 'vault'>('wallet');
  const [page, setPage] = useState('home');
  const [vaultVisible, setVaultVisible] = useState(false);
  const [wallet, setWallet] = useState<JsonObject | null>(null);
  const [selectedActivity, setSelectedActivity] = useState<Activity | null>(null);
  const [vault, setVault] = useState<JsonObject | null>(null);
  const [pairing, setPairing] = useState<JsonObject | null>(null);
  const [paired, setPaired] = useState(false);
  const [backupStored, setBackupStored] = useState(false);
  const [tapState, setTapState] = useState<TapState | null>(null);
  const [linked, setLinked] = useState(false);
  // A signer's approval the phone could not finish with yet, e.g. before a timelock matures.
  const [retained, setRetained] = useState<ProtocolMessage | null>(null);
  const [review, setReview] = useState<JsonObject | null>(null);
  const [reviewKind, setReviewKind] = useState<ReviewKind>('policy');
  const [signerCandidate, setSignerCandidate] = useState<JsonObject | null>(null);
  const [contact, setContact] = useState<JsonObject | null>(null);
  const [contactName, setContactName] = useState('');
  const [receivedContact, setReceivedContact] = useState<JsonObject | null>(null);
  const [contactRestoreReview, setContactRestoreReview] = useState<JsonObject | null>(null);
  const [contactRecovery, setContactRecovery] = useState<JsonObject | null>(null);
  const [contactStatus, setContactStatus] = useState<JsonObject | null>(null);
  const [contactSweep, setContactSweep] = useState<JsonObject | null>(null);
  const [recoverySource, setRecoverySource] = useState<JsonObject | null>(null);
  const [recoveryTiming, setRecoveryTiming] = useState<JsonObject | null>(null);
  const [backupText, setBackupText] = useState('');
  const [restored, setRestored] = useState<JsonObject | null>(null);
  const [reviewMessage, setReviewMessage] = useState<JsonObject | null>(null);
  const [outgoing, setOutgoing] = useState<ProtocolMessage | null>(null);
  const [address, setAddress] = useState('');
  // Policy amounts as typed, in bitcoin.
  const [monthly, setMonthly] = useState('');
  const [emergency, setEmergency] = useState('');
  const [step, setStep] = useState('1');
  const [destination, setDestination] = useState('');
  const [payment, setPayment] = useState('10000');
  const [paymentReview, setPaymentReview] = useState<JsonObject | null>(null);
  const [recoveredPayment, setRecoveredPayment] = useState<JsonObject | null>(null);
  const [previousRecovery, setPreviousRecovery] = useState<JsonObject | null>(null);
  const [softLimit, setSoftLimit] = useState('1000000');
  const [busy, setBusy] = useState(false);
  const [liveError, setLiveError] = useState<{ wallet?: string; vault?: string }>({});
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const privacyGeneration = useRef(0);
  const screenScroll = useRef<ScrollView>(null);
  // The open tap, if any: on the phone its link to the signer, on the signer the phone's
  // request waiting for an answer.
  const link = useRef<SignerLink | null>(null);
  const tapAbort = useRef<AbortController | null>(null);
  const tapMessage = useRef<ProtocolMessage | null>(null);
  // The signer's answer, kept until the phone has finished with it: retrying after a failed
  // broadcast reuses the approval instead of asking the signer again.
  const tapReply = useRef<ProtocolMessage | null>(null);
  const tapAfter = useRef<(() => void) | undefined>(undefined);
  const pendingReply = useRef<{ resolve: (message: ProtocolMessage) => void; reject: (error: Error) => void } | null>(null);
  const go = (next: string) => {
    if (next === 'policy' && vault) {
      // Start from the current limits, or for a first policy from a share of the balance.
      const set = vault.config.monthly_limit_sats > 0;
      const round = (sats: number) => String(Math.floor(sats / 10_000) / 10_000);
      setMonthly(round(set ? vault.config.monthly_limit_sats : vault.balance_sats / 24));
      setEmergency(round(set ? vault.config.emergency_access_limit_sats : vault.balance_sats / 4));
    }
    setPage(next); setError(''); setNotice(''); screenScroll.current?.scrollTo({ y: 0, animated: false });
  };
  useEffect(() => { if (cloudBackup) setBackupText(JSON.stringify(cloudBackup)); }, [cloudBackup]);
  // Switching devices away mid-tap moves the phone away from its signer.
  useEffect(() => () => { tapAbort.current?.abort(); link.current?.close(); }, []);

  useEffect(() => {
    let active = true;
    setSessionLoaded(false);
    transport.execute({ method: 'session' }).then(session => {
      if (!active) return;
      setIdentity(session.identity);
      setPaired(Boolean(session.paired || session.config));
      setBackupStored(Boolean(session.backup_stored));
      if (signer && session.config) setPairing(session.config);
    }).catch(e => { if (active) setError(String(e)); }).finally(() => { if (active) setSessionLoaded(true); });
    return () => { active = false; };
  }, [transport]);

  // Presentation privacy only. Wallet operations/authorization are exclusively Rust-owned.
  const hideVault = () => { privacyGeneration.current += 1; setVaultVisible(false); setVault(null); setRecoveredPayment(null); setPreviousRecovery(null); setPage('home'); setTab('wallet'); setError(''); setNotice(''); };
  useEffect(() => {
    const subscription = AppState.addEventListener('change', state => { if (state !== 'active') hideVault(); });
    return () => subscription.remove();
  }, []);

  // Live updates: sync once on open, then again whenever the transport reports chain activity.
  // Background refreshes never set busy or show errors, so they cannot block or hide user
  // actions; the manual sync button still reports failures. The vault refreshes only while
  // it is revealed, so opening the app never fetches savings.
  const liveState = useRef({ vaultVisible: false, busy: false, running: false, again: false });
  liveState.current.vaultVisible = vaultVisible;
  liveState.current.busy = busy;
  // A device runs one command at a time. Never queue a background sync ahead of a user action:
  // a hint that arrives mid-action waits until the action finishes.
  useEffect(() => { if (!busy && liveState.current.again && !liveState.current.running) void refreshInBackground(); }, [busy]);
  async function refreshInBackground() {
    const state = liveState.current;
    if (state.running || state.busy) { state.again = true; return; }
    state.running = true;
    try {
      do {
        state.again = false;
        if (liveState.current.busy) { state.again = true; break; }
        const generation = privacyGeneration.current;
        try {
          setWallet(await transport.execute({ method: 'background_wallet_snapshot' }));
          setLiveError(current => ({ ...current, wallet: undefined }));
        } catch (e) { setLiveError(current => ({ ...current, wallet: e instanceof Error ? e.message : String(e) })); }
        if (!liveState.current.vaultVisible || generation !== privacyGeneration.current) continue;
        try {
          const next = await transport.execute({ method: 'vault_snapshot' });
          if (generation === privacyGeneration.current) { setVault(next); setLiveError(current => ({ ...current, vault: undefined })); }
        } catch (e) {
          if (generation === privacyGeneration.current) setLiveError(current => ({ ...current, vault: e instanceof Error ? e.message : String(e) }));
        }
      } while (state.again);
    } finally { state.running = false; }
  }
  useEffect(() => {
    if (signer || !identity) return;
    void refreshInBackground();
    return transport.watch?.(() => void refreshInBackground());
  }, [transport, signer, identity?.vault_pubkey]);

  async function run(command: JsonObject, after?: (value: JsonObject) => void) {
    if (busy) return;
    const generation = privacyGeneration.current;
    const stillVisible = () => generation === privacyGeneration.current;
    setBusy(true); setError(''); setNotice('');
    try {
      const result = await transport.execute(command);
      after?.(result);
      // Keep completed protocol results, but never reveal late private notices after hiding.
      if (!stillVisible()) { setNotice(''); setError(''); return result; }
      if (result.txid) {
        setNotice(`Broadcast ${result.txid}`);
        if (!signer && tab === 'wallet') {
          try { setWallet(await transport.execute({ method: 'wallet_snapshot' })); }
          catch (e) { if (stillVisible()) setError(`Transaction broadcast; balance refresh failed: ${String(e)}. Sync again to refresh the display.`); }
        }
      }
      return result;
    } catch (e) { if (stillVisible()) setError(e instanceof Error ? e.message : String(e)); }
    finally { setBusy(false); }
  }
  const call = (command: JsonObject) => transport.execute(command);
  // Where a message goes. Phone to signer travels over an open tap; a signer's answer goes back
  // to the phone that asked. Anything else, or with no tap, is offered as a file.
  async function send(kind: Kind, body: JsonObject) {
    const next = { kind, body };
    if (signer) {
      const pending = pendingReply.current;
      if (pending) { pendingReply.current = null; pending.resolve(next); } else setOutgoing(next);
      return;
    }
    if (link.current && signerBound.includes(kind) && !deferred.includes(kind)) {
      await handleMessage(await link.current.request(next));
      return;
    }
    setOutgoing(next);
  }
  const progress = (text: string) => setTapState(current => current && { ...current, step: text });
  // Prepare a request in Rust, then take it to the signer: wait for a tap, send it, and handle
  // the answer, which can lead to further exchanges in the same tap.
  async function toSigner(prepare: () => Promise<ProtocolMessage>, after?: () => void) {
    if (busy || tapState) return;
    setBusy(true); setError(''); setNotice('');
    let request: ProtocolMessage;
    try { request = await prepare(); }
    catch (e) { setError(message(e)); setBusy(false); return; }
    if (!tap || !signerBound.includes(request.kind)) { setOutgoing(request); setBusy(false); after?.(); return; }
    tapMessage.current = request; tapReply.current = null; tapAfter.current = after;
    await connectAndSend();
  }
  async function connectAndSend() {
    const request = tapMessage.current;
    if (!request || !tap) return;
    const purpose: TapPurpose = request.kind === 'signer-hello' && paired && backupStored ? 'replace' : purposeOf(request.kind);
    const abort = new AbortController();
    tapAbort.current = abort;
    setBusy(true);
    try {
      if (tapReply.current) setTapState({ purpose, phase: 'working', step: 'Finishing' });
      else {
        setTapState({ purpose, phase: 'waiting' });
        const connected = await tap.connect(abort.signal);
        if (abort.signal.aborted) { connected.close(); return; }
        link.current = connected;
        setTapState(purpose === 'setup' || purpose === 'replace' ? { purpose, phase: 'working', step: 'Connected to your signer' } : { purpose, phase: 'review' });
        tapReply.current = await connected.request(request);
      }
      await handleMessage(tapReply.current);
      link.current?.close(); link.current = null; tapMessage.current = null; tapReply.current = null;
      setTapState(purpose === 'setup' ? { purpose, phase: 'done' } : null);
      tapAfter.current?.();
    } catch (e) {
      link.current?.close(); link.current = null;
      if (!abort.signal.aborted) setTapState({ purpose, phase: 'failed', error: message(e) });
    } finally { setBusy(false); }
  }
  function cancelTap() {
    if (tapReply.current) setRetained(tapReply.current);
    tapAbort.current?.abort(); link.current?.close(); link.current = null; tapMessage.current = null; tapReply.current = null;
    setTapState(null); setBusy(false);
  }
  const propose = (command: JsonObject, kind: Kind, after?: () => void) =>
    toSigner(async () => ({ kind, body: await call(command) }), after);
  const setupVault = () => toSigner(async () => ({ kind: 'signer-hello', body: { network, phone: identity?.vault_pubkey } }));
  async function shareOutgoing() {
    if (!outgoing || busy) return;
    if (!signer && tap && signerBound.includes(outgoing.kind)) {
      const pending = outgoing;
      void toSigner(async () => pending, () => setOutgoing(null));
      return;
    }
    setBusy(true); setError('');
    const generation = privacyGeneration.current;
    try {
      const result = await deliver(outgoing);
      if (result === 'completed') setOutgoing(null);
      else if (generation === privacyGeneration.current) setNotice('File offered for sharing. Check that the other device received it; this message remains available to resend.');
    }
    catch (e) { if (generation === privacyGeneration.current) setError(`Sharing could not be confirmed: ${String(e)}. The message is still available to retry.`); }
    finally { setBusy(false); }
  }
  const initialize = () => run({ method: 'initialize', network }, value => {
    setIdentity(value);
    if (signer && !tap) void send('signer-identity', value);
  });
  // Retry / Catch up: an explicit full sync, which also reports its own errors.
  const syncWallet = () => run({ method: 'wallet_snapshot' }, value => {
    setWallet(value); setLiveError(current => ({ ...current, wallet: undefined }));
  });
  const syncVault = () => {
    const generation = privacyGeneration.current;
    return run({ method: 'vault_snapshot' }, value => {
      if (generation === privacyGeneration.current) { setVault(value); setLiveError(current => ({ ...current, vault: undefined })); }
    });
  };
  const reveal = () => {
    const generation = privacyGeneration.current;
    return run({ method: 'vault_snapshot' }, value => {
      if (generation === privacyGeneration.current) { setVault(value); setVaultVisible(true); }
    });
  };
  const reviewRecovered = () => {
    const generation = privacyGeneration.current;
    return run({ method: 'review_recovered_wallet' }, value => {
      if (generation === privacyGeneration.current) setRecoveredPayment(value);
    });
  };
  const reviewPrevious = (vault_id: string) => {
    const generation = privacyGeneration.current;
    return run({ method: 'review_previous_vault_recovery', vault_id }, value => {
      if (generation === privacyGeneration.current) setPreviousRecovery(value);
    });
  };
  function proposal(method = 'propose_policy') {
    const btcToSats = (text: string) => /^\d*\.?\d{0,8}$/.test(text.trim()) && text.trim() !== '.' ? Math.round(Number(text.trim() || '0') * 100_000_000) : NaN;
    const limits = method === 'propose_policy' ? { monthly_limit_sats: btcToSats(monthly), emergency_access_limit_sats: btcToSats(emergency) } : {};
    if (Object.values(limits).some(Number.isNaN)) { setError('Enter amounts in bitcoin, like 0.05.'); return; }
    void propose({ method, ...limits }, 'policy-proposal', () => setPage('home'));
  }
  async function exportBackup() {
    if (busy) return;
    setBusy(true); setError('');
    const generation = privacyGeneration.current;
    try {
      const backup = await transport.execute({ method: 'export_backup' });
      if (saveBackup) {
        const result = await saveBackup(backup);
        if (generation === privacyGeneration.current) setNotice(result === 'offered'
          ? 'Backup file offered for export. Check it was saved separately from the signer; the app cannot confirm delivery.'
          : 'Encrypted backup exported separately from the signer.');
      }
      else if (generation === privacyGeneration.current) { setBackupText(JSON.stringify(backup)); setNotice('Encrypted backup ready to copy. Keep it separately from your signer.'); }
    } catch (e) { if (generation === privacyGeneration.current) setError(String(e)); }
    finally { setBusy(false); }
  }
  function requestRestore() {
    let backup: JsonObject;
    try { backup = parseEncryptedBackup(backupText); }
    catch (e) { setError(`Cannot read the encrypted cloud backup: ${String(e)}`); return; }
    void propose({ method: 'propose_restore', backup }, 'restore-proposal');
  }
  function requestContactRestore() {
    try {
      void run({ method: 'propose_contact_restore', backup: parseEncryptedBackup(backupText), contact: receivedContact }, value => void send('contact-restore-proposal', value));
    } catch (e) { setError(`Cannot read the encrypted cloud backup: ${String(e)}`); }
  }

  // Every received message, by file or by tap, on either device. Rust validates each one;
  // this only routes it and shows what comes back. Throws so the sender learns of failure.
  async function handleMessage(received: ProtocolMessage) {
    const generation = privacyGeneration.current;
    const showPrivate = (nextPage?: string) => {
      if (generation !== privacyGeneration.current) return;
      setTab('vault');
      if (nextPage) setPage(nextPage);
    };
    const body = received.body;
    const reviewWith = async (method: string, kind: ReviewKind, args: JsonObject, keep = false) => {
      const value = await call({ method, ...args });
      setReview(value); setReviewKind(kind); if (keep) setReviewMessage(body);
    };
    const broadcast = (result: JsonObject) => { if (result.txid) setNotice(`Broadcast ${result.txid}`); };
    switch (received.kind) {
      case 'recovery-contact': setReceivedContact(await call({ method: 'inspect_recovery_contact', contact: body })); showPrivate(paired ? 'add-contact' : 'contact-recover'); return;
      case 'contact-restore-proposal':
        setContactRestoreReview(await call({ method: 'review_contact_restore', request: body }));
        setReviewMessage(body); setVault(null); setVaultVisible(false); showPrivate('contact-approve'); return;
      case 'contact-restore-approved':
        setContactRecovery(await call({ method: 'store_contact_restored_key', message: body }));
        setRecoverySource(null); showPrivate('contact-recover'); setNotice('Contact-assisted key recovery complete. Moving vault funds still requires the Bitcoin recovery delay.'); return;
      // Signer: a phone asks who this is. A new signer creates its key on its first tap.
      case 'signer-hello': {
        // Setup and replacement both need a signer without a vault, or this phone's own when
        // finishing an interrupted setup. Refusing here keeps the phone from pairing with it.
        if (pairing && pairing.phone_vault_pubkey !== body.phone) throw new Error('This signer already protects another vault.');
        let own = identity;
        if (!own) { own = await call({ method: 'initialize', network }); setIdentity(own); }
        await send('signer-identity', own!); return;
      }
      case 'signer-identity':
        if (paired && backupStored) {
          setSignerCandidate(await call({ method: 'inspect_signer_replacement', signer: body })); showPrivate('replace-signer');
          return;
        }
        progress('Creating your vault');
        { const setup = await call({ method: 'setup_vault', signer: body }); setPaired(true); await send('vault-setup', setup); }
        return;
      case 'vault-setup': {
        const accepted = await call({ method: 'accept_setup', config: body.config, backup: body.backup });
        setPairing(body.config); setPaired(true);
        await send('vault-accepted', accepted); return;
      }
      case 'vault-accepted':
        // A signer finishing a rotation or replacement only confirms it switched vaults.
        if (!body.backup) { setNotice('Your signer now uses the new vault.'); return; }
        progress('Saving your recovery backup');
        await call({ method: 'store_backup', message: body.backup }); setBackupStored(true);
        if (saveBackup) {
          try { await saveBackup(await call({ method: 'export_backup' })); }
          catch { setNotice('Vault ready. Saving the recovery backup failed: export it from Vault settings.'); }
        }
        return;
      case 'recovery-vault-request':
        if (!pairing) throw new Error('This signer has no vault yet.');
        setReview({ vault_address: pairing.vault_address }); setReviewKind('share-vault'); return;
      case 'policy-proposal': await reviewWith('review_policy', 'policy', { request: body }); return;
      case 'sweep-proposal': await reviewWith('review_sweep', 'sweep', { request: body }); return;
      case 'sweep-approved': broadcast(await call({ method: 'activate_sweep', approval: body })); return;
      case 'recovery-vault': setRecoverySource(await call({ method: 'import_recovery_bundle', bundle: body })); showPrivate('signer-recovery'); return;
      case 'offline-action-proposal': await reviewWith('review_offline_action', 'offline-action', { request: body }); return;
      case 'offline-action-approved': broadcast(await call({ method: 'activate_offline_action', approval: body })); return;
      case 'restore-proposal': await reviewWith('review_restore', 'restore', { request: body }); return;
      case 'restore-approved':
        setRestored(await call({ method: 'store_restored_key', message: body })); showPrivate();
        setNotice('Old phone key recovered into encrypted storage. Your new wallet key is unchanged.'); return;
      case 'phone-rotation-proposal': await reviewWith('review_phone_rotation', 'rotation', { request: body }, true); return;
      case 'phone-rotation-approved': {
        const completed = await call({ method: 'activate_rotation', approval: body });
        setPaired(true); setBackupStored(true); setRestored(null); setRecoverySource(null); setVault(null); setVaultVisible(false); showPrivate('home');
        await send('phone-rotation-completed', completed);
        setNotice('Vault rotation broadcast. Once it confirms, finish on your signer.'); return;
      }
      case 'phone-rotation-completed': setReview(body); setReviewKind('rotation-adoption'); return;
      case 'signer-replacement-proposal': await reviewWith('review_signer_replacement', 'signer-replacement', { request: body }, true); return;
      case 'signer-replacement-approved': {
        const completed = await call({ method: 'activate_signer_replacement', approval: body });
        setSignerCandidate(null); setVault(null); setVaultVisible(false); showPrivate('home');
        await send('signer-replacement-completed', completed);
        setNotice('Signer replacement broadcast. Once it confirms, finish on your new signer.'); return;
      }
      case 'signer-replacement-completed': setReview(body); setReviewKind('signer-adoption'); return;
      case 'backup-proposal': await reviewWith('review_backup', 'backup', { message: body }, true); return;
      case 'backup-approved': {
        const value = await call({ method: 'store_backup', message: body });
        setBackupStored(true);
        setVault(previous => previous ? { ...previous, recovery_backup_stored: value.stored } : previous);
        setNotice('Encrypted cloud backup stored locally. The signer can recover the phone key.'); return;
      }
      case 'policy-approved':
        await call({ method: 'activate_policy', approval: body });
        setNotice(network === 'regtest' ? 'Policy activated. Confirm its rollover with the regtest controls.' : 'Policy broadcast. Wait for network confirmation, then sync the vault.'); return;
      default: throw new Error('Unsupported Anzen protocol message. Check that both devices use a compatible app version.');
    }
  }
  async function finishRetained() {
    if (!retained || busy) return;
    setBusy(true); setError(''); setNotice('');
    try { await handleMessage(retained); setRetained(null); }
    catch (e) { setError(message(e)); }
    finally { setBusy(false); }
  }
  // A message received as a file: opening it is the user's go-ahead to process it.
  async function importMessage() {
    if (!incoming || busy) return;
    const received = incoming;
    setBusy(true); setError(''); setNotice('');
    try { await handleMessage(received); dismissIncoming(); }
    catch (e) { setError(message(e)); }
    finally { setBusy(false); }
  }
  // The signer side of a tap: handle the phone's request, and answer once there is a reply,
  // which may wait for the user's approval. The phone moving away abandons the request.
  const handleRef = useRef(handleMessage);
  handleRef.current = handleMessage;
  const decline = () => { const pending = pendingReply.current; pendingReply.current = null; pending?.reject(new Error(DECLINED)); };
  useEffect(() => {
    if (!signer || !tap) return;
    return tap.answer((request, signal) => new Promise<ProtocolMessage>((resolve, reject) => {
      if (pendingReply.current) { reject(new Error('Your signer is busy with another request.')); return; }
      const finish = () => { signal.removeEventListener('abort', disconnect); setLinked(false); };
      const pending = { resolve: (reply: ProtocolMessage) => { finish(); resolve(reply); }, reject: (error: Error) => { finish(); reject(error); } };
      const disconnect = () => {
        if (pendingReply.current !== pending) return;
        pendingReply.current = null; setReview(null); setReviewMessage(null);
        pending.reject(new Error('The phone disconnected.'));
      };
      pendingReply.current = pending;
      signal.addEventListener('abort', disconnect);
      setLinked(true); setError(''); setNotice(''); setBusy(true);
      handleRef.current(request).catch(error => {
        if (pendingReply.current === pending) pendingReply.current = null;
        setError(message(error)); pending.reject(error instanceof Error ? error : new Error(String(error)));
      }).finally(() => setBusy(false));
    }));
  }, [signer, tap]);
  const inPrivate = signer || tab === 'vault';
  const walletSheet = Boolean(identity && !signer && tab === 'wallet' && ['send', 'receive', 'confirm-send', 'transaction'].includes(page));
  const vaultSheetLabels: Record<string, string> = { settings: 'Vault settings', policy: 'Vault policy', withdraw: 'Withdraw', monthly: 'Monthly allowance', emergency: 'Emergency access', deposit: 'Vault deposit', sweep: 'Move vault funds', recovery: 'Phone recovery' };
  const vaultSheet = Boolean(identity && !signer && tab === 'vault' && vaultVisible && vaultSheetLabels[page]);
  const anySheet = walletSheet || vaultSheet || Boolean(tapState);
  const feedback = <>
    {(busy || !sessionLoaded) && <ActivityIndicator color={colors.mint} style={{ margin: 20 }} />}
    {!!notice && <View style={s.notice}><Text selectable style={s.noticeText}>{notice}</Text></View>}
    {!!error && <View style={[s.notice, s.error]}><Text selectable accessibilityRole="alert" style={s.errorText}>{error}</Text></View>}
  </>;
  // The block the revealed vault reflects, as a data attribute: lets tests wait for live updates.
  return <View style={s.screen} testID="device-screen" {...({ dataSet: { vaultHeight: vaultVisible && vault ? vault.chain_height : '' } } as object)}>
    <DarkBackground/><DeviceStatus/><DarkEffects/>
    {identity && !signer && <View style={{ position: 'absolute', top: Platform.OS === 'web' ? 56 : 14, left: 26, right: 26, flexDirection: 'row', justifyContent: 'space-between', zIndex: 10 }} pointerEvents="box-none">
      <IconButton label={page !== 'home' && !anySheet ? (tab === 'wallet' ? 'Back to wallet' : 'Back to vault') : 'Settings'} name={page !== 'home' && !anySheet ? 'close' : 'settings'} onPress={() => go(page !== 'home' ? 'home' : 'settings')} disabled={tab === 'vault' && !vaultVisible && page === 'home'}/>
      <View style={{ flexDirection: 'row', gap: 8 }}>

        {tab === 'vault' && <IconButton label="Hide vault" name="close" onPress={hideVault}/>}
      </View>
    </View>}
    <ScrollView ref={screenScroll} contentContainerStyle={[s.content, tab === 'vault' && page !== 'home' && !vaultSheet && { paddingTop: 56 }]} keyboardShouldPersistTaps="handled" showsVerticalScrollIndicator={false} accessibilityElementsHidden={anySheet} importantForAccessibility={anySheet ? 'no-hide-descendants' : 'auto'}>
      {!identity && !(signer && tap) ? <>
        <View style={s.hero}><Text style={s.eyebrow}>{signer ? 'Your second device' : 'Cold storage made easy'}</Text>
          <Text style={[s.balance, { fontSize: 34 }]}>{signer ? 'Your policy.\nYour approval.' : 'Everyday simple.\nBitcoin secure.'}</Text></View>
        <Text style={s.subtext}>{signer ? 'Use this separate device to review and approve vault policy. It never needs your wallet phone’s private storage.' : 'Create your everyday wallet, then add a signer to protect your savings.'}</Text>
        {signer && <Text style={s.tiny}>A second phone is not a secure-element hardware wallet. Do not run both roles on the same phone for independent protection.</Text>}
        <Button title={signer ? 'Create signer' : 'Create wallet'} primary onPress={initialize} disabled={busy || !sessionLoaded} />
      </> : <>
        {incoming && <View style={s.notice}><Text style={s.noticeText}>Incoming · {incoming.kind.replaceAll('-', ' ')}</Text><Button title="Open received message" onPress={importMessage} disabled={busy} /></View>}
        {retained && inPrivate && <View style={s.notice}><Text style={s.noticeText}>Approved on your signer · not finished yet</Text><Button title="Finish" onPress={() => void finishRetained()} disabled={busy} /></View>}
        {signer ? <>
          {review ? <View style={s.hero}><Text style={s.eyebrow}>Review & approve</Text></View> : tap ? <>
            {/* A signer has nothing to press until a phone asks: it is held to the phone. */}
            <View style={[s.hero, { paddingTop: 72 }]}><Text style={s.eyebrow}>Signer</Text>
              <TapArt phase={linked ? 'working' : 'idle'}/>
              <Text style={[s.balance, { fontSize: 32, lineHeight: 40, textAlign: 'center', marginTop: 30 }]}>{paired ? 'Ready.' : 'Hold me near\nyour phone.'}</Text>
              <Text style={s.balanceCaption}>{linked ? 'Connected to your phone' : paired ? 'Hold near your phone to approve.' : 'On your phone, open Vault and choose Set up vault.'}</Text></View>
            {pairing && <View style={{ marginTop: 40, alignItems: 'center' }}><Text style={[s.tiny, { color: colors.quiet }]}>Your vault</Text><AddressLines address={pairing.vault_address} vault testID="signer-vault-address"/></View>}
          </> : <>
            <View style={s.hero}><Text style={s.eyebrow}>Signer</Text><Text style={[s.balance, { fontSize: 38 }]}>{paired ? 'Ready.' : 'Your second key.'}</Text><Text style={s.balanceCaption}>Your policy. Your approval.</Text></View>
            {!pairing && <Button title="Share signer public key" onPress={() => void send('signer-identity', identity!)} disabled={busy} />}
            {pairing && <><Text style={s.heading}>Your vault</Text><Text selectable style={s.mono}>{pairing.vault_address}</Text></>}
            {paired && <Button title="Share public recovery vault" disabled={busy} onPress={() => run({ method: 'export_recovery_bundle' }, value => void send('recovery-vault', value))} />}
          </>}
          {review && reviewKind === 'share-vault' && <><Text style={s.heading}>Share your vault with this phone?</Text>
            <Text selectable style={s.mono}>{review.vault_address}</Text>
            <Text style={s.subtext}>The phone gets your vault’s public details to start a recovery. It cannot move funds without this signer or Bitcoin’s recovery delay.</Text>
            <Button title="Share vault details" primary disabled={busy} onPress={() => run({ method: 'export_recovery_bundle' }, value => { void send('recovery-vault', value); setReview(null); })} />
            <Button title="Don’t share" disabled={busy} onPress={() => { setReview(null); decline(); }} />
          </>}
          {review && reviewKind === 'offline-action' && <><Text style={s.heading}>{review.action === 'hww_recovery' ? 'Recover vault funds' : 'Revoke all vault actions'}</Text>
            <Text style={s.tiny}>Vault</Text><Text selectable style={s.mono}>{review.vault_address}</Text>
            <Text style={s.tiny}>Destination</Text><Text selectable style={s.mono}>{review.destination}</Text>
            <View style={s.row}><Text style={s.tiny}>Amount</Text><Text testID="offline-action-amount" style={s.rowValue}>{btc(review.amount_sats)} BTC</Text></View>
            <View style={s.row}><Text style={s.tiny}>Fee</Text><Text style={s.rowValue}>{review.fee_sats} sats</Text></View>
            <Text style={s.subtext}>{review.recovery_delay_blocks
              ? `Bitcoin requires ${review.recovery_delay_blocks} blocks since each input confirmed. This offline signer cannot check maturity; signing does not unlock funds early.`
              : 'Spend the reviewed live connectors to disable their monthly and emergency actions. Vault savings stay in place; only connector change goes to this destination. Revocation wins only once confirmed before a conflicting action.'}</Text>
            <Button title="Approve offline action" primary disabled={busy} onPress={() => run({ method: 'approve_offline_action', review_id: review.review_id }, value => { void send('offline-action-approved', value); setReview(null); })} />
            <Button title="Reject offline action" danger disabled={busy} onPress={() => run({ method: 'reject_offline_action', review_id: review.review_id }, () => { setReview(null); decline(); })} />
          </>}
          {review && reviewKind === 'policy' && <><Text style={s.heading}>Annual vault policy</Text>
            {[
              ['Vault balance', `₿${btc(review.vault_input_sats)}`, ''],
              ['Monthly allowance', `₿${btc(review.monthly_limit_sats)} a month`, `To your wallet · ${review.funded_withdrawals} of 12 months funded`],
              ['Emergency access', review.emergency_delay_seconds ? `Up to ₿${btc(review.emergency_access_limit_sats)}` : 'Off', review.emergency_delay_seconds ? `Phone only, after a one-week delay (${review.emergency_delay_seconds.toLocaleString('en-US')} seconds)` : 'No emergency withdrawal'],
            ].map(([label, value, detail]) => <View key={label} style={[s.row, { paddingVertical: 14 }]}><Text style={s.tiny}>{label}</Text><Text style={[s.rowValue, { marginTop: 6 }]}>{value}</Text>{!!detail && <Text style={[s.tiny, { marginTop: 5 }]}>{detail}</Text>}</View>)}
            <View style={{ flexDirection: 'row', justifyContent: 'space-between', marginTop: 18 }}><Text style={s.tiny}>Rollover fee</Text><Text style={s.tiny}>{review.rollover_fee_sats} sats</Text></View>
            <Text style={s.subtext}>One approval signs {review.transaction_count} transactions. The signer rebuilds every transaction from these parameters before signing.</Text>
            {review.funding_warning && <Text style={s.errorText}>{review.funding_warning}</Text>}
            {review.previous_policy_warning && <Text style={s.errorText}>{review.previous_policy_warning}</Text>}
            <Button title="Approve and sign policy" primary disabled={busy} onPress={() => run({ method: 'approve_policy', review_id: review.review_id }, value => {
              void send('policy-approved', value); setReview(null);
            })} />
            <Button title="Reject policy" danger disabled={busy} onPress={() => run({ method: 'reject_policy', review_id: review.review_id }, () => { setReview(null); decline(); })} />
          </>}
          {review && reviewKind === 'sweep' && <><Text style={s.heading}>Move vault funds</Text>
            <View style={s.row}><Text style={s.tiny}>Destination</Text><Text selectable style={s.mono}>{review.destination}</Text></View>
            <View style={s.row}><Text style={s.tiny}>Amount</Text><Text style={s.rowValue}>{btc(review.amount_sats)} BTC</Text></View>
            <View style={s.row}><Text style={s.tiny}>Fee</Text><Text style={s.rowValue}>{review.fee_sats} sats</Text></View>
            <Text style={s.subtext}>This immediately moves the reviewed vault outputs. It does not create a new annual policy. Verify the destination independently.</Text>
            <Button title="Approve vault transfer" primary disabled={busy} onPress={() => run({ method: 'approve_sweep', review_id: review.review_id }, value => { void send('sweep-approved', value); setReview(null); })} />
            <Button title="Reject vault transfer" danger disabled={busy} onPress={() => run({ method: 'reject_sweep', review_id: review.review_id }, () => { setReview(null); decline(); })} />
          </>}
          {review && reviewKind === 'backup' && <><Text style={s.heading}>Protect phone recovery</Text>
            <Text selectable style={s.mono}>{review.vault_address}</Text>
            <Text style={s.subtext}>Protect the backup’s random encryption key. The phone seed and its ciphertext stay on the wallet/cloud side, not on this signer. This does not approve spending.</Text>
            <Text style={s.rowValue}>Recovery contacts: {review.friend_fingerprints?.length ?? 0}</Text>
            {review.friend_fingerprints?.map((fingerprint: string) => <Text key={fingerprint} selectable style={s.mono}>{fingerprint}</Text>)}
            {!!review.friend_fingerprints?.length && <Text style={s.subtext}>Compare each fingerprint with the contact. Any one of these contacts plus the cloud backup can recover the phone key; the Bitcoin recovery delay still applies. This is not threshold sharing.</Text>}
            <Button title="Approve recovery backup" primary disabled={busy} onPress={() => run({ method: 'approve_backup', review_id: review.review_id, message: reviewMessage }, value => { void send('backup-approved', value); setReview(null); setReviewMessage(null); })} />
            <Button title="Reject recovery backup" danger disabled={busy} onPress={() => run({ method: 'reject_backup', review_id: review.review_id }, () => { setReview(null); setReviewMessage(null); decline(); })} />
          </>}
          {review && reviewKind === 'restore' && <><Text style={s.heading}>Unlock phone recovery</Text>
            <Text style={s.tiny}>Recovery vault</Text><Text selectable style={s.mono}>{review.vault_address}</Text>
            <Text style={s.tiny}>Replacement phone identity · compare on both devices</Text><Text selectable style={s.mono}>{review.recipient_public_key}</Text>
            <Text style={s.tiny}>Cloud backup fingerprint</Text><Text selectable style={s.mono}>{review.backup_id}</Text>
            <Text style={s.subtext}>Release this backup’s decryption key only to the verified replacement phone. This signer cannot inspect the cloud payload; the phone authenticates it. Rotate the recovered vault key afterwards.</Text>
            <Button title="Approve phone recovery" primary disabled={busy} onPress={() => run({ method: 'approve_restore', review_id: review.review_id }, value => { void send('restore-approved', value); setReview(null); })} />
            <Button title="Reject phone recovery" danger disabled={busy} onPress={() => run({ method: 'reject_restore', review_id: review.review_id }, () => { setReview(null); decline(); })} />
          </>}
          {review && reviewKind === 'rotation' && <><Text style={s.heading}>Rotate the recovered vault</Text>
            <Text style={s.tiny}>New phone identity · compare on the replacement phone</Text><Text selectable style={s.mono}>{review.new_phone_public_key}</Text>
            <Text style={s.tiny}>Old vault</Text><Text selectable style={s.mono}>{review.old_vault_address}</Text>
            <Text style={s.tiny}>New vault</Text><Text selectable style={s.mono}>{review.new_vault_address}</Text>
            <Text style={s.rowValue}>{btc(review.amount_sats)} BTC transferred</Text>
            <Text style={s.subtext}>Preserve {btc(review.monthly_limit_sats)} BTC monthly and {btc(review.emergency_access_limit_sats)} BTC emergency. {review.funded_withdrawals} monthly withdrawals funded.</Text>
            <Text style={s.subtext}>Vault transfer fee: {review.sweep_fee_sats} sats. {review.revokes_connectors ? 'Revoke the old connectors and return their change to the new vault.' : 'No live old connectors were supplied.'}</Text>
            {review.previous_policy_warning && <Text style={s.errorText}>{review.previous_policy_warning}</Text>}
            <Text style={s.subtext}>Renew the policy timelocks and protect a fresh encrypted phone backup. Old wallet funds are separate from this vault transfer. The node must accept and confirm these transactions.</Text>
            <Text style={s.rowValue}>Recovery contacts: {review.friend_fingerprints?.length ?? 0}</Text>
            {review.friend_fingerprints?.map((fingerprint: string) => <Text key={fingerprint} selectable style={s.mono}>{fingerprint}</Text>)}
            {!!review.friend_fingerprints?.length && <Text style={s.subtext}>Compare these fingerprints with your existing contacts. Each contact receives access to the new backup; any one contact plus that backup can recover the new phone key.</Text>}
            <Button title="Approve phone rotation" primary disabled={busy} onPress={() => run({ method: 'approve_phone_rotation', review_id: review.review_id, request: reviewMessage }, value => { void send('phone-rotation-approved', value); setReview(null); setReviewMessage(null); })} />
            <Button title="Reject phone rotation" danger disabled={busy} onPress={() => run({ method: 'reject_phone_rotation', review_id: review.review_id }, () => { setReview(null); setReviewMessage(null); decline(); })} />
          </>}
          {review && reviewKind === 'rotation-adoption' && <><Text style={s.heading}>Finish rotation pairing</Text>
            <Text selectable style={s.mono}>{review.new_vault_address}</Text>
            <Text style={s.subtext}>The phone reports its approved rotation was broadcast. Check confirmation on the phone before selecting the new vault here. This offline signer cannot check the chain.</Text>
            <Button title="Use the rotated vault" primary disabled={busy} onPress={() => run({ method: 'adopt_phone_rotation', request_id: review.request_id }, value => { setPairing(value); setReview(null); setNotice('Signer paired with the rotated vault. The spending limits are preserved.'); void send('vault-accepted', { vault_address: value.vault_address }); })} />
            <Button title="Keep previous pairing" disabled={busy} onPress={() => { setReview(null); decline(); }} />
          </>}
          {review && reviewKind === 'signer-replacement' && <><Text style={s.heading}>Replace the unavailable signer</Text>
            <Text style={s.tiny}>Current vault</Text><Text selectable style={s.mono}>{review.old_vault_address}</Text>
            <Text style={s.tiny}>Surviving phone identity · compare on the phone</Text><Text selectable style={s.mono}>{review.phone_public_key}</Text>
            <Text style={s.tiny}>New signer identity</Text><Text selectable style={s.mono}>{review.signer_public_key}</Text>
            <Text style={s.tiny}>New vault</Text><Text selectable style={s.mono}>{review.new_vault_address}</Text>
            <Text style={s.rowValue}>{btc(review.amount_sats)} BTC to move</Text>
            <Text style={s.subtext}>Preserve {btc(review.monthly_limit_sats)} BTC monthly and {btc(review.emergency_access_limit_sats)} BTC emergency. {review.funded_withdrawals} monthly withdrawals funded.</Text>
            <Text style={s.subtext}>The phone recovery transaction requires {review.recovery_delay_blocks} blocks ({network === 'signet' ? '~21 days on Mutinynet' : '~14 months'}) on every old vault input. Approval on this new signer cannot unlock it early. This signer does not check chain maturity.</Text>
            <Text style={s.tiny}>Recovery transfer: {review.input_count} inputs · {review.recovery_fee_sats} sats fee. Included old connectors return to the new vault; its policy delays restart on confirmation.</Text>
            <Text style={s.rowValue}>Recovery contacts: {review.friend_fingerprints?.length ?? 0}</Text>
            {review.friend_fingerprints?.map((fingerprint: string) => <Text key={fingerprint} selectable style={s.mono}>{fingerprint}</Text>)}
            {!!review.friend_fingerprints?.length && <Text style={s.subtext}>Compare these fingerprints with your existing contacts. The fresh backup keeps these contacts and is protected by this new signer.</Text>}
            <Button title="Approve signer replacement" primary disabled={busy} onPress={() => run({ method: 'approve_signer_replacement', review_id: review.review_id, request: reviewMessage }, value => { void send('signer-replacement-approved', value); setReview(null); setReviewMessage(null); })} />
            <Button title="Reject signer replacement" danger disabled={busy} onPress={() => run({ method: 'reject_signer_replacement', review_id: review.review_id }, () => { setReview(null); setReviewMessage(null); decline(); })} />
          </>}
          {review && reviewKind === 'signer-adoption' && <><Text style={s.heading}>Pair the replacement signer</Text>
            <Text selectable style={s.mono}>{review.new_vault_address}</Text>
            <Text style={s.subtext}>The phone reports the approved recovery was broadcast. Check confirmation on the phone before pairing. This offline signer cannot verify the chain.</Text>
            <Button title="Use the replacement vault" primary disabled={busy} onPress={() => run({ method: 'adopt_signer_replacement', request_id: review.request_id }, value => { setPairing(value); setPaired(true); setReview(null); setNotice('Replacement signer paired. The phone key and spending limits are unchanged.'); void send('vault-accepted', { vault_address: value.vault_address }); })} />
            <Button title="Wait before pairing" disabled={busy} onPress={() => { setReview(null); decline(); }} />
          </>}
        </> : tab === 'wallet' ? <>
          {(page === 'home' || walletSheet) && <><View style={s.hero}><Text style={s.eyebrow}>Wallet</Text><Balance sats={wallet?.available_sats} /><Text style={s.balanceCaption}>Available bitcoin</Text>
            <SyncStatus error={liveError.wallet} behind={wallet?.behind_blocks} busy={busy} onSync={() => void syncWallet()}/>
            {wallet && wallet.pending_sats > 0 && <Text style={s.tiny}>{btc(wallet.pending_sats)} BTC awaiting confirmation</Text>}
            {wallet && wallet.immature_sats > 0 && <Text style={s.tiny}>{btc(wallet.immature_sats)} BTC in immature mining rewards</Text>}
            <View style={s.actions}><ActionPill title="Send" label="↑ Send" icon="send" onPress={() => setPage('send')} /><ActionPill title="Receive" label="↓ Receive" icon="receive" onPress={() => run({ method: 'receive_address' }, value => { setAddress(value.address); setPage('receive'); })} disabled={busy}/></View></View>
            <WalletHistory items={wallet?.activity || []} loaded={Boolean(wallet)} onSelect={entry => { setSelectedActivity(entry); setPage('transaction'); }}/></>}
          {page === 'settings' && <><View style={s.hero}><Text style={s.eyebrow}>Settings</Text></View>
            <View style={{ marginTop: 48, marginHorizontal: 4 }}>
              <MenuRow title="Be a recovery contact" subtitle="Help someone protect their bitcoin" disabled={busy} onPress={() => run({ method: 'recovery_contact' }, value => { setContact(value.contact); setPage('contact'); })}/>
              <MenuRow title="Recover with a contact" subtitle="Restore with a trusted person's help" disabled={busy} onPress={() => { setTab('vault'); setPage('contact-recover'); }}/>
              <Text style={s.footnote}>anzen · {identity?.network || network}</Text>
            </View></>}
          {page === 'contact' && <><Text style={s.heading}>Help someone recover</Text>
            <Text style={s.subtext}>Create a separate OpenPGP recovery key on this device. Only the public key is shared. Keep this device safe: it can help recover someone’s phone key if they approve it as a contact.</Text>
            {contact ? <><Text style={s.rowValue}>{contact.name}</Text><Text selectable testID="contact-fingerprint" style={s.mono}>{contact.fingerprint}</Text>
              <Button title="Share contact public key" disabled={busy} onPress={() => void send('recovery-contact', contact)} />
            </> : <><Field label="Recovery contact name" value={contactName} onChange={setContactName} />
              <Button title="Create recovery contact key" primary disabled={busy} onPress={() => run({ method: 'generate_recovery_contact', name: contactName }, setContact)} />
            </>}
            <Button title="Back to wallet" onPress={() => setPage('home')} />
          </>}
        </> : !vaultVisible ? page === 'home' && (paired && backupStored ? <>
          <View style={[s.hero, { paddingBottom: 28 }]}><Text style={s.eyebrow}>Private space</Text><Text style={[s.balance, { fontSize: 34 }]}>Your vault.</Text><Text style={s.balanceCaption}>Savings stay hidden until you’re ready.</Text></View>
          <Button title="Reveal vault" primary onPress={reveal} disabled={busy} />
        </> : <>
          {/* Setup is one tap: the signer's key, the vault and the recovery backup in one go. */}
          <View style={[s.hero, { paddingTop: 72 }]}><Text style={s.eyebrow}>Vault</Text>
            <TapArt phase="idle"/>
            <Text style={[s.balance, { fontSize: 32, lineHeight: 40, textAlign: 'center', marginTop: 30 }]}>{paired ? 'Almost there.' : 'Protect your\nsavings.'}</Text>
            <Text style={[s.balanceCaption, { lineHeight: 18, maxWidth: 270 }]}>{paired ? 'Hold your signer to your phone once more to save your recovery backup.' : 'A vault needs two keys: this phone and a signer you keep somewhere safe.'}</Text></View>
          {tap ? <View style={{ marginTop: 34 }}>
            <Button title={paired ? 'Finish setup' : 'Set up vault'} primary onPress={setupVault} disabled={busy} />
            {paired ? <Button title="Reveal vault" onPress={reveal} disabled={busy} />
              : <Button title="Recover with your signer" onPress={() => toSigner(async () => ({ kind: 'recovery-vault-request', body: {} }))} disabled={busy} />}
          </View> : <>
            <Text style={[s.subtext, { textAlign: 'center' }]}>{paired ? 'Waiting for your signer to accept the vault.' : 'Receive your signer’s public key to set up your vault.'}</Text>
            {paired && <Button title="Reveal vault" primary onPress={reveal} disabled={busy} />}
          </>}
        </>) : (page === 'home' || vaultSheet) && <>
          <View style={s.hero}><Text style={s.eyebrow}>Vault</Text><Balance sats={vault?.balance_sats} /><Text style={s.balanceCaption}>Confirmed bitcoin · protected by two keys</Text>
          <SyncStatus error={liveError.vault} busy={busy} onSync={() => void syncVault()}/>
            <View style={s.actions}><ActionPill title="Withdraw" icon="send" onPress={() => go('withdraw')}/><ActionPill title="Deposit" icon="receive" onPress={() => go('deposit')}/></View>
          </View>
          {vault && <VaultSummary vault={vault} onMonthly={() => go('monthly')} onEmergency={() => go('emergency')} onPolicy={() => go('policy')}/>}
          <View style={{ marginTop: 26 }}><MenuRow title="Vault settings" subtitle="Policy, backups and recovery" icon="settings" onPress={() => go('settings')}/></View>
        </>}
      </>}
      {identity && !signer && tab === 'vault' && <>
        {page === 'recovered-wallet' && !recoveredPayment && <><Text style={s.heading}>Recovered wallet</Text><Text style={s.subtext}>Check the old phone’s ordinary wallet for funds you can move to this phone.</Text><Button title="Review recovered wallet funds" disabled={busy} onPress={reviewRecovered}/></>}
        {page === 'retired' && <><Text style={s.heading}>Retired vaults</Text>
          <Text style={s.subtext}>Old addresses remain monitored. Each new deposit has its own recovery delay.</Text>
          {vault?.previous_vaults?.map((old: JsonObject) => <View key={old.vault_id} testID={`previous-vault-${old.vault_id}`} style={s.row}>
            <Text selectable style={s.mono}>{old.vault_address}</Text>
            <Text testID="previous-vault-balance" style={s.rowValue}>{btc(old.phone.total_sats)} BTC</Text>
            {old.phone.blocks_remaining === null ? <Text style={s.tiny}>No confirmed funds. This address remains monitored for later deposits.</Text> : <>
              <Text testID="previous-phone-blocks" style={s.subtext}>{old.phone.blocks_remaining} blocks until phone recovery</Text>
              <Text testID="previous-hww-blocks" style={s.tiny}>{old.hww.blocks_remaining} blocks until the old HWW can recover</Text>
              <Text testID="previous-hww-height" style={s.tiny}>Old HWW next-block height: {old.hww.earliest_next_block_height}</Text>
              <Text style={s.tiny}>{btc(old.phone.mature_sats)} BTC eligible now</Text>
            </>}
            {!old.can_recover && <Text style={s.tiny}>The previous phone key is required. This phone can only monitor the address.</Text>}
            <Button title="Review old vault funds" disabled={busy || !old.can_recover || old.phone.blocks_remaining === null} onPress={() => reviewPrevious(old.vault_id)}/>
          </View>)}
        </>}
        {page === 'contact-approve' && contactRestoreReview && <><Text style={s.heading}>Approve contact recovery</Text>
          <Text style={s.tiny}>Replacement phone identity · compare independently</Text><Text selectable style={s.mono}>{contactRestoreReview.recipient_public_key}</Text>
          <Text style={s.tiny}>Your contact fingerprint</Text><Text selectable style={s.mono}>{contactRestoreReview.contact_fingerprint}</Text>
          <Text style={s.tiny}>Backup commitment</Text><Text selectable style={s.mono}>{contactRestoreReview.backup_id}</Text>
          <Text style={s.subtext}>Release the backup decryption key only to the verified replacement phone. This contact cannot inspect the absent cloud payload. Recovery does not unlock the vault early.</Text>
          <Button title="Approve contact recovery" primary disabled={busy} onPress={() => run({ method: 'approve_contact_restore', review_id: contactRestoreReview.review_id, request: reviewMessage }, value => { void send('contact-restore-approved', value); setContactRestoreReview(null); setReviewMessage(null); })} />
          <Button title="Reject contact recovery" danger disabled={busy} onPress={() => run({ method: 'reject_contact_restore', review_id: contactRestoreReview.review_id }, () => { setContactRestoreReview(null); setReviewMessage(null); })} />
        </>}
        {page === 'contact-recover' && <><Text style={s.heading}>Recover with your contact</Text>
          <Text style={s.subtext}>Use a fresh phone, your encrypted cloud backup, and an enrolled contact. Neither original vault device is needed.</Text>
          <Text style={s.tiny}>This phone’s identity · compare on the contact’s screen</Text><Text selectable testID="contact-recipient-identity" style={s.mono}>{identity.vault_pubkey}</Text>
          {receivedContact && <><Text style={s.rowValue}>{receivedContact.name}</Text><Text selectable style={s.mono}>{receivedContact.fingerprint}</Text></>}
          {!contactRecovery && <><Text style={s.subtext}>Have your contact share their public contact key here, then load or paste the encrypted backup.</Text>
            <TextInput accessibilityLabel="Social recovery cloud backup JSON" multiline style={[s.input, { minHeight: 90 }]} value={backupText} onChangeText={setBackupText} autoCapitalize="none" autoCorrect={false} />
            <Button title="Request contact recovery" primary disabled={busy || !receivedContact || !backupText} onPress={requestContactRestore} />
          </>}
          <Button title="Check stored contact recovery" disabled={busy} onPress={() => run({ method: 'contact_recovery_state' }, setContactRecovery)} />
          {contactRecovery && <><Text style={s.rowValue}>Recovered vault</Text><Text selectable style={s.mono}>{contactRecovery.config.vault_address}</Text>
            <Text style={s.subtext}>Your old ordinary wallet has no vault recovery delay. Review its balance separately to move it to this phone’s fresh wallet.</Text>
            <Button title="Review recovered wallet funds" disabled={busy} onPress={reviewRecovered} />
            {!contactRecovery.spending_policy_available && <Text style={s.subtext}>Spending policy unavailable: this backup contains the key and descriptor, not the old allowance history. This recovery transfers funds; it does not renew a policy.</Text>}
            <Button title="Check recovered vault balance and delay" disabled={busy} onPress={() => run({ method: 'contact_recovery_status' }, setContactStatus)} />
            {contactStatus && <><Text style={s.rowValue}>{btc(contactStatus.balance_sats)} BTC recoverable</Text><Text style={s.subtext}>{btc(contactStatus.mature_balance_sats)} BTC mature now. Later deposits can be recovered separately.</Text><Text testID="contact-recovery-blocks" style={s.subtext}>{contactStatus.blocks_remaining ?? 'No confirmed funds;'} blocks remaining</Text></>}
            <Field label="Social recovery destination" value={destination} onChange={setDestination} />
            <Button title="Use new wallet address" disabled={busy} onPress={() => run({ method: 'receive_address' }, value => setDestination(value.address))} />
            <Button title="Review recovered vault transfer" primary disabled={busy || !destination} onPress={() => run({ method: 'review_contact_sweep', destination }, setContactSweep)} />
            {contactSweep && <><Text style={s.heading}>Review social recovery transfer</Text><Text style={s.rowValue}>{contactSweep.amount_sats} sats</Text><Text selectable style={s.mono}>{contactSweep.destination}</Text>
              <Text style={s.subtext}>{contactSweep.input_count} inputs · {contactSweep.fee_sats} sats fee. {contactSweep.blocks_remaining} blocks remaining at review. Confirmation checks maturity again.</Text>
              <Button title="Confirm recovered vault transfer" primary disabled={busy} onPress={() => run({ method: 'confirm_contact_sweep', review_id: contactSweep.review_id }, () => { setContactSweep(null); setContactStatus(null); })} />
              <Button title="Cancel recovered vault transfer" danger disabled={busy} onPress={() => run({ method: 'reject_contact_sweep', review_id: contactSweep.review_id }, () => setContactSweep(null))} />
            </>}
          </>}
        </>}
        {page === 'add-contact' && receivedContact && <><Text style={s.heading}>Add a recovery contact</Text>
          <Text style={s.rowValue}>{receivedContact.name}</Text><Text selectable style={s.mono}>{receivedContact.fingerprint}</Text>
          <Text style={s.subtext}>Compare this fingerprint with your contact independently. Your signer must approve the updated encrypted backup. Any one contact can recover the phone key with the cloud backup; Bitcoin still enforces the recovery delay.</Text>
          <Text style={s.tiny}>Check your contact fingerprints when rotating and export the new backup afterwards. Do not rely on this prototype as your only recovery route.</Text>
          <Button title="Propose recovery contact" primary disabled={busy || !paired} onPress={() => propose({ method: 'propose_backup_with_contact', contact: receivedContact }, 'backup-proposal', () => { setReceivedContact(null); setPage('home'); })} />
          <Button title="Cancel adding contact" onPress={() => { setReceivedContact(null); setPage('home'); }} />
        </>}
        {page === 'replace-signer' && <><Text style={s.heading}>Replace the lost signer</Text>
          <Text style={s.subtext}>{tap ? 'Hold a new signer to your phone.' : 'Create a fresh device in hardware wallet mode and send its public identity here.'} Your phone key stays the same. Bitcoin still requires the phone recovery delay before moving old vault funds.</Text>
          {tap && <Button title={signerCandidate ? 'Connect a different signer' : 'Connect new signer'} primary={!signerCandidate} disabled={busy} onPress={setupVault} />}
          {signerCandidate && <>
            <Text style={s.tiny}>Phone identity · compare on the new signer</Text><Text selectable style={s.mono}>{identity.vault_pubkey}</Text>
            <Text style={s.tiny}>New vault · compare on the new signer</Text><Text selectable style={s.mono}>{signerCandidate.new_config.vault_address}</Text>
            <Text testID="signer-replacement-blocks" style={s.subtext}>{signerCandidate.recovery_blocks_remaining ?? 'No confirmed vault inputs;'} blocks remaining</Text>
            <Text testID="signer-replacement-selection" style={s.subtext}>{btc(signerCandidate.selected_sats)} of {btc(signerCandidate.total_sats)} BTC in this replacement group</Text>
            <Text style={s.tiny}>Up to 1,000 mature inputs, or the oldest group before maturity. New deposits cannot delay older savings. Funds left behind stay visible under Retired vaults after replacement.</Text>
            <Button title="Refresh replacement readiness" disabled={busy} onPress={() => run({ method: 'inspect_signer_replacement', signer: signerCandidate.signer }, setSignerCandidate)} />
            <Button title="Propose signer replacement" primary disabled={busy} onPress={() => propose({ method: 'propose_signer_replacement', signer: signerCandidate.signer }, 'signer-replacement-proposal')} />
            <Text style={s.tiny}>You can prepare and sign before maturity. An early activation is rejected without broadcasting, and the same approved request can be retried after mining.</Text>
          </>}
          <Button title="Close signer replacement" onPress={() => { setPage('home'); setSignerCandidate(null); }} />
        </>}
        {previousRecovery && <><Text style={s.heading}>Recover retired vault funds</Text>
          <Text style={s.tiny}>From the old vault</Text><Text selectable style={s.mono}>{previousRecovery.source_address}</Text>
          <Text style={s.rowValue}>{btc(previousRecovery.amount_sats)} BTC to your current vault</Text>
          <Text selectable style={s.mono}>{previousRecovery.destination}</Text>
          <Text style={s.subtext}>Fee: {previousRecovery.fee_sats} sats · {previousRecovery.input_count} inputs · {previousRecovery.blocks_remaining} blocks remaining</Text>
          <Text style={s.tiny}>Review the destination. This moves only the selected group; it does not change your monthly or emergency limits. Sync the vault after confirmation to check for leftovers.</Text>
          <Button title="Confirm previous vault transfer" primary disabled={busy} onPress={() => run({ method: 'confirm_previous_vault_recovery', review_id: previousRecovery.review_id }, () => setPreviousRecovery(null))} />
          <Button title="Cancel previous vault transfer" disabled={busy} onPress={() => run({ method: 'reject_previous_vault_recovery', review_id: previousRecovery.review_id }, () => setPreviousRecovery(null))} />
        </>}
        {recoveredPayment && <><Text style={s.heading}>Recover the old wallet balance</Text>
          <Text style={s.subtext}>{recoveredPayment.amount_sats} sats to this phone’s fresh wallet address. Fee: {recoveredPayment.fee_sats} sats.</Text>
          <Text selectable style={s.mono}>{recoveredPayment.destination}</Text>
          <Text style={s.tiny}>Only spendable coins found by the wallet scan are included. Immature coinbase outputs need more confirmations. You can scan again for later deposits.</Text>
          <Button title="Confirm recovered wallet transfer" primary disabled={busy} onPress={() => run({ method: 'confirm_recovered_wallet', review_id: recoveredPayment.review_id }, () => setRecoveredPayment(null))} />
          <Button title="Cancel recovered wallet transfer" disabled={busy} onPress={() => run({ method: 'reject_recovered_wallet', review_id: recoveredPayment.review_id }, () => setRecoveredPayment(null))} />
        </>}
        {!vaultVisible && page === 'home' && paired && backupStored && <MenuRow title="Signer recovery and revocation" icon="key" disabled={busy} onPress={() => run({ method: 'recovery_source' }, value => { setRecoverySource(value); go('signer-recovery'); })}/>}
        {page === 'signer-recovery' && recoverySource && <>
          <Text style={s.heading}>Signer recovery and revocation</Text>
          <Text style={s.subtext}>Public recovery source. No original phone key is needed to prepare these requests. The separate signer must approve them.</Text>
          <Text selectable style={s.mono}>{recoverySource.vault_address}</Text>
          {tap && <Button title="Load vault from signer" disabled={busy} onPress={() => void toSigner(async () => ({ kind: 'recovery-vault-request', body: {} }))} />}
          <Text style={s.subtext}>Last approved policy: {btc(recoverySource.monthly_limit_sats)} BTC monthly · {btc(recoverySource.emergency_access_limit_sats)} BTC emergency. The offline signer cannot confirm whether that approval was activated on-chain.</Text>
          <Button title="Check recovery timing" disabled={busy} onPress={() => run({ method: 'recovery_snapshot' }, setRecoveryTiming)} />
          {recoveryTiming && recoveryTiming.vault_address === recoverySource.vault_address && <>
            <Text style={s.rowValue}>{btc(recoveryTiming.hww.total_sats)} BTC in the recovery vault</Text>
            <Text style={s.tiny}>As of block {recoveryTiming.chain_height}. Timelocks do not replace a missing key.</Text>
            {recoveryTiming.utxo_count === 0 ? <Text style={s.subtext}>No confirmed vault outputs remain.</Text> : <>
              <Text style={s.label}>Phone recovery</Text><Text testID="source-phone-recovery-blocks" style={s.subtext}>{recoveryTiming.phone.blocks_remaining} blocks remaining</Text>
              <Text style={s.tiny}>{btc(recoveryTiming.phone.mature_sats)} BTC eligible by the phone timelock</Text>
              <Text style={s.label}>HWW recovery</Text><Text testID="source-hww-recovery-blocks" style={s.subtext}>{recoveryTiming.hww.blocks_remaining} blocks remaining</Text>
              <Text style={s.tiny}>{btc(recoveryTiming.hww.mature_sats)} BTC eligible by the HWW timelock</Text>
            </>}
          </>}
          <Text style={s.heading}>Restore from cloud backup</Text>
          <Text style={s.tiny}>This phone’s identity · verify on the signer</Text><Text selectable style={s.mono}>{identity.vault_pubkey}</Text>
          <Text style={s.label}>Encrypted cloud backup JSON</Text>
          <TextInput accessibilityLabel="Encrypted cloud backup JSON" multiline value={backupText} onChangeText={setBackupText} maxLength={8 * 1024 * 1024} autoCapitalize="none" autoCorrect={false} style={[s.input, { height: 90 }]} />
          {!backupText && <Text style={s.subtext}>No cloud backup loaded. HWW fallback does not need a backup, but it still requires the surviving signer and the Bitcoin recovery delay.</Text>}
          <Button title="Request phone key recovery" disabled={busy || !backupText} onPress={requestRestore} />
          <Button title="Check restored key" disabled={busy} onPress={() => run({ method: 'restored_phone' }, setRestored)} />
          {restored && <><Text style={s.noticeText}>Old key recovered: {restored.old_phone_public_key}. Rotation is still required; recovering a backup does not move funds.</Text>
            <Button title="Propose restored vault rotation" primary disabled={busy} onPress={() => propose({ method: 'propose_restored_rotation' }, 'phone-rotation-proposal')} />
            <Button title="Review recovered wallet funds" disabled={busy} onPress={reviewRecovered} />
          </>}
          <Field label="Recovery destination" value={destination} onChange={setDestination} />
          <Button title="Use this wallet’s fresh address" disabled={busy} onPress={() => run({ method: 'receive_address' }, value => setDestination(value.address))} />
          <Button title="Request all-action revocation" danger disabled={busy} onPress={() => propose({ method: 'propose_offline_action', action: 'revoke_actions', destination }, 'offline-action-proposal')} />
          <Button title="Request HWW fallback" disabled={busy} onPress={() => propose({ method: 'propose_offline_action', action: 'hww_recovery', destination }, 'offline-action-proposal')} />
          <Text style={s.tiny}>Fallback keeps the 65,535-block ({network === 'signet' ? '~23 days on Mutinynet' : '~15 months'}) delay. A request includes up to 1,000 mature outputs, or the oldest group if none is mature. Newer deposits can be recovered later. It transfers funds only; it does not rotate keys or renew the annual policy.</Text>
          <Button title="Close signer recovery" onPress={() => { setRecoverySource(null); go('home'); }} />
        </>}
      </>}
      {outgoing && (inPrivate || outgoing.kind === 'signer-identity' || outgoing.kind === 'vault-setup' || outgoing.kind === 'recovery-contact') && <View style={s.notice}>
        <Text style={s.noticeText}>{!signer && tap && deferred.includes(outgoing.kind) ? 'Once it confirms, finish on your signer' : `Ready to share · ${outgoing.kind.replaceAll('-', ' ')}`}</Text>
        <Button title={outgoing.kind === 'recovery-contact' ? 'Send contact public key' : outgoing.kind === 'contact-restore-proposal' ? 'Send to contact' : outgoing.kind === 'contact-restore-approved' ? 'Send recovery approval' : signer ? 'Send to wallet'
          : tap && signerBound.includes(outgoing.kind) ? (deferred.includes(outgoing.kind) ? 'Finish on signer' : 'Continue on signer') : 'Send to signer'} disabled={busy} onPress={shareOutgoing} />
      </View>}
      {identity && inPrivate && (signer || paired) && <SavedMessages transport={transport} deliver={deliver} />}
      {!anySheet && feedback}
    </ScrollView>
    {!signer && <View style={s.tabs}><BlurView pointerEvents="none" tint="dark" intensity={48} style={[StyleSheet.absoluteFill, { borderRadius: 30 }]}/>{(['wallet', 'vault'] as const).map(value => <Pressable key={value} accessibilityRole="tab" accessibilityLabel={value === 'wallet' ? 'Wallet' : 'Vault'} aria-selected={tab === value}
      onPress={() => { screenScroll.current?.scrollTo({ y: 0, animated: false }); if (value === 'wallet') hideVault(); else { setTab('vault'); setPage('home'); setError(''); setNotice(''); } }} style={[s.tab, tab === value && s.selectedTab]}>
      <Icon name={value} size={16} color={tab === value ? colors.text : colors.faint}/><Text style={[s.tabText, tab === value && s.selectedText]}>{value === 'wallet' ? 'Wallet' : 'Vault'}</Text>
    </Pressable>)}</View>}
    {Platform.OS === 'web' && <View pointerEvents="none" style={{ position: 'absolute', bottom: 9, left: '50%', marginLeft: -67, width: 134, height: 5, borderRadius: 3, backgroundColor: '#ffffff3d' }}/>}
    {walletSheet && <DarkSheet label={page === 'receive' ? 'Receive bitcoin' : page === 'transaction' ? 'Transaction details' : 'Send bitcoin'} onClose={() => setPage('home')}>
      {page === 'receive' && <><Text style={s.sheetEyebrow}>Receive</Text><AddressCard address={address}/></>}
      {page === 'send' && <><Text style={s.sheetEyebrow}>Send</Text>
        <TextInput accessibilityLabel="Amount in sats" value={payment} onChangeText={setPayment} keyboardType="number-pad" placeholder="0" placeholderTextColor={colors.quiet}
          style={{ marginTop: 26, textAlign: 'center', fontFamily: fonts.number, fontSize: 38, fontWeight: '500', fontVariant: ['tabular-nums'], color: Number(payment) > 0 ? colors.text : colors.quiet }}/>
        <Text style={{ marginTop: 8, textAlign: 'center', fontFamily: fonts.number, fontSize: 12, color: colors.faint }}>sats · ₿{btc(Number(payment) || 0)}</Text>
        <View style={{ marginTop: 24, marginHorizontal: 6, flexDirection: 'row', alignItems: 'center', gap: 10, paddingHorizontal: 18, borderRadius: 26, backgroundColor: 'rgba(255,255,255,.07)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.12), 0 0 0 .5px rgba(255,255,255,.05)' }}>
          <Icon name="scan" size={15} color="rgba(255,255,255,.3)"/>
          <TextInput accessibilityLabel="Destination address" value={destination} onChangeText={setDestination} placeholder="Address · paste or scan" placeholderTextColor={colors.quiet} autoCapitalize="none" autoCorrect={false}
            style={{ flex: 1, paddingVertical: 15, color: colors.text, fontSize: 12.5, fontFamily: destination ? fonts.mono : undefined }}/>
        </View>
        <View style={{ alignItems: 'center', marginTop: 16 }}>
          <Pressable accessibilityRole="button" accessibilityLabel="Review payment" disabled={busy} onPress={() => run({ method: 'review_payment', destination, amount_sats: Number(payment) }, value => { setPaymentReview(value); setPage('confirm-send'); })}
            style={({ pressed }) => ({ height: 40, paddingHorizontal: 22, borderRadius: 20, flexDirection: 'row', alignItems: 'center', gap: 7, backgroundColor: 'rgba(255,255,255,.08)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.14), 0 0 0 .5px rgba(255,255,255,.06)', opacity: busy ? .4 : 1, transform: [{ scale: pressed ? .95 : 1 }] })}>
            <Icon name="send" size={13} strokeWidth={2.2} color="rgba(255,255,255,.64)"/><Text style={{ color: colors.text, fontSize: 12.5, fontWeight: '500' }}>Review</Text>
          </Pressable>
        </View></>}
      {page === 'confirm-send' && paymentReview && <><Text style={s.sheetEyebrow}>Confirm payment</Text><Text style={s.rowValue}>{paymentReview.amount_sats} sats</Text><Text selectable style={s.mono}>{paymentReview.destination}</Text><Text style={s.subtext}>Fee: {paymentReview.fee_sats} sats · Total: {paymentReview.total_sats} sats</Text>
        <Button title="Confirm and send" primary disabled={busy} onPress={() => run({ method: 'confirm_payment', review_id: paymentReview.review_id }, () => setPage('home'))} /><Button title="Go back" onPress={() => run({ method: 'reject_payment', review_id: paymentReview.review_id }, () => { setPaymentReview(null); setPage('send'); })} disabled={busy} /></>}
      {page === 'transaction' && selectedActivity && <><Text style={s.sheetEyebrow}>Transaction details</Text>
        <Text style={[s.balance, { fontSize: 30, textAlign: 'center' }]}>{selectedActivity.balance_change_sats < 0 ? '−' : '+'}₿{btc(Math.abs(selectedActivity.balance_change_sats))}</Text>
        <Text style={s.subtext}>{selectedActivity.confirmed ? 'Confirmed' : 'Awaiting confirmation'} · wallet balance change{selectedActivity.balance_change_sats < 0 ? ', including fees' : ''}</Text>
        <Text selectable style={s.mono}>{selectedActivity.txid}</Text><Button title="Back to wallet" onPress={() => setPage('home')}/>
      </>}
      {feedback}
    </DarkSheet>}
    {vaultSheet && vault && <DarkSheet label={vaultSheetLabels[page]} onClose={() => go('home')}>
      <Text style={s.sheetEyebrow}>{vaultSheetLabels[page]}</Text>
      {page === 'deposit' && <AddressCard address={vault.config.vault_address} vault/>}
      {page === 'withdraw' && <WithdrawalOptions go={go} vault={vault}/>}
      {page === 'settings' && <VaultSettings vault={vault} disabled={busy}
        go={next => next === 'signer-recovery' ? void run({ method: 'recovery_source' }, value => { setRecoverySource(value); go(next); }) : go(next)}
        backup={() => propose({ method: 'propose_backup' }, 'backup-proposal', () => setPage('home'))}
        exportBackup={exportBackup} reviewRecovered={() => { go('recovered-wallet'); void reviewRecovered(); }}/>
      }
      {page === 'policy' && <PolicyEditor vault={vault} monthly={monthly} setMonthly={setMonthly} emergency={emergency} setEmergency={setEmergency}
        disabled={busy} onPropose={() => proposal()} onRenew={() => proposal('refresh_policy')}/>}
      {page === 'monthly' && <>
        <Text style={[s.balance, { fontSize: 34, textAlign: 'center', marginTop: 12 }]}>₿{btc(vault.config.monthly_limit_sats)}</Text>
        <Text style={[s.subtext, { textAlign: 'center' }]}>From your vault to your everyday wallet.</Text>
        <Field label="Allowance step" value={step} onChange={setStep} numeric/>
        <Button title="Withdraw monthly allowance" primary disabled={busy} onPress={() => run({ method: 'monthly', step: Number(step), action: 'authorization' })}/>
        <Button title="Revoke remaining allowances" danger disabled={busy} onPress={() => run({ method: 'monthly', step: Number(step), action: 'revocation' })}/>
        <Text style={s.tiny}>Revocation disables this step and every later allowance once confirmed.</Text>
        <Field label="Soft limit in sats" value={softLimit} onChange={setSoftLimit} numeric/>
        <Button title="Apply soft limit" disabled={busy} onPress={() => run({ method: 'apply_soft_limit', step: Number(step), limit_sats: Number(softLimit) })}/>
        <Text style={s.tiny}>After withdrawal, return funds above your soft limit to the vault, less transaction fees.</Text>
      </>}
      {page === 'emergency' && <>
        <Text style={[s.balance, { fontSize: 34, textAlign: 'center', marginTop: 12 }]}>₿{btc(vault.config.emergency_access_limit_sats)}</Text>
        <Text style={[s.subtext, { textAlign: 'center' }]}>One-week delay after activation confirms. Cancel before withdrawal confirms to keep the funds in cold storage.</Text>
        <Button title="Start emergency access" primary disabled={busy} onPress={() => run({ method: 'initiate_emergency' })}/>
        <Button title="Withdraw emergency funds" disabled={busy} onPress={() => run({ method: 'withdraw_emergency' })}/>
        <Button title="Cancel emergency access" danger disabled={busy} onPress={() => run({ method: 'cancel_emergency' })}/>
      </>}
      {(page === 'sweep' || page === 'recovery') && <>
        <Field label="Vault transfer destination" value={destination} onChange={setDestination}/>
        {page === 'sweep' ? <>
          <Text style={s.subtext}>Both devices approve an immediate transfer of all confirmed vault outputs. This does not renew the monthly policy.</Text>
          <Button title="Propose vault transfer" primary disabled={busy} onPress={() => propose({ method: 'propose_sweep', destination }, 'sweep-proposal', () => setPage('home'))}/>
        </> : <>
          <Text style={s.subtext}>Phone recovery requires {vault.config.phone_recovery_blocks} blocks ({network === 'signet' ? '~21 days on Mutinynet' : '~14 months'}) since confirmation. Earliest next-block height: {vault.phone_recovery_next_block_height ?? 'No confirmed funds'}. Verify the destination before continuing.</Text>
          <Button title="Confirm phone recovery transfer" danger disabled={busy} onPress={() => run({ method: 'phone_recovery', destination }, () => setPage('home'))}/>
        </>}
        <Button title="Cancel vault transfer" onPress={() => go('home')}/>
      </>}
      {feedback}
    </DarkSheet>}
    {tapState && <TapSheet state={tapState} onCancel={cancelTap} onRetry={() => void connectAndSend()} onDone={() => setTapState(null)}/>}
  </View>;
}
