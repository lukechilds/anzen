import React, { useEffect, useRef, useState } from 'react';
import SavedMessages from './SavedMessages';
import { parseEncryptedBackup } from './fileExchange';
import { ActivityIndicator, AppState, Platform, Pressable, RefreshControl, ScrollView, StyleSheet, View } from 'react-native';
import { useInsets } from './ui/insets';
import { Text, TextInput } from './ui/Text';
import { Sheet, SheetHost } from './ui/Sheet';
import { TAB_BAR_CLEARANCE, TabBar } from './ui/Tabs';
import { PasteButton } from './ui/PasteButton';
import { haptics } from './ui/haptics';
import { authenticationName } from './ui/authenticate';
import Svg, { Path } from 'react-native-svg';
import { DarkBackground, DarkEffects, DeviceStatus, Icon, TopFade } from './DarkChrome';
import { ActionPill, Activity, IconButton, MenuRow, WalletHistory } from './DarkSurfaces';
import AddressCard, { AddressLines } from './AddressCard';
import VaultSummary from './VaultSummary';
import Descriptors from './Descriptors';
import RecoveryPhrase from './RecoveryPhrase';
import ForgetVault from './ForgetVault';
import Benchmarks, { BenchmarkRunner } from './Benchmarks';
import { VaultSettings, WithdrawalOptions } from './VaultControls';
import { PolicyEditor } from './VaultControls';
import { DECLINED, TapArt, TapPurpose, TapSheet, TapState } from './Tap';
import SignerDevice from './SignerDevice';
import { useSimulatedFaceId } from './FaceId';
import { Code, Note, Page, Pager, Review } from './SignerScreens';
import { colors, fonts, formatBtc, styles as s } from './theme';
import { Deliver, DeviceTransport, JsonObject, ProtocolMessage, SignerLink, Tap, TransferResult, signerBound } from './transport';

type Props = {
  network?: 'regtest' | 'bitcoin' | 'signet';
  transport: DeviceTransport;
  incoming: ProtocolMessage | null;
  dismissIncoming: () => void;
  deliver: Deliver;
  // The Bluetooth link to the signer. Without it, messages travel as files.
  tap?: Tap;
  // The signer cannot protect the phone's recovery backup (a Ledger or a Trezor, for now), so a vault is
  // complete without one. Losing the phone then leaves only the signer's own delayed recovery.
  signerKeepsNoBackup?: boolean;
  saveBackup?: (backup: JsonObject) => Promise<TransferResult>;
  cloudBackup?: JsonObject | null;
  // Asks the owner to confirm it's them (Face ID or passcode) before the app opens, the vault
  // opens or money moves. Without it, the browser emulator plays a simulated Face ID check.
  authenticate?: (cover?: boolean) => Promise<boolean>;
  // GPU vanity-search benchmarks, shown in the wallet's settings.
  benchmark?: BenchmarkRunner;
  // This device's own settings (network, hardware wallet mode), from the native app: in the
  // wallet's settings, and behind a link on the signer.
  deviceSettings?: React.ReactNode;
  // Sheets of the surrounding app's own, presented with this one's.
  sheets?: React.ReactNode;
};
type Kind = ProtocolMessage['kind'];
type ReviewKind = 'policy' | 'sweep' | 'backup' | 'offline-action' | 'restore' | 'rotation' | 'rotation-adoption' | 'share-vault';
// These finish only after the phone's transaction confirms, so the phone offers them later.
const deferred: Kind[] = ['phone-rotation-completed'];
const purposeOf = (kind: Kind): TapPurpose => kind === 'signer-hello' ? 'setup'
  : kind === 'recovery-vault-request' ? 'recover' : deferred.includes(kind) ? 'finish' : 'approve';
const message = (error: unknown) => error instanceof Error ? error.message : String(error);
// How long the app can be away before its owner is asked for again.
const RELOCK_AFTER_MS = 30_000;
const btc = (sats: number = 0) => formatBtc(sats);
// The signer's round display is small: the same review layout, set tighter.
const r = { ...s, ...StyleSheet.create({
  heading: { ...s.heading, fontSize: 18, lineHeight: 23, marginTop: 10, marginBottom: 6 },
  row: { ...s.row, paddingVertical: 11 },
  rowValue: { ...s.rowValue, fontSize: 18, marginTop: 3 },
  subtext: { ...s.subtext, fontSize: 11.5, lineHeight: 17, marginVertical: 8 },
  tiny: { ...s.tiny, fontSize: 10.5, lineHeight: 15 },
  mono: { ...s.mono, fontSize: 10, lineHeight: 15 },
  errorText: { ...s.errorText, fontSize: 11, lineHeight: 16 },
}) };

// The signer's idle mark: radio waves, mint while a phone is connected.
function Waves({ on }: { on: boolean }) {
  const tint = on ? colors.mint : 'rgba(255,255,255,.55)';
  return <Svg width={34} height={34} viewBox="0 0 24 24" fill="none" stroke={tint} strokeWidth={1.6} strokeLinecap="round" accessibilityElementsHidden>
    <Path d="M8.5 9a4.5 4.5 0 0 1 0 6M12 6.5a8 8 0 0 1 0 11M15.5 4a11.5 11.5 0 0 1 0 16"/><Path d="M5 12h.01" strokeWidth={2.6}/>
  </Svg>;
}

function Balance({ sats }: { sats?: number }) {
  const [width, setWidth] = useState(0);
  const text = sats === undefined ? '₿—' : `₿${btc(sats)}`;
  const digits = text.slice(1);
  // React Native Web does not implement native Text's adjustsFontSizeToFit. Size tabular
  // digits conservatively on both platforms so a long amount is never clipped/ellipsized.
  const fontSize = width ? Math.min(46, width / (text.length * .66)) : 46;
  return <View style={{ width: '100%', alignItems: 'center' }} onLayout={event => setWidth(event.nativeEvent.layout.width)}>
    {/* A little space after the ₿; the text content stays "₿0.1000". */}
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
    style={s.input} keyboardType={numeric ? 'number-pad' : 'default'} autoCapitalize="none" autoCorrect={false} spellCheck={false} autoComplete="off"
    returnKeyType="done" clearButtonMode="while-editing" /></>;
}

