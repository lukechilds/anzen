import { Platform, StyleSheet } from 'react-native';

export const colors = {
  background: '#060607', surface: '#18181c', border: '#ffffff14',
  text: 'rgba(255,255,255,.92)', secondary: 'rgba(255,255,255,.68)', muted: 'rgba(255,255,255,.55)',
  quiet: 'rgba(255,255,255,.42)', faint: 'rgba(255,255,255,.30)', mint: 'rgba(167,227,187,.95)', red: '#e89b92',
};
export const fonts = {
  mono: Platform.select({ web: 'ui-monospace, "SF Mono", Menlo, Consolas, monospace', ios: 'Menlo', default: 'monospace' }),
  number: Platform.select({ web: 'ui-rounded, -apple-system, BlinkMacSystemFont, sans-serif', ios: 'System', default: 'sans-serif' }),
};
// Bitcoin amounts show at least four decimal places (₿0.1000), and every significant satoshi.
export const formatBtc = (sats: number) =>
  (sats / 100_000_000).toLocaleString('en-US', { minimumFractionDigits: 4, maximumFractionDigits: 8 });
// Raised glass: a top highlight and a hairline ring, as on the mockup's pills and sheets.
export const glass = 'inset 0 1px 0 rgba(255,255,255,.14), 0 0 0 .5px rgba(255,255,255,.06)';
export const styles = StyleSheet.create({
  screen: { flex: 1, backgroundColor: colors.background, overflow: 'hidden', borderRadius: Platform.OS === 'web' ? 48 : 0 },
  top: { height: 74, paddingHorizontal: 24, flexDirection: 'row', alignItems: 'center', justifyContent: 'space-between' },
  brand: { color: colors.text, fontSize: 26, fontWeight: '600', letterSpacing: -1.5 },
  tiny: { color: colors.muted, fontSize: 11, lineHeight: 17 },
  badge: { paddingHorizontal: 10, paddingVertical: 6, borderRadius: 16, backgroundColor: '#ffffff08' },
  content: { paddingHorizontal: 26, paddingBottom: 132 },
  eyebrow: { color: colors.faint, textTransform: 'uppercase', letterSpacing: 4, fontSize: 10, lineHeight: 12, textAlign: 'center', marginBottom: 16 },
  hero: { alignItems: 'center', paddingTop: 64, paddingBottom: 0 },
  balance: { color: colors.text, fontSize: 46, lineHeight: 54, fontWeight: '500', letterSpacing: -.5, fontVariant: ['tabular-nums'], fontFamily: Platform.OS === 'web' ? fonts.number : undefined },
  balanceCaption: { color: 'rgba(255,255,255,.42)', fontSize: 12, lineHeight: 15, marginTop: 12, textAlign: 'center' },
  subtext: { color: colors.muted, fontSize: 13, lineHeight: 21, marginVertical: 12 },
  actions: { flexDirection: 'row', gap: 12, marginTop: 54, width: '100%' },
  button: { backgroundColor: 'rgba(255,255,255,.08)', borderRadius: 26, paddingHorizontal: 19, minHeight: 48, alignItems: 'center', justifyContent: 'center', marginVertical: 5, boxShadow: 'inset 0 1px 0 rgba(255,255,255,.16), 0 0 0 .5px rgba(255,255,255,.07)' },
  buttonText: { color: colors.text, fontWeight: '500', fontSize: 13 },
  primary: { backgroundColor: 'rgba(255,255,255,.14)' },
  primaryText: { color: colors.text },
  danger: {},
  dangerText: { color: colors.red },
  heading: { color: colors.text, fontSize: 21, letterSpacing: -.5, fontWeight: '500', marginTop: 24, marginBottom: 8 },
  row: { paddingVertical: 19, borderBottomColor: colors.border, borderBottomWidth: 1 },
  rowHeading: { color: colors.text, fontSize: 14, fontWeight: '500' },
  rowValue: { color: colors.text, fontSize: 25, marginTop: 10, fontWeight: '400' },
  label: { color: colors.muted, fontSize: 12, marginBottom: 8, marginTop: 16 },
  input: { borderRadius: 26, paddingVertical: 15, paddingHorizontal: 18, color: colors.text, backgroundColor: 'rgba(255,255,255,.07)', fontSize: 13, boxShadow: 'inset 0 1px 0 rgba(255,255,255,.12), 0 0 0 .5px rgba(255,255,255,.05)' },
  tabs: { position: 'absolute', left: 20, right: 20, bottom: 28, height: 60, zIndex: 25, flexDirection: 'row', padding: 5, borderRadius: 30, backgroundColor: 'rgba(28,28,33,.46)', overflow: 'hidden', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.14), 0 0 0 .5px rgba(255,255,255,.08), 0 16px 36px rgba(0,0,0,.5)' },
  tab: { flex: 1, height: 50, alignItems: 'center', justifyContent: 'center', gap: 3, borderRadius: 25 },
  selectedTab: { backgroundColor: 'rgba(255,255,255,.12)', boxShadow: 'inset 0 1px 0 rgba(255,255,255,.16), 0 0 0 .5px rgba(255,255,255,.05)' },
  tabText: { color: colors.faint, fontSize: 10.5, lineHeight: 13, fontWeight: '500', letterSpacing: .2 },
  selectedText: { color: colors.text },
  notice: { borderRadius: 14, padding: 14, backgroundColor: '#a7e3bb0a', marginVertical: 12 },
  noticeText: { color: colors.mint, fontSize: 12, lineHeight: 19 },
  error: { backgroundColor: '#efaca30a' },
  errorText: { color: colors.red, fontSize: 12, lineHeight: 19 },
  // Addresses and keys have no spaces, so let them wrap anywhere rather than overflow.
  mono: { fontFamily: fonts.mono, color: colors.muted, fontSize: 11, lineHeight: 17, ...(Platform.OS === 'web' ? { wordBreak: 'break-all' } as object : {}) },
  // Sheet titles are smaller and brighter than page eyebrows.
  sheetEyebrow: { color: colors.quiet, textTransform: 'uppercase', letterSpacing: 3, fontSize: 9.5, lineHeight: 12, textAlign: 'center', marginTop: 16 },
  // Status footers under settings, e.g. "mainnet · block 908,391".
  footnote: { fontFamily: fonts.mono, fontSize: 9.5, letterSpacing: 1, color: colors.faint, textAlign: 'center', marginTop: 44 },
  separator: { height: 1, backgroundColor: colors.border, marginVertical: 18 },
});
