import { Directory, File, FileMode, Paths } from 'expo-file-system';
import * as DocumentPicker from 'expo-document-picker';
import * as Sharing from 'expo-sharing';
import { fileExchange } from './fileExchange';

let exportSequence = 0;
export const nativeFileExchange = fileExchange({
  async pick() {
    const result = await DocumentPicker.getDocumentAsync({
      type: ['application/json', 'text/plain', 'application/octet-stream'],
      multiple: false,
      copyToCacheDirectory: true,
    });
    if (result.canceled) return null;
    const asset = result.assets[0];
    if (!asset) throw new Error('No file was selected.');
    return { size: asset.size, open: () => new File(asset.uri).open(FileMode.ReadOnly) };
  },
  sharingAvailable: Sharing.isAvailableAsync,
  async createExport(channel, text) {
    const directory = new Directory(Paths.cache, 'anzen-file-exchange');
    directory.create({ idempotent: true });
    const prefix = channel === 'backup' ? 'anzen-encrypted-recovery-backup' : 'anzen-message';
    // Names contain no device identity or wallet data. A collision fails closed;
    // never overwrite another export or accept a path from received JSON.
    const file = new File(directory, `${prefix}-${Date.now()}-${++exportSequence}.json`);
    file.create({ overwrite: false });
    file.write(text);
    return file.uri;
  },
  async share(uri, title) {
    await Sharing.shareAsync(uri, { mimeType: 'application/json', UTI: 'public.json', dialogTitle: title });
    // The receiver may read lazily after its activity opens. Keep this cache copy;
    // OS eviction may remove it later. It is not a durable backup. Never delete a
    // user-selected source file, and never share the private wallet directory.
  },
});