export default function AnzenApp({ transport, incoming, dismissIncoming, deliver, tap, saveBackup, cloudBackup, authenticate: deviceAuthenticate, network = 'regtest', benchmark, deviceSettings, sheets: hostSheets, signerKeepsNoBackup = false }: Props) {
  const signer = transport.role === 'signer';
  const [identity, setIdentity] = useState<JsonObject | null>(null);
  const [sessionLoaded, setSessionLoaded] = useState(false);
  const [tab, setTab] = useState<'wallet' | 'vault'>('wallet');
  const [page, setPage] = useState('home');
  const [vaultVisible, setVaultVisible] = useState(false);
  const [revealError, setRevealError] = useState('');
  const faceId = useSimulatedFaceId();
  const authenticate = deviceAuthenticate ?? (Platform.OS === 'web' ? faceId.authenticate : undefined);
  // An existing wallet stays covered until its owner is confirmed.
  const [locked, setLocked] = useState(!signer && Boolean(authenticate));
  const [unlockFailed, setUnlockFailed] = useState(false);
  // On a device the wallet is covered while the app is put away, so the app switcher's picture of
  // it shows nothing, and it locks again once it has been away for a while.
  const [away, setAway] = useState(false);
  const leftAt = useRef(0);
  // The device's own check takes the screen for a moment, which the app sees as being put
  // away. While one is up, that is not a reason to hide the vault.
  const authenticating = useRef(false);
  const confirmOwner = async (cover?: boolean) => {
    if (!authenticate) return true;
    authenticating.current = true;
    try { return await authenticate(cover); }
    catch { return false; }
    // The app is told it is active again a moment after the check closes.
    finally { setTimeout(() => { authenticating.current = false; }, 600); }
  };
  const unlock = async () => {
    setUnlockFailed(false);
    if (await confirmOwner(true)) setLocked(false); else setUnlockFailed(true);
  };
  // Moving money needs the owner too.
  const secure = async (action: () => unknown) => { if (!await confirmOwner()) return; await action(); };
  const opening = useRef(false);
  const [wallet, setWallet] = useState<JsonObject | null>(null);
  const [selectedActivity, setSelectedActivity] = useState<Activity | null>(null);
  const [vault, setVault] = useState<JsonObject | null>(null);
  const [pairing, setPairing] = useState<JsonObject | null>(null);
  const [paired, setPaired] = useState(false);
  // Bumped to read the session again, after the device's state changed under the app.
  const [sessionEpoch, setSessionEpoch] = useState(0);
  const [backupStored, setBackupStored] = useState(false);
  // A vault is ready once it has its signer and, where the signer can protect one, its backup.
  const ready = paired && (backupStored || signerKeepsNoBackup);
  const [tapState, setTapState] = useState<TapState | null>(null);
  const [linked, setLinked] = useState(false);
  const [signerSettings, setSignerSettings] = useState(false);
  // A new phone gets its wallet straight away; this explains, once, how it is protected.
  const [welcome, setWelcome] = useState(false);
  const autoCreated = useRef(false);
  // A signer's approval the phone could not finish with yet, e.g. before a timelock matures.
  const [retained, setRetained] = useState<ProtocolMessage | null>(null);
  const [review, setReview] = useState<JsonObject | null>(null);
  const [reviewKind, setReviewKind] = useState<ReviewKind>('policy');
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
  const [softLimit, setSoftLimit] = useState('1000000');
  const [moreMonthly, setMoreMonthly] = useState(false);
  const [busy, setBusy] = useState(false);
  const [liveError, setLiveError] = useState<{ wallet?: string; vault?: string }>({});
  const [error, setError] = useState('');
  const [notice, setNotice] = useState('');
  const privacyGeneration = useRef(0);
  const screenScroll = useRef<ScrollView>(null);
  const insets = useInsets();
  const web = Platform.OS === 'web';
  // Pulling the wallet or the open vault down syncs it.
  const [refreshing, setRefreshing] = useState(false);
  // Whether the screen has scrolled, so its top fades what passes under the status bar.
  const [scrolled, setScrolled] = useState(false);
  // What this device calls its owner check, for the app's words about it.
  const [ownerCheck, setOwnerCheck] = useState('Face ID');
  useEffect(() => { void authenticationName().then(setOwnerCheck).catch(() => {}); }, []);
  const destinationInput = useRef<React.ComponentRef<typeof TextInput>>(null);
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
    setPage(next); setMoreMonthly(false); setError(''); setNotice(''); screenScroll.current?.scrollTo({ y: 0, animated: false });
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
      if (!signer && session.identity && authenticate) void unlock();
      else setLocked(false);
      setPaired(Boolean(session.paired || session.config));
      setBackupStored(Boolean(session.backup_stored));
      if (signer && session.config) setPairing(session.config);
    }).catch(e => { if (active) setError(String(e)); }).finally(() => { if (active) setSessionLoaded(true); });
    return () => { active = false; };
  }, [transport, sessionEpoch]);

  // Presentation privacy only. Wallet operations/authorization are exclusively Rust-owned.
  const hideVault = () => { privacyGeneration.current += 1; setScrolled(false); setRevealError(''); setVaultVisible(false); setVault(null); setRecoveredPayment(null); setPage('home'); setTab('wallet'); setError(''); setNotice(''); };
  useEffect(() => {
    const subscription = AppState.addEventListener('change', state => {
      if (state !== 'active' && !authenticating.current) hideVault();
      if (signer || !deviceAuthenticate) return;
      if (state === 'active') {
        setAway(false);
        if (leftAt.current && Date.now() - leftAt.current > RELOCK_AFTER_MS) { setPage('home'); setLocked(true); void unlock(); }
        leftAt.current = 0;
      } else if (!authenticating.current) {
        setAway(true);
        // From the first moment away: the app may be suspended before it hears it is in the background.
        if (!leftAt.current) leftAt.current = Date.now();
      }
    });
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
        haptics.success();
        setNotice(`Sent · ${result.txid.slice(0, 8)}`);
        if (!signer && tab === 'wallet') {
          try { setWallet(await transport.execute({ method: 'wallet_snapshot' })); }
          catch (e) { if (stillVisible()) setError(`Transaction broadcast; balance refresh failed: ${String(e)}. Sync again to refresh the display.`); }
        }
      }
      return result;
    } catch (e) { haptics.failure(); if (stillVisible()) setError(e instanceof Error ? e.message : String(e)); }
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
    const purpose = purposeOf(request.kind);
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
        setTapState(purpose === 'setup' ? { purpose, phase: 'working', step: 'Connected to your signer' } : { purpose, phase: 'review' });
        tapReply.current = await connected.request(request);
      }
      await handleMessage(tapReply.current);
      link.current?.close(); link.current = null; tapMessage.current = null; tapReply.current = null;
      haptics.success();
      setTapState(purpose === 'setup' ? { purpose, phase: 'done' } : null);
      tapAfter.current?.();
    } catch (e) {
      link.current?.close(); link.current = null;
      if (!abort.signal.aborted) { haptics.failure(); setTapState({ purpose, phase: 'failed', error: message(e) }); }
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
    if (!signer) setWelcome(true);
  });
  // There is nothing to choose on a new phone, so its wallet is created without asking.
  useEffect(() => {
    if (signer || !sessionLoaded || identity || autoCreated.current) return;
    autoCreated.current = true;
    void initialize();
  }, [signer, sessionLoaded, identity]);
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
  // Opening the Vault tab opens the vault: savings stay hidden until then, and hide again on
  // leaving the tab or the app. The owner unlocks first where the device can ask.
  async function reveal() {
    if (opening.current) return;
    opening.current = true; setBusy(true); setRevealError('');
    const generation = privacyGeneration.current;
    try {
      if (!await confirmOwner()) throw new Error('The vault stays locked.');
      const value = await call({ method: 'vault_snapshot' });
      if (generation === privacyGeneration.current) { setVault(value); setVaultVisible(true); }
    } catch (e) { if (generation === privacyGeneration.current) setRevealError(message(e)); }
    finally { opening.current = false; setBusy(false); }
  }
  useEffect(() => {
    if (signer || tab !== 'vault' || page !== 'home' || !ready || vaultVisible || busy || tapState || revealError) return;
    void reveal();
  }, [signer, tab, page, ready, vaultVisible, busy, tapState, revealError]);
  const reviewRecovered = () => {
    const generation = privacyGeneration.current;
    return run({ method: 'review_recovered_wallet' }, value => {
      if (generation === privacyGeneration.current) setRecoveredPayment(value);
    });
  };
  function proposal(method = 'propose_policy') {
    const btcToSats = (text: string) => /^\d*\.?\d{0,8}$/.test(text.trim()) && text.trim() !== '.' ? Math.round(Number(text.trim() || '0') * 100_000_000) : NaN;
    const limits = method === 'propose_policy' ? { monthly_limit_sats: btcToSats(monthly), emergency_access_limit_sats: btcToSats(emergency) } : {};
    if (Object.values(limits).some(Number.isNaN)) { setError('Enter amounts in bitcoin, like 0.05.'); return; }
    void propose({ method, ...limits }, 'policy-proposal', () => setPage('home'));
  }
  function requestRestore() {
    let backup: JsonObject;
    try { backup = parseEncryptedBackup(backupText); }
    catch (e) { setError(`Cannot read the encrypted cloud backup: ${String(e)}`); return; }
    void propose({ method: 'propose_restore', backup }, 'restore-proposal');
  }

  // The recovery backup goes to the cloud whenever it changes; there is nothing to export.
  async function saveToCloud() {
    if (!saveBackup) return;
    try { await saveBackup(await call({ method: 'export_backup' })); }
    catch { setNotice('Saving the recovery backup failed. Change it again from Vault settings to retry.'); }
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
    const broadcast = (result: JsonObject) => { if (result.txid) setNotice(`Sent · ${result.txid.slice(0, 8)}`); };
    switch (received.kind) {
      // Signer: a phone asks who this is. A new signer creates its key on its first tap.
      case 'signer-hello': {
        // Setup needs a signer without a vault, or this phone's own when finishing an
        // interrupted setup. Refusing here keeps the phone from pairing with it.
        if (pairing && pairing.phone_vault_pubkey !== body.phone) throw new Error('This signer already protects another vault.');
        let own = identity;
        if (!own) { own = await call({ method: 'initialize', network }); setIdentity(own); }
        await send('signer-identity', own!); return;
      }
      case 'signer-identity':
        // A lost signer comes back from its own backup as the same key; a vault never changes signer.
        if (ready) throw new Error('This vault already has a signer.');
        progress('Creating your vault');
        { const setup = await call({ method: 'setup_vault', signer: body }); setPaired(true); await send('vault-setup', setup); }
        return;
      case 'vault-setup': {
        const accepted = await call({ method: 'accept_setup', config: body.config, backup: body.backup });
        setPairing(body.config); setPaired(true);
        await send('vault-accepted', accepted); return;
      }
      case 'vault-accepted':
        // A signer that keeps no backup has finished setup; one finishing a rotation only
        // confirms it switched vaults.
        if (!body.backup) { setNotice(signerKeepsNoBackup ? 'Vault set up. This signer keeps no recovery backup for your phone.' : 'Your signer now uses the new vault.'); return; }
        progress('Saving your recovery backup');
        await call({ method: 'store_backup', message: body.backup }); setBackupStored(true);
        await saveToCloud(); return;
      case 'recovery-vault-request':
        if (!pairing) throw new Error('This signer has no vault yet.');
        setReview({ vault_address: pairing.vault_address }); setReviewKind('share-vault'); return;
      case 'policy-proposal': await reviewWith('review_policy', 'policy', { request: body }); return;
      case 'sweep-proposal': await reviewWith('review_sweep', 'sweep', { request: body }); return;
      case 'sweep-approved': broadcast(await call({ method: 'activate_sweep', approval: body })); void refreshInBackground(); return;
      case 'recovery-vault': setRecoverySource(await call({ method: 'import_recovery_bundle', bundle: body })); showPrivate('signer-recovery'); return;
      case 'offline-action-proposal': await reviewWith('review_offline_action', 'offline-action', { request: body }); return;
      case 'offline-action-approved': broadcast(await call({ method: 'activate_offline_action', approval: body })); void refreshInBackground(); return;
      case 'restore-proposal': await reviewWith('review_restore', 'restore', { request: body }); return;
      case 'restore-approved':
        setRestored(await call({ method: 'store_restored_key', message: body })); showPrivate();
        setNotice('Old phone key recovered into encrypted storage. Your new wallet key is unchanged.'); return;
      case 'phone-rotation-proposal': await reviewWith('review_phone_rotation', 'rotation', { request: body }, true); return;
      case 'phone-rotation-approved': {
        const completed = await call({ method: 'activate_rotation', approval: body });
        await saveToCloud();
        setPaired(true); setBackupStored(true); setRestored(null); setRecoverySource(null); setVault(null); setVaultVisible(false); showPrivate('home');
        await send('phone-rotation-completed', completed);
        setNotice('Vault rotation broadcast. Once it confirms, finish on your signer.'); return;
      }
      case 'phone-rotation-completed': setReview(body); setReviewKind('rotation-adoption'); return;
      case 'backup-proposal': await reviewWith('review_backup', 'backup', { message: body }, true); return;
      case 'backup-approved': {
        const value = await call({ method: 'store_backup', message: body });
        setBackupStored(true); await saveToCloud();
        setVault(previous => previous ? { ...previous, recovery_backup_stored: value.stored } : previous);
        setNotice('Recovery backup saved.'); return;
      }
      case 'policy-approved':
        await call({ method: 'activate_policy', approval: body });
        setNotice('Allowance approved.'); void refreshInBackground(); return;
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
  // Show or import this device's recovery phrase, in settings. Import only before vault setup.
  // The emulator's signer has no Face ID; a phone in hardware wallet mode still asks for its own.
  const recoveryPhrase = <RecoveryPhrase signer={signer} network={network} hasKeys={Boolean(identity)} canImport={!paired && !backupStored}
    busy={busy} run={run} secure={signer && !deviceAuthenticate ? async action => { await action(); } : secure} onImported={value => {
      setIdentity(value); setNotice('Recovery phrase imported.');
      if (signer) { if (!tap) void send('signer-identity', value); }
      else { setWallet(null); setWelcome(false); }
    }}/>;
  // Forget this phone's vault, in settings: test networks only, and the wallet stays.
  const forgetVault = !signer && paired && network !== 'bitcoin' && <ForgetVault busy={busy} run={run} onForgotten={value => {
    setIdentity(value); setPaired(false); setBackupStored(false); setVault(null); setVaultVisible(false); setWallet(null);
    setRecoveredPayment(null); setNotice('Vault forgotten. Your wallet is unchanged.'); setSessionEpoch(epoch => epoch + 1);
  }}/>;
  const walletSheet = Boolean(identity && !signer && tab === 'wallet' && ['send', 'receive', 'confirm-send', 'transaction'].includes(page));
  const vaultSheetLabels: Record<string, string> = { lost: 'Lost a device', 'saved-messages': 'Saved messages', settings: 'Vault settings', policy: 'Vault policy', withdraw: 'Withdraw', monthly: 'Monthly allowance', emergency: 'Emergency access', deposit: 'Vault deposit', sweep: 'Move vault funds' };
  const vaultSheet = Boolean(identity && !signer && tab === 'vault' && vaultVisible && vaultSheetLabels[page]);
  const anySheet = walletSheet || vaultSheet || Boolean(tapState) || welcome;
  const feedback = <>
    {(busy || !sessionLoaded) && <ActivityIndicator color={colors.mint} style={{ margin: 20 }} />}
    {!!notice && <View style={s.notice}><Text selectable style={s.noticeText}>{notice}</Text></View>}
    {!!error && <View style={[s.notice, s.error]}><Text selectable accessibilityRole="alert" style={s.errorText}>{error}</Text></View>}
  </>;
  // The block the revealed vault reflects, as a data attribute: lets tests wait for live updates.
  // One set of screens: on a phone they scroll in the app; a signer shows them on its round display.
  const screenContent = <>
      {!identity ? <View style={[s.hero, { paddingTop: 260 }]}>
        {error ? <Button title="Try again" primary onPress={initialize} disabled={busy}/> : <Text style={s.tiny}>Creating your wallet…</Text>}
      </View> : <>
        {incoming && <View style={s.notice}><Text style={s.noticeText}>Incoming · {incoming.kind.replaceAll('-', ' ')}</Text><Button title="Open received message" onPress={importMessage} disabled={busy} /></View>}
        {retained && inPrivate && <View style={s.notice}><Text style={s.noticeText}>Approved on your signer · not finished yet</Text><Button title="Finish" onPress={() => void finishRetained()} disabled={busy} /></View>}
        {tab === 'wallet' ? <>
          {(page === 'home' || walletSheet) && <><View style={s.hero}><Text style={s.eyebrow}>Wallet</Text><Balance sats={wallet?.available_sats} /><Text style={s.balanceCaption}>Available bitcoin</Text>
            <SyncStatus error={liveError.wallet} behind={wallet?.behind_blocks} busy={busy} onSync={() => void syncWallet()}/>
            {wallet && wallet.pending_sats > 0 && <Text style={s.tiny}>{btc(wallet.pending_sats)} BTC awaiting confirmation</Text>}
            {wallet && wallet.immature_sats > 0 && <Text style={s.tiny}>{btc(wallet.immature_sats)} BTC in immature mining rewards</Text>}
            <View style={s.actions}><ActionPill title="Send" label="↑ Send" icon="send" onPress={() => { setError(''); setNotice(''); setPage('send'); }} /><ActionPill title="Receive" label="↓ Receive" icon="receive" onPress={() => run({ method: 'receive_address' }, value => { setAddress(value.address); setPage('receive'); })} disabled={busy}/></View></View>
            <WalletHistory items={wallet?.activity || []} loaded={Boolean(wallet)} onSelect={entry => { setSelectedActivity(entry); setPage('transaction'); }}/></>}
          {page === 'settings' && <><View style={s.hero}><Text style={s.eyebrow}>Settings</Text></View>
            {deviceSettings}
            {recoveryPhrase}
            {forgetVault}
            <View style={{ marginTop: 34 }}>
              <Text style={s.sheetEyebrow}>Watch-only</Text>
              <View style={{ marginTop: 12 }}><Descriptors kind="wallet" busy={busy} run={run}/></View>
            </View>
            {benchmark && <Benchmarks runner={benchmark}/>}
            <View style={{ marginTop: 48, marginHorizontal: 4 }}>
              <Text style={s.footnote}>anzen · {identity?.network || network}</Text>
            </View></>}
        </> : !vaultVisible ? page === 'home' && (ready ? <>
          <View style={[s.hero, { paddingBottom: 20 }]}><Text style={s.eyebrow}>Vault</Text>
            <Text style={[s.balance, { fontSize: 34 }]}>{revealError ? 'Still locked.' : 'Opening…'}</Text></View>
          {!!revealError && <><Text accessibilityRole="alert" style={[s.errorText, { textAlign: 'center' }]}>{revealError}</Text>
            <Button title="Try again" primary onPress={() => setRevealError('')} /></>}
        </> : <>
          {/* Setup is one tap: the signer's key, the vault and the recovery backup in one go. */}
          <View style={[s.hero, { paddingTop: 72 }]}><Text style={s.eyebrow}>Vault</Text>
            <TapArt phase="idle"/>
            <Text style={[s.balance, { fontSize: 32, lineHeight: 40, textAlign: 'center', marginTop: 30 }]}>{paired ? 'Almost there.' : 'Protect your\nsavings.'}</Text>
            <Text style={[s.balanceCaption, { lineHeight: 18, maxWidth: 270 }]}>{paired ? 'Connect your signer once more to finish.' : 'Your phone and a signer, together.'}</Text></View>
          {tap ? <View style={{ marginTop: 34 }}>
            <Button title={paired ? 'Finish setup' : 'Set up vault'} primary onPress={setupVault} disabled={busy} />
            {paired ? <Button title="Open vault" onPress={() => void reveal()} disabled={busy} />
              : <Pressable accessibilityRole="button" accessibilityLabel="Recover with your signer" disabled={busy} onPress={() => void toSigner(async () => ({ kind: 'recovery-vault-request', body: {} }))} hitSlop={8} style={({ pressed }) => ({ alignSelf: 'center', padding: 12, opacity: pressed ? .6 : 1 })}>
                <Text style={[s.tiny, { color: colors.quiet }]}>Recover with your signer</Text>
              </Pressable>}
          </View> : <>
            <Text style={[s.subtext, { textAlign: 'center' }]}>{paired ? 'Waiting for your signer to accept the vault.' : 'Receive your signer’s public key to set up your vault.'}</Text>
            {paired && <Button title="Open vault" primary onPress={() => void reveal()} disabled={busy} />}
          </>}
        </>) : (page === 'home' || vaultSheet) && <>
          <View style={s.hero}><Text style={s.eyebrow}>Vault</Text><Balance sats={vault?.balance_sats} /><Text style={s.balanceCaption}>Protected by two keys</Text>
          <SyncStatus error={liveError.vault} busy={busy} onSync={() => void syncVault()}/>
            <View style={s.actions}><ActionPill title="Withdraw" icon="send" onPress={() => go('withdraw')}/><ActionPill title="Deposit" icon="receive" onPress={() => go('deposit')}/></View>
          </View>
          {vault && <VaultSummary vault={vault} onMonthly={() => go('monthly')} onEmergency={() => go('emergency')} onPolicy={() => go('policy')}/>}
        </>}
      </>}
      {identity && !signer && tab === 'vault' && <>
        {page === 'recovered-wallet' && !recoveredPayment && <><Text style={s.heading}>Recovered wallet</Text><Text style={s.subtext}>Check the old phone’s ordinary wallet for funds you can move to this phone.</Text><Button title="Review recovered wallet funds" disabled={busy} onPress={reviewRecovered}/></>}
        {recoveredPayment && <><Text style={s.heading}>Recover the old wallet balance</Text>
          <Text style={s.subtext}>{recoveredPayment.amount_sats} sats to this phone’s fresh wallet address. Fee: {recoveredPayment.fee_sats} sats.</Text>
          <Text selectable style={s.mono}>{recoveredPayment.destination}</Text>
          <Button title="Confirm recovered wallet transfer" primary disabled={busy} onPress={() => void secure(() => run({ method: 'confirm_recovered_wallet', review_id: recoveredPayment.review_id }, () => setRecoveredPayment(null)))} />
          <Button title="Cancel recovered wallet transfer" disabled={busy} onPress={() => run({ method: 'reject_recovered_wallet', review_id: recoveredPayment.review_id }, () => setRecoveredPayment(null))} />
        </>}
        {page === 'signer-recovery' && recoverySource && <>
          <Text style={s.heading}>Signer recovery and revocation</Text>
          <Text selectable style={s.mono}>{recoverySource.vault_address}</Text>
          {tap && <Button title="Load vault from signer" disabled={busy} onPress={() => void toSigner(async () => ({ kind: 'recovery-vault-request', body: {} }))} />}
          <Text style={s.tiny}>Last policy: {btc(recoverySource.monthly_limit_sats)} BTC monthly · {btc(recoverySource.emergency_access_limit_sats)} BTC emergency</Text>
          <Button title="Check recovery timing" disabled={busy} onPress={() => run({ method: 'recovery_snapshot' }, setRecoveryTiming)} />
          {recoveryTiming && recoveryTiming.vault_address === recoverySource.vault_address && <>
            <Text style={s.rowValue}>{btc(recoveryTiming.hww.total_sats)} BTC in the recovery vault</Text>
            {recoveryTiming.utxo_count === 0 ? <Text style={s.subtext}>No confirmed vault outputs remain.</Text> : <>
              <Text style={s.label}>Signer alone</Text>
              <Text testID="source-hww-recovery-days" style={s.subtext}>{recoveryTiming.hww.seconds_remaining > 0
                ? `${Math.ceil(recoveryTiming.hww.seconds_remaining / 86_400)} days remaining · from ${new Date(recoveryTiming.hww.opens_at * 1000).toLocaleDateString(undefined, { day: 'numeric', month: 'long', year: 'numeric' })}`
                : 'Available now'}</Text>
              <Text style={s.tiny}>{btc(recoveryTiming.hww.mature_sats)} BTC your signer can move alone</Text>
            </>}
          </>}
          <Text style={s.heading}>Restore from cloud backup</Text>
          <Text style={s.tiny}>This phone’s identity · verify on the signer</Text><Text selectable style={s.mono}>{identity.vault_pubkey}</Text>
          <Text style={s.label}>Encrypted cloud backup JSON</Text>
          <TextInput accessibilityLabel="Encrypted cloud backup JSON" multiline value={backupText} onChangeText={setBackupText} maxLength={8 * 1024 * 1024} autoCapitalize="none" autoCorrect={false} spellCheck={false} autoComplete="off" textAlignVertical="top" style={[s.input, { height: 90 }]} />
          {!backupText && <Text style={s.tiny}>No cloud backup loaded.</Text>}
          <Button title="Request phone key recovery" disabled={busy || !backupText} onPress={requestRestore} />
          <Button title="Check restored key" disabled={busy} onPress={() => run({ method: 'restored_phone' }, setRestored)} />
          {restored && <><Text style={s.noticeText}>Old key recovered: {restored.old_phone_public_key}. Rotation is still required.</Text>
            <Button title="Propose restored vault rotation" primary disabled={busy} onPress={() => propose({ method: 'propose_restored_rotation' }, 'phone-rotation-proposal')} />
            <Button title="Review recovered wallet funds" disabled={busy} onPress={reviewRecovered} />
          </>}
          <Field label="Recovery destination" value={destination} onChange={setDestination} />
          <Button title="Use this wallet’s fresh address" disabled={busy} onPress={() => run({ method: 'receive_address' }, value => setDestination(value.address))} />
          <Button title="Request all-action revocation" danger disabled={busy} onPress={() => propose({ method: 'propose_offline_action', action: 'revoke_actions', destination }, 'offline-action-proposal')} />
          <Button title="Request HWW fallback" disabled={busy} onPress={() => propose({ method: 'propose_offline_action', action: 'hww_recovery', destination }, 'offline-action-proposal')} />
          <Button title="Close signer recovery" onPress={() => { setRecoverySource(null); go('home'); }} />
        </>}
      </>}
      {outgoing && (inPrivate || outgoing.kind === 'signer-identity' || outgoing.kind === 'vault-setup') && <View style={s.notice}>
        <Text style={s.noticeText}>{!signer && tap && deferred.includes(outgoing.kind) ? 'Once it confirms, finish on your signer' : `Ready to share · ${outgoing.kind.replaceAll('-', ' ')}`}</Text>
        <Button title={signer ? 'Send to wallet'
          : tap && signerBound.includes(outgoing.kind) ? (deferred.includes(outgoing.kind) ? 'Finish on signer' : 'Continue on signer') : 'Send to signer'} disabled={busy} onPress={shareOutgoing} />
      </View>}
      {!anySheet && feedback}
  </>;
  // The signer's reviews, one fact per page. Rust validated each request when it arrived; the
  // pages show what an approval signs, and the last one asks for it.
  function signerReview(): Review | null {
    if (!review) return null;
    const approve = (method: string, kind: Kind, extra: JsonObject = {}) => () =>
      void run({ method, review_id: review.review_id, ...extra }, value => { void send(kind, value); setReview(null); setReviewMessage(null); });
    const reject = (method: string) => () =>
      void run({ method, review_id: review.review_id }, () => { setReview(null); setReviewMessage(null); decline(); });
    switch (reviewKind) {
      case 'policy': return {
        pages: [
          <Page title="Annual vault policy" value={`₿${btc(review.vault_input_sats)}`}><Note>in your vault. Approving renews it for another year.</Note></Page>,
          <Page eyebrow="Monthly allowance" value={`₿${btc(review.monthly_limit_sats)} a month`}>
            <Note>To your wallet · {review.funded_withdrawals} of 12 months funded</Note>
            {review.funding_warning && <Note warning>{review.funding_warning}</Note>}
          </Page>,
          <Page eyebrow="Emergency access" value={review.emergency_delay_seconds ? `Up to ₿${btc(review.emergency_access_limit_sats)}` : 'Off'}>
            <Note>{review.emergency_delay_seconds ? `Phone only, after a one-week delay (${review.emergency_delay_seconds.toLocaleString('en-US')} seconds)` : 'No emergency withdrawal'}</Note>
          </Page>,
          <Page eyebrow="Sign" value={`${review.transaction_count} transactions`}>
            <Note>Rollover fee {review.rollover_fee_sats} sats. This signer rebuilt every transaction from these limits.</Note>
            {review.previous_policy_warning && <Note warning>{review.previous_policy_warning}</Note>}
          </Page>,
        ],
        reject: { label: 'Reject policy', onPress: reject('reject_policy') },
        confirm: { label: 'Approve and sign policy', onPress: approve('approve_policy', 'policy-approved'), hold: true },
      };
      case 'sweep': return {
        pages: [
          <Page title="Move vault funds" value={`₿${btc(review.amount_sats)}`}><Note>Fee {review.fee_sats} sats · moves immediately</Note></Page>,
          <Page eyebrow="To"><Code>{review.destination}</Code><Note>Check this address independently.</Note></Page>,
          <Page eyebrow="Sign"><Note>Moves every reviewed vault output now. It does not renew the policy.</Note></Page>,
        ],
        reject: { label: 'Reject vault transfer', onPress: reject('reject_sweep') },
        confirm: { label: 'Approve vault transfer', onPress: approve('approve_sweep', 'sweep-approved'), hold: true },
      };
      case 'backup': return {
        pages: [
          <Page title="Protect phone recovery"><Code>{review.vault_address}</Code><Note>Lets this signer help restore your phone key. It does not approve spending.</Note></Page>,
        ],
        reject: { label: 'Reject recovery backup', onPress: reject('reject_backup') },
        confirm: { label: 'Approve recovery backup', onPress: approve('approve_backup', 'backup-approved', { message: reviewMessage }) },
      };
      case 'offline-action': return {
        pages: [
          <Page title={review.action === 'hww_recovery' ? 'Recover vault funds' : 'Revoke all vault actions'} value={`₿${btc(review.amount_sats)}`} testID="offline-action-amount"><Note>Fee {review.fee_sats} sats</Note></Page>,
          <Page eyebrow="To"><Code>{review.destination}</Code></Page>,
          <Page eyebrow="From vault"><Code>{review.vault_address}</Code></Page>,
          <Page eyebrow="Sign"><Note>{review.recovery_delay_seconds
            ? `Bitcoin accepts this only ${Math.round(review.recovery_delay_seconds / 86_400)} days after each input confirmed, so a yearly renewal keeps it locked. This offline signer cannot check the time; signing does not unlock funds early.`
            : 'Spends the live connectors to disable their monthly and emergency actions. Savings stay in the vault; only connector change goes to this address.'}</Note></Page>,
        ],
        reject: { label: 'Reject offline action', onPress: reject('reject_offline_action') },
        confirm: { label: 'Approve offline action', onPress: approve('approve_offline_action', 'offline-action-approved'), hold: true },
      };
      case 'restore': return {
        pages: [
          <Page title="Unlock phone recovery"><Code>{review.vault_address}</Code><Note>Releases the backup key to a replacement phone.</Note></Page>,
          <Page eyebrow="Replacement phone"><Code>{review.recipient_public_key}</Code><Note>Compare with the phone’s screen.</Note></Page>,
          <Page eyebrow="Cloud backup"><Code>{review.backup_id}</Code><Note>This signer cannot read the backup; the phone checks it. Rotate the vault afterwards.</Note></Page>,
        ],
        reject: { label: 'Reject phone recovery', onPress: reject('reject_restore') },
        confirm: { label: 'Approve phone recovery', onPress: approve('approve_restore', 'restore-approved'), hold: true },
      };
      case 'rotation': return {
        pages: [
          <Page title="Rotate the recovered vault" value={`₿${btc(review.amount_sats)}`}><Note>moves to a new vault with a fresh phone key.</Note></Page>,
          <Page eyebrow="New phone"><Code>{review.new_phone_public_key}</Code><Note>Compare with the replacement phone.</Note></Page>,
          <Page eyebrow="From"><Code>{review.old_vault_address}</Code><Text style={[r.tiny, { textAlign: 'center', marginTop: 12 }]}>To</Text><Code>{review.new_vault_address}</Code></Page>,
          <Page eyebrow="Policy">
            <Note>Preserve {btc(review.monthly_limit_sats)} BTC monthly and {btc(review.emergency_access_limit_sats)} BTC emergency. {review.funded_withdrawals} monthly withdrawals funded.</Note>
            <Note>Transfer fee {review.sweep_fee_sats} sats. {review.revokes_connectors ? 'Revoke the old connectors and return their change to the new vault.' : 'No live old connectors were supplied.'}</Note>
            {review.previous_policy_warning && <Note warning>{review.previous_policy_warning}</Note>}
          </Page>,
        ],
        reject: { label: 'Reject phone rotation', onPress: reject('reject_phone_rotation') },
        confirm: { label: 'Approve phone rotation', onPress: approve('approve_phone_rotation', 'phone-rotation-approved', { request: reviewMessage }), hold: true },
      };
      case 'rotation-adoption': return {
        pages: [<Page title="Finish rotation pairing"><Code>{review.new_vault_address}</Code><Note>The phone reports its rotation was broadcast. Check it confirmed on the phone first; this signer cannot see the chain.</Note></Page>],
        reject: { label: 'Keep previous pairing', onPress: () => { setReview(null); decline(); } },
        confirm: { label: 'Use the rotated vault', onPress: () => void run({ method: 'adopt_phone_rotation', request_id: review.request_id }, value => {
          setPairing(value); setReview(null); setNotice('Signer paired with the rotated vault. The spending limits are preserved.'); void send('vault-accepted', { vault_address: value.vault_address });
        }) },
      };
      case 'share-vault': return {
        pages: [<Page title="Share your vault with this phone?"><Code>{review.vault_address}</Code><Note>The phone gets your vault’s public details to start a recovery. It cannot move funds without this signer.</Note></Page>],
        reject: { label: 'Don’t share', onPress: () => { setReview(null); decline(); } },
        confirm: { label: 'Share vault details', onPress: () => void run({ method: 'export_recovery_bundle' }, value => { void send('recovery-vault', value); setReview(null); }) },
      };
    }
  }
  // Between requests the signer has little to show: whether it is ready, and for files (the
  // fallback without a tap link) the messages to carry.
  const signerIdle = <>
    {!identity && !tap ? <Page title="Your second key.">
      <Note>Use this separate device to approve vault policy. A second phone is not a secure-element hardware wallet.</Note>
      <View style={{ alignSelf: 'stretch', marginTop: 12 }}><Button title="Create signer" primary onPress={initialize} disabled={busy || !sessionLoaded}/></View>
    </Page> : tap ? <View style={{ alignItems: 'center' }}>
      <Waves on={linked}/>
      <Text style={{ color: colors.text, fontSize: 24, lineHeight: 29, fontWeight: '500', letterSpacing: -.4, textAlign: 'center', marginTop: 14 }}>{linked ? 'Connected' : paired ? 'Ready.' : 'Ready to pair.'}</Text>
      <Text style={[s.tiny, { color: colors.quiet, textAlign: 'center', marginTop: 8 }]}>{linked ? 'Talking to your phone.' : paired ? 'Requests from your phone appear here.' : 'On your phone, open Vault and choose Set up vault.'}</Text>
      {pairing && !linked && <AddressLines address={pairing.vault_address} vault size={9.5} testID="signer-vault-address"/>}
    </View> : <Page title={paired ? 'Ready.' : 'Your second key.'}>
      {pairing ? <Code>{pairing.vault_address}</Code> : <View style={{ alignSelf: 'stretch', marginTop: 12 }}><Button title="Share signer public key" onPress={() => void send('signer-identity', identity!)} disabled={busy}/></View>}
      {paired && <View style={{ alignSelf: 'stretch', marginTop: 8 }}><Button title="Share public recovery vault" disabled={busy} onPress={() => run({ method: 'export_recovery_bundle' }, value => void send('recovery-vault', value))}/></View>}
    </Page>}
    {incoming && <View style={[s.notice, { marginTop: 14 }]}><Text style={[s.noticeText, { textAlign: 'center' }]}>Incoming · {incoming.kind.replaceAll('-', ' ')}</Text><Button title="Open received message" onPress={importMessage} disabled={busy}/></View>}
    {outgoing && <View style={[s.notice, { marginTop: 14 }]}><Text style={[s.noticeText, { textAlign: 'center' }]}>Ready to share · {outgoing.kind.replaceAll('-', ' ')}</Text><Button title="Send to wallet" onPress={shareOutgoing} disabled={busy}/></View>}
    {!!notice && <Note>{notice}</Note>}
    {!!error && <Text selectable accessibilityRole="alert" style={[r.errorText, { textAlign: 'center', marginTop: 10 }]}>{error}</Text>}
    {(busy || !sessionLoaded) && <ActivityIndicator color={colors.mint} style={{ marginTop: 12 }}/>}
    {identity && <SavedMessages transport={transport} deliver={deliver}/>}
    {signerSettings ? <>{deviceSettings}{recoveryPhrase}</>
      : <Pressable accessibilityRole="button" accessibilityLabel="Device settings" onPress={() => setSignerSettings(true)} hitSlop={12} style={({ pressed }) => ({ alignSelf: 'center', marginTop: 18, padding: 8, opacity: pressed ? .6 : 1 })}>
        <Text style={[s.tiny, { color: colors.quiet }]}>Device settings</Text>
      </Pressable>}
  </>;
  if (signer) {
    const current = signerReview();
    return <SheetHost sheets={hostSheets}><View style={{ flex: 1, paddingTop: insets.top, paddingBottom: insets.bottom }}><SignerDevice screen={(_, height) => current
      ? <Pager key={review?.review_id ?? review?.request_id ?? reviewKind} review={current} busy={busy} inset={16}/>
      : <ScrollView showsVerticalScrollIndicator={false} keyboardShouldPersistTaps="handled"
        keyboardDismissMode="interactive" automaticallyAdjustKeyboardInsets
        contentContainerStyle={{ minHeight: height, justifyContent: 'center', padding: 18 }}>
        {signerIdle}
      </ScrollView>}/></View></SheetHost>;
  }
  const reviewPayment = () => void run({ method: 'review_payment', destination: destination.trim(), amount_sats: Number(payment) }, value => { setPaymentReview(value); setPage('confirm-send'); });
  const changeTab = (value: 'wallet' | 'vault') => {
    screenScroll.current?.scrollTo({ y: 0, animated: false });
    if (value === 'wallet') hideVault(); else { setTab('vault'); setPage('home'); setError(''); setNotice(''); setScrolled(false); }
  };
  // Pull to refresh, on the wallet's home and on the open vault's.
  const canRefresh = identity && page === 'home' && (tab === 'wallet' || vaultVisible);
  const refresh = async () => {
    setRefreshing(true);
    try { await (tab === 'wallet' ? syncWallet() : syncVault()); } finally { setRefreshing(false); }
  };
  // A device keeps the screen clear of the status bar and lets it run under the system's tab bar;
  // the emulator draws its own status bar and tab pill, and leaves room for them itself.
  const screen = <>
    <DarkBackground/>
    {identity && <View style={{ position: 'absolute', top: web ? 56 : insets.top + 8, left: 26, right: 26, flexDirection: 'row', justifyContent: 'space-between', zIndex: 10 }} pointerEvents="box-none">
      {/* A closed vault has nothing to configure or hide yet. */}
      {tab === 'vault' && !vaultVisible && page === 'home' ? <View/> : <IconButton label={page !== 'home' && !anySheet ? (tab === 'wallet' ? 'Back to wallet' : 'Back to vault') : 'Settings'} name={page !== 'home' && !anySheet ? 'close' : 'settings'} onPress={() => go(page !== 'home' ? 'home' : 'settings')}/>}
      {tab === 'vault' && (vaultVisible || page !== 'home') && <IconButton label="Hide vault" name="close" onPress={hideVault}/>}
    </View>}
    {!web && <TopFade height={insets.top + 84} solid={insets.top + 48} shown={scrolled}/>}
    <ScrollView ref={screenScroll} style={!web && { marginTop: insets.top }}
      onScroll={web ? undefined : event => setScrolled(event.nativeEvent.contentOffset.y > 6)} scrollEventThrottle={32}
      contentContainerStyle={[s.content, !web && { paddingBottom: TAB_BAR_CLEARANCE + insets.bottom + 24 }, tab === 'vault' && page !== 'home' && !vaultSheet && { paddingTop: 56 }]}
      keyboardShouldPersistTaps="handled" keyboardDismissMode="interactive" automaticallyAdjustKeyboardInsets showsVerticalScrollIndicator={false}
      refreshControl={canRefresh && !web ? <RefreshControl refreshing={refreshing} onRefresh={() => void refresh()} tintColor={colors.mint}/> : undefined}
      accessibilityElementsHidden={anySheet} importantForAccessibility={anySheet ? 'no-hide-descendants' : 'auto'}>
      {screenContent}
    </ScrollView>
  </>;
  const sheets = <>
    {walletSheet && <Sheet label={page === 'receive' ? 'Receive bitcoin' : page === 'transaction' ? 'Transaction details' : 'Send bitcoin'} onClose={() => { setPage('home'); setError(''); }}>
      {page === 'receive' && <><Text style={s.sheetEyebrow}>Receive</Text><AddressCard address={address}/></>}
      {page === 'send' && <><Text style={s.sheetEyebrow}>Send</Text>
        <TextInput accessibilityLabel="Amount in sats" value={payment} onChangeText={setPayment} keyboardType="number-pad" placeholder="0" placeholderTextColor={colors.quiet} selectTextOnFocus
          style={{ marginTop: 26, textAlign: 'center', fontFamily: fonts.number, fontSize: 38, fontWeight: '500', fontVariant: ['tabular-nums'], color: Number(payment) > 0 ? colors.text : colors.quiet }}/>
        <Text style={{ marginTop: 8, textAlign: 'center', fontFamily: fonts.number, fontSize: 12, color: colors.faint }}>sats · ₿{btc(Number(payment) || 0)}</Text>
        <View style={{ marginTop: 24, marginHorizontal: 6, flexDirection: 'row', alignItems: 'center', gap: 10, paddingLeft: 18, paddingRight: 10, borderRadius: 26, backgroundColor: 'rgba(255,255,255,.07)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.12), 0 0 0 .5px rgba(255,255,255,.05)' }}>
          <TextInput ref={destinationInput} accessibilityLabel="Destination address" value={destination} onChangeText={setDestination} placeholder="Bitcoin address" placeholderTextColor={colors.quiet}
            autoCapitalize="none" autoCorrect={false} spellCheck={false} autoComplete="off" returnKeyType="done" onSubmitEditing={reviewPayment}
            style={{ flex: 1, paddingVertical: 15, color: colors.text, fontSize: 12.5, fontFamily: destination ? fonts.mono : undefined }}/>
          {/* A payment link carries its address after "bitcoin:"; the address is what Rust takes. */}
          {!destination && <PasteButton onPaste={text => setDestination(text.replace(/^bitcoin:/i, '').split('?')[0])}/>}
        </View>
        <View style={{ alignItems: 'center', marginTop: 16 }}>
          <Pressable accessibilityRole="button" accessibilityLabel="Review payment" disabled={busy} onPress={reviewPayment} hitSlop={4}
            style={({ pressed }) => ({ height: 40, paddingHorizontal: 22, borderRadius: 20, flexDirection: 'row', alignItems: 'center', gap: 7, backgroundColor: 'rgba(255,255,255,.08)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.14), 0 0 0 .5px rgba(255,255,255,.06)', opacity: busy ? .4 : 1, transform: [{ scale: pressed ? .95 : 1 }] })}>
            <Icon name="send" size={13} strokeWidth={2.2} color="rgba(255,255,255,.64)"/><Text style={{ color: colors.text, fontSize: 12.5, fontWeight: '500' }}>Review</Text>
          </Pressable>
        </View></>}
      {page === 'confirm-send' && paymentReview && <><Text style={s.sheetEyebrow}>Confirm payment</Text><Text style={s.rowValue}>{paymentReview.amount_sats} sats</Text><Text selectable style={s.mono}>{paymentReview.destination}</Text><Text style={s.subtext}>Fee: {paymentReview.fee_sats} sats · Total: {paymentReview.total_sats} sats</Text>
        <Button title="Confirm and send" primary disabled={busy} onPress={() => void secure(() => run({ method: 'confirm_payment', review_id: paymentReview.review_id }, () => setPage('home')))} /><Button title="Go back" onPress={() => run({ method: 'reject_payment', review_id: paymentReview.review_id }, () => { setPaymentReview(null); setPage('send'); })} disabled={busy} /></>}
      {page === 'transaction' && selectedActivity && <><Text style={s.sheetEyebrow}>Transaction details</Text>
        <Text style={[s.balance, { fontSize: 30, textAlign: 'center' }]}>{selectedActivity.balance_change_sats < 0 ? '−' : '+'}₿{btc(Math.abs(selectedActivity.balance_change_sats))}</Text>
        <Text style={s.subtext}>{selectedActivity.confirmed ? 'Confirmed' : 'Awaiting confirmation'} · wallet balance change{selectedActivity.balance_change_sats < 0 ? ', including fees' : ''}</Text>
        <Text selectable style={s.mono}>{selectedActivity.txid}</Text><Button title="Back to wallet" onPress={() => setPage('home')}/>
      </>}
      {feedback}
    </Sheet>}
    {vaultSheet && vault && <Sheet label={vaultSheetLabels[page]} onClose={() => go('home')}>
      <Text style={s.sheetEyebrow}>{vaultSheetLabels[page]}</Text>
      {page === 'deposit' && <AddressCard address={vault.config.vault_address} vault/>}
      {page === 'withdraw' && <WithdrawalOptions go={go} vault={vault}/>}
      {page === 'settings' && <VaultSettings vault={vault} disabled={busy} noBackup={signerKeepsNoBackup}
        go={go} backup={() => propose({ method: 'propose_backup' }, 'backup-proposal', () => setPage('home'))}
        reviewRecovered={() => { go('recovered-wallet'); void reviewRecovered(); }}>
        <Descriptors kind="vault" busy={busy} run={run}/>
      </VaultSettings>}
      {page === 'lost' && <View style={{ gap: 18, marginTop: 22, marginHorizontal: 6 }}>
        {([['key', 'Lost your signer?', 'Restore it from the backup its maker gave you. It comes back as the same key, so your vault doesn’t change.'],
          ['phone', 'Lost your phone?', 'On a new phone, open Vault and recover with your signer. Your cloud backup restores this phone’s key.'],
          ['clock', 'Lost both?', 'Nobody can move your savings. Restore your signer from its backup: it can move them alone a year after your last renewal.']] as const).map(([icon, title, detail]) =>
          <View key={icon} style={{ flexDirection: 'row', gap: 14, alignItems: 'center' }}>
            <View style={{ width: 40, height: 40, borderRadius: 20, alignItems: 'center', justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.06)' }}><Icon name={icon} size={18}/></View>
            <View style={{ flex: 1 }}><Text style={{ color: colors.text, fontSize: 14, fontWeight: '500' }}>{title}</Text><Text style={[s.tiny, { color: colors.quiet, marginTop: 2 }]}>{detail}</Text></View>
          </View>)}
        <MenuRow title="Signer recovery and revocation" subtitle="Restore a phone or disable vault actions" disabled={busy}
          onPress={() => void run({ method: 'recovery_source' }, value => { setRecoverySource(value); go('signer-recovery'); })}/>
      </View>}
      {page === 'saved-messages' && <SavedMessages transport={transport} deliver={deliver} open/>}
      {page === 'policy' && <PolicyEditor vault={vault} monthly={monthly} setMonthly={setMonthly} emergency={emergency} setEmergency={setEmergency}
        disabled={busy} onPropose={() => proposal()} onRenew={() => proposal('refresh_policy')}/>}
      {page === 'monthly' && <>
        <Text style={[s.balance, { fontSize: 34, textAlign: 'center', marginTop: 12 }]}>₿{btc(vault.config.monthly_limit_sats)}</Text>
        <Text style={[s.balanceCaption, { marginBottom: 18 }]}>a month, to your wallet</Text>
        <Button title="Withdraw monthly allowance" primary disabled={busy} onPress={() => void secure(() => run({ method: 'monthly', step: Number(step), action: 'authorization' }))}/>
        <Button title="Revoke remaining allowances" danger disabled={busy} onPress={() => run({ method: 'monthly', step: Number(step), action: 'revocation' })}/>
        {!moreMonthly ? <Pressable accessibilityRole="button" accessibilityLabel="More options" onPress={() => setMoreMonthly(true)} hitSlop={8} style={({ pressed }) => ({ alignSelf: 'center', padding: 12, opacity: pressed ? .6 : 1 })}>
          <Text style={[s.tiny, { color: colors.quiet }]}>More options</Text></Pressable> : <>
          <Field label="Allowance step" value={step} onChange={setStep} numeric/>
          <Field label="Soft limit in sats" value={softLimit} onChange={setSoftLimit} numeric/>
          <Button title="Apply soft limit" disabled={busy} onPress={() => run({ method: 'apply_soft_limit', step: Number(step), limit_sats: Number(softLimit) })}/>
          <Text style={s.tiny}>Returns anything above the soft limit to the vault.</Text>
        </>}
      </>}
      {page === 'emergency' && <>
        <Text style={[s.balance, { fontSize: 34, textAlign: 'center', marginTop: 12 }]}>₿{btc(vault.config.emergency_access_limit_sats)}</Text>
        <Text style={[s.balanceCaption, { marginBottom: 18 }]}>Phone only · arrives a week after you start</Text>
        <Button title="Start emergency access" primary disabled={busy} onPress={() => void secure(() => run({ method: 'initiate_emergency' }))}/>
        <Button title="Withdraw emergency funds" disabled={busy} onPress={() => void secure(() => run({ method: 'withdraw_emergency' }))}/>
        <Button title="Cancel emergency access" danger disabled={busy} onPress={() => run({ method: 'cancel_emergency' })}/>
      </>}
      {page === 'sweep' && <>
        <Field label="Vault transfer destination" value={destination} onChange={setDestination}/>
        <Text style={s.subtext}>Both devices approve an immediate transfer of all confirmed vault outputs. This does not renew the monthly policy.</Text>
        <Button title="Propose vault transfer" primary disabled={busy} onPress={() => propose({ method: 'propose_sweep', destination }, 'sweep-proposal', () => setPage('home'))}/>
        <Button title="Cancel vault transfer" onPress={() => go('home')}/>
      </>}
      {feedback}
    </Sheet>}
    {welcome && <Sheet label="Welcome" onClose={() => setWelcome(false)}>
      <Text style={[s.balance, { fontSize: 24, lineHeight: 30, textAlign: 'center', marginTop: 26 }]}>Your wallet is ready</Text>
      <View style={{ gap: 18, marginTop: 26, marginHorizontal: 6 }}>
        {([['face', `Protected by ${ownerCheck}`, 'Only you can open it.'],
          ['cloud', 'Backed up for you', 'Encrypted across your Apple devices. Nothing to write down.'],
          ['phone', 'Lost your phone?', `Recover on a new iPhone with ${ownerCheck.startsWith('your') ? ownerCheck : `your ${ownerCheck}`}.`]] as const).map(([icon, title, detail]) =>
          <View key={icon} style={{ flexDirection: 'row', gap: 14, alignItems: 'center' }}>
            <View style={{ width: 40, height: 40, borderRadius: 20, alignItems: 'center', justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.06)' }}><Icon name={icon} size={18}/></View>
            <View style={{ flex: 1 }}><Text style={{ color: colors.text, fontSize: 14, fontWeight: '500' }}>{title}</Text><Text style={[s.tiny, { color: colors.quiet, marginTop: 2 }]}>{detail}</Text></View>
          </View>)}
      </View>
      <View style={{ marginTop: 28 }}><Button title="Get started" primary onPress={() => setWelcome(false)}/></View>
    </Sheet>}
    {tapState && <TapSheet state={tapState} hint={tap?.hint} onCancel={cancelTap} onRetry={() => void connectAndSend()} onDone={() => setTapState(null)}/>}
    {hostSheets}
  </>;
  return <View style={s.screen} testID="device-screen" {...({ dataSet: { vaultHeight: vaultVisible && vault ? vault.chain_height : '' } } as object)}>
    <SheetHost sheets={sheets}>
      <DeviceStatus/><DarkEffects/>
      {/* Nothing of the wallet is on screen while it is locked, for a screen reader either. */}
      <TabBar tab={tab} onChange={changeTab} hidden={!identity || locked}>{locked ? null : screen}</TabBar>
      {web && <View pointerEvents="none" style={{ position: 'absolute', bottom: 9, left: '50%', marginLeft: -67, width: 134, height: 5, borderRadius: 3, backgroundColor: '#ffffff3d' }}/>}
      {(locked || away) && <View style={[StyleSheet.absoluteFill, { zIndex: 85, backgroundColor: colors.background, alignItems: 'center', justifyContent: 'center', padding: 40 }]}>
        {/* The owner's check did not pass, or was put away: the wallet stays covered until it does. */}
        {locked && unlockFailed && <><Text style={[s.balance, { fontSize: 28, lineHeight: 34 }]}>Locked</Text>
          <View style={{ alignSelf: 'stretch', marginTop: 22 }}><Button title="Unlock" primary onPress={() => void unlock()}/></View></>}
      </View>}
      {faceId.prompt}
    </SheetHost>
  </View>;
}
