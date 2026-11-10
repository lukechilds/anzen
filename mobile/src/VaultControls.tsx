import React from 'react';
import { Pressable, Text, TextInput, View } from 'react-native';
import { MenuRow, OptionCard } from './DarkSurfaces';
import { colors, fonts, formatBtc, glass, styles as s } from './theme';
import { JsonObject } from './transport';

// Navigation only. Rust remains responsible for proposal validation, eligibility, fees,
// signature checks and broadcast. No amounts or timelocks are constructed in this UI.
export function VaultSettings({ vault, go, backup, reviewRecovered, disabled, children }: {
  vault: JsonObject; go: (page: string) => void; backup: () => void;
  reviewRecovered: () => void; disabled: boolean; children?: React.ReactNode;
}) {
  return <View style={{ marginTop: 14 }}>
    <MenuRow title="Spending policy" subtitle="Monthly allowance and emergency access" onPress={() => go('policy')}/>
    <MenuRow title="Recovery backup" subtitle={vault.recovery_backup_stored ? 'Encrypted and saved for you' : 'Approve with your signer'} value={vault.recovery_backup_stored ? 'Saved' : 'Not set'} onPress={backup} disabled={disabled}/>
    <MenuRow title="Lost a device?" subtitle="What to do if your phone or signer goes missing" onPress={() => go('lost')}/>
    {vault.recovered_wallet_available && <MenuRow title="Review recovered wallet funds" subtitle="Move the old phone’s everyday balance" onPress={reviewRecovered} disabled={disabled}/>}
    <MenuRow title="Saved messages" subtitle="Resend a request as a file" onPress={() => go('saved-messages')}/>
    {children}
    <Text style={[s.footnote, { marginTop: 28, marginBottom: 6 }]}>{vault.config.network} · block {vault.chain_height.toLocaleString('en-US')}</Text>
  </View>;
}

export function WithdrawalOptions({ go, vault }: { go: (page: string) => void; vault: JsonObject }) {
  const schedule = vault.schedule;
  return <View style={{ gap: 10, marginTop: 22, marginHorizontal: 6, marginBottom: 4 }}>
    {!!schedule?.entries.length && <OptionCard title="Monthly allowance" value={`Up to ₿${formatBtc(vault.config.monthly_limit_sats)} a month`} detail="Phone only · your spending policy" icon="wallet" onPress={() => go('monthly')}/>}
    {schedule?.emergency_access && <OptionCard title="Emergency access" value={`Up to ₿${formatBtc(vault.config.emergency_access_limit_sats)}`} detail="Phone only · one-week delay" icon="phone" onPress={() => go('emergency')}/>}
    <OptionCard title="Move all vault funds" value="Any amount" detail="Immediately · phone + signer" icon="key" onPress={() => go('sweep')}/>
    {!schedule && <Text style={[s.tiny, { textAlign: 'center', marginTop: 12 }]}>Approve a policy with your signer to enable phone-only withdrawals.</Text>}
  </View>;
}

const toSats = (text: string) => /^\d*\.?\d{0,8}$/.test(text.trim()) && text.trim() !== '.' ? Math.round(Number(text.trim() || '0') * 100_000_000) : NaN;
const choice = (sats: number) => String(Math.floor(sats / 10_000) / 10_000);

function Chips({ options, value, onChange }: { options: { label: string; value: string }[]; value: string; onChange: (value: string) => void }) {
  return <View style={{ flexDirection: 'row', justifyContent: 'center', gap: 8, marginTop: 14 }}>
    {options.map(option => {
      const selected = toSats(option.value) === toSats(value);
      return <Pressable key={option.label} accessibilityRole="button" accessibilityLabel={option.label} aria-selected={selected} onPress={() => onChange(option.value)}
        style={({ pressed }) => ({ height: 32, paddingHorizontal: 14, borderRadius: 16, justifyContent: 'center', backgroundColor: selected ? 'rgba(255,255,255,.16)' : 'rgba(255,255,255,.06)', boxShadow: glass, transform: [{ scale: pressed ? .95 : 1 }] })}>
        <Text style={{ color: selected ? colors.text : colors.secondary, fontSize: 12, fontFamily: fonts.number, fontVariant: ['tabular-nums'] }}>{option.label}</Text>
      </Pressable>;
    })}
  </View>;
}

