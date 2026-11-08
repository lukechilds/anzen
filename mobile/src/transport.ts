// Application commands are interpreted and validated by Rust. This is only a transport contract.
export type Role = 'wallet' | 'signer';
export type JsonObject = Record<string, any>;
export interface DeviceTransport {
  readonly role: Role;
  execute(command: JsonObject): Promise<JsonObject>;
  // Hints that the chain may have changed for this wallet; the listener re-runs the ordinary
  // Rust sync. Returns an unsubscribe function. Transports without a change signal omit it.
  watch?(listener: (reason: string) => void): () => void;
}
// A native chooser closing is not a delivery receipt. The in-browser courier can
// explicitly acknowledge placing a copy in the other instance's inbox, or saving
// a cloud file. Neither result implies that a signing operation was approved.
export type TransferResult = 'completed' | 'offered';
export type Deliver = (message: ProtocolMessage) => TransferResult | Promise<TransferResult>;
export const protocolKinds = [
  'signer-identity', 'vault-setup', 'vault-accepted', 'policy-proposal', 'policy-approved',
  'sweep-proposal', 'sweep-approved', 'backup-proposal', 'backup-approved', 'recovery-vault',
  'offline-action-proposal', 'offline-action-approved', 'restore-proposal', 'restore-approved',
  'phone-rotation-proposal', 'phone-rotation-approved', 'phone-rotation-completed',
  // Requests that exist only during a tap: the phone asks; the signer answers with
  // signer-identity or recovery-vault.
  'signer-hello', 'recovery-vault-request',
] as const;
export interface ProtocolMessage {
  kind: typeof protocolKinds[number];
  body: JsonObject;
}

// A tap: the phone and its signer held together. Each platform supplies the link (in-page in the
// playground; NFC or Bluetooth on devices). What crosses it is ordinary protocol JSON, which each
// device's Rust validates exactly as it would a received file.
export interface SignerLink {
  // Resolves with the signer's answer. An answer that needs the signer's user waits for them.
  request(message: ProtocolMessage): Promise<ProtocolMessage>;
  close(): void;
}
// The signer's side: answer one request. The signal aborts when the phone moves away.
export type TapAnswer = (message: ProtocolMessage, signal: AbortSignal) => Promise<ProtocolMessage>;
export interface Tap {
  // Phone: resolves once a signer is held close. Aborting stops waiting.
  connect(signal: AbortSignal): Promise<SignerLink>;
  // Signer: answer phones held close. Returns an unsubscribe function.
  answer(handler: TapAnswer): () => void;
}
// Messages a phone sends to its own signer, which therefore travel by tap. A rotation's
// completion waits for confirmation first, so the phone offers it later.
export const signerBound: readonly ProtocolMessage['kind'][] = [
  'signer-hello', 'recovery-vault-request', 'vault-setup', 'policy-proposal', 'sweep-proposal',
  'backup-proposal', 'offline-action-proposal', 'restore-proposal', 'phone-rotation-proposal',
  'phone-rotation-completed',
];
