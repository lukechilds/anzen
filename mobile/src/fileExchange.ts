import { protocolKinds } from './transport';
import type { JsonObject, ProtocolMessage, Role, TransferResult } from './transport';

export const MAX_TRANSFER_BYTES = 8 * 1024 * 1024;
type Channel = 'protocol' | 'backup';
export interface TransferReader {
  readBytes(length: number): Uint8Array;
  close(): void;
}
export interface SelectedFile {
  size?: number;
  open(): TransferReader;
}
export interface FileExchangeIO {
  pick(): Promise<SelectedFile | null>;
  sharingAvailable(): Promise<boolean>;
  createExport(channel: Channel, text: string): Promise<string>;
  share(uri: string, title: string): Promise<void>;
}

const object = (value: unknown): value is JsonObject =>
  value !== null && typeof value === 'object' && !Array.isArray(value);
const tooLarge = () => new Error('File exceeds the 8 MiB transfer limit.');

function parse(text: string): unknown {
  if (text.length > MAX_TRANSFER_BYTES || new TextEncoder().encode(text).length > MAX_TRANSFER_BYTES) throw tooLarge();
  // Parser errors must not echo fragments of an accidentally selected private file.
  try { return JSON.parse(text); }
  catch { throw new Error('The selected content is not valid JSON.'); }
}

export function parseProtocolMessage(text: string): ProtocolMessage {
  const value = parse(text);
  // Transport framing only. Rust still authenticates the contents and independently
  // validates every policy, destination, signature and approval when the user opens it.
  if (!object(value) || !protocolKinds.includes(value.kind) || !object(value.body)
    || Object.keys(value).some(key => key !== 'kind' && key !== 'body')) {
    throw new Error('Not an Anzen protocol message. Encrypted backups must be imported separately in wallet mode.');
  }
  return value as ProtocolMessage;
}

export function parseEncryptedBackup(text: string): JsonObject {
  const value = parse(text);
  if (!object(value) || !object(value.encrypted_payload) || 'body' in value) {
    throw new Error('Not an encrypted backup file. Protocol messages must be imported separately.');
  }
  // This is not backup validation or decryption; those belong to hot_wallet.
  return value;
}

export function readTransfer(file: SelectedFile): string {
  if (file.size !== undefined && file.size > MAX_TRANSFER_BYTES) throw tooLarge();
  const reader = file.open();
  try {
    const chunks: Uint8Array[] = [];
    let length = 0;
    for (;;) {
      // Do not trust provider metadata or assume one read fills the requested buffer.
      const chunk = reader.readBytes(Math.min(64 * 1024, MAX_TRANSFER_BYTES + 1 - length));
      if (chunk.length === 0) break;
      length += chunk.length;
      if (length > MAX_TRANSFER_BYTES) throw tooLarge();
      chunks.push(chunk);
    }
    const bytes = new Uint8Array(length);
    let offset = 0;
    for (const chunk of chunks) { bytes.set(chunk, offset); offset += chunk.length; }
    try { return new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
    catch { throw new Error('The selected file is not valid UTF-8.'); }
  } finally { reader.close(); }
}

export function fileExchange(io: FileExchangeIO) {
  const requireWallet = (role: Role) => {
    if (role !== 'wallet') throw new Error('Encrypted backup files belong on the wallet, not the signer.');
  };
  async function pick() {
    const selected = await io.pick();
    return selected === null ? null : readTransfer(selected);
  }
  async function share(channel: Channel, text: string): Promise<TransferResult> {
    if (!await io.sharingAvailable()) throw new Error('File sharing is unavailable on this device.');
    const uri = await io.createExport(channel, text);
    await io.share(uri, channel === 'backup'
      ? 'Anzen encrypted recovery backup — save separately from the signer'
      : 'Anzen protocol message');
    // Expo does not expose delivery or cancellation confirmation. Never discard the
    // outgoing request or report a successful backup just because the chooser closed.
    return 'offered';
  }
  return {
    async receiveMessage(): Promise<ProtocolMessage | null> {
      const text = await pick();
      return text === null ? null : parseProtocolMessage(text);
    },
    async receiveBackup(role: Role): Promise<JsonObject | null> {
      requireWallet(role);
      const text = await pick();
      return text === null ? null : parseEncryptedBackup(text);
    },
    async sendMessage(message: ProtocolMessage): Promise<TransferResult> {
      const text = JSON.stringify(message);
      parseProtocolMessage(text);
      return share('protocol', text);
    },
    async saveBackup(role: Role, backup: JsonObject): Promise<TransferResult> {
      requireWallet(role);
      const text = JSON.stringify(backup);
      parseEncryptedBackup(text);
      return share('backup', text);
    },
  };
}