function Amount({ label, value, onChange, size }: { label: string; value: string; onChange: (value: string) => void; size: number }) {
  return <View style={{ flexDirection: 'row', justifyContent: 'center', alignItems: 'baseline', marginTop: 10 }}>
    <Text style={{ color: colors.quiet, fontSize: size * .7, fontFamily: fonts.number, marginRight: 3 }}>₿</Text>
    <TextInput accessibilityLabel={label} value={value} onChangeText={onChange} keyboardType="decimal-pad" placeholder="0" placeholderTextColor={colors.quiet} selectTextOnFocus
      style={{ width: Math.min(230, Math.max(1, (value || '0').length) * size * .6 + 6), textAlign: 'center', fontFamily: fonts.number, fontSize: size, fontWeight: '500', fontVariant: ['tabular-nums'], color: toSats(value) > 0 ? colors.text : colors.quiet, padding: 0 }}/>
  </View>;
}

// The two numbers a policy sets, in bitcoin, with suggestions sized to the vault. The signer
// shows them again for approval and Rust validates them; nothing here builds a transaction.
export function PolicyEditor({ vault, monthly, setMonthly, emergency, setEmergency, disabled, onPropose, onRenew }: {
  vault: JsonObject; monthly: string; setMonthly: (value: string) => void; emergency: string; setEmergency: (value: string) => void;
  disabled: boolean; onPropose: () => void; onRenew: () => void;
}) {
  const balance = vault.balance_sats, perMonth = toSats(monthly), reserve = toSats(emergency);
  const months = perMonth > 0 ? Math.floor(Math.max(0, balance - (reserve || 0)) / perMonth) : 12;
  const share = (fraction: number) => ({ label: `₿${formatBtc(Math.floor(balance * fraction / 10_000) * 10_000)}`, value: choice(balance * fraction) });
  return <View style={{ marginTop: 22 }}>
    <Text style={[s.tiny, { textAlign: 'center' }]}>Monthly allowance</Text>
    <Amount label="Monthly allowance in BTC" value={monthly} onChange={setMonthly} size={36}/>
    <Text style={[s.tiny, { textAlign: 'center', color: colors.quiet, marginTop: 4 }]}>{perMonth >= 0 ? `a month to your wallet · ₿${formatBtc(perMonth * 12)} a year` : 'Enter an amount in bitcoin'}</Text>
    {balance > 0 && <Chips options={[share(.02), share(.04), share(.08)]} value={monthly} onChange={setMonthly}/>}
    <View style={s.separator}/>
    <Text style={[s.tiny, { textAlign: 'center' }]}>Emergency access</Text>
    <Amount label="Emergency access in BTC" value={emergency} onChange={setEmergency} size={24}/>
    <Text style={[s.tiny, { textAlign: 'center', color: colors.quiet, marginTop: 4 }]}>{reserve > 0 ? 'Phone only, after a one-week delay' : 'Off'}</Text>
    {balance > 0 && <Chips options={[{ label: 'Off', value: '0' }, share(.1), share(.25)]} value={emergency} onChange={setEmergency}/>}
    {perMonth > 0 && months < 12 && <Text style={[s.errorText, { textAlign: 'center', marginTop: 16 }]}>Your vault covers {months} of 12 months at this amount.</Text>}
    <View style={{ alignItems: 'center', gap: 10, marginTop: 26 }}>
      <Pressable accessibilityRole="button" accessibilityLabel="Continue on signer" disabled={disabled} onPress={onPropose}
        style={({ pressed }) => ({ height: 46, paddingHorizontal: 28, borderRadius: 23, justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.14)', boxShadow: glass, opacity: disabled ? .4 : 1, transform: [{ scale: pressed ? .96 : 1 }] })}>
        <Text style={{ color: colors.text, fontSize: 13.5, fontWeight: '500' }}>Continue on signer</Text>
      </Pressable>
      {!!vault.schedule && <Pressable accessibilityRole="button" accessibilityLabel="Renew for another year" disabled={disabled} onPress={onRenew} style={({ pressed }) => ({ paddingVertical: 8, opacity: disabled ? .4 : pressed ? .6 : 1 })}>
        <Text style={{ color: colors.secondary, fontSize: 12.5 }}>Renew for another year</Text>
      </Pressable>}
    </View>
  </View>;
}
