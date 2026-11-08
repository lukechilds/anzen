import React from 'react';
import { Pressable, Text, View } from 'react-native';
import { Icon, IconName } from './DarkChrome';
import { d } from './DarkSurfaces';
import { colors, formatBtc } from './theme';
import { JsonObject } from './transport';

function PolicyRow({ title, detail, value, icon, onPress }: {
  title: string; detail: string; value?: string; icon: IconName; onPress: () => void;
}) {
  return <Pressable accessibilityRole="button" accessibilityLabel={title} onPress={onPress} style={[d.transaction, d.pending]}>
    <View style={d.avatar}><Icon name={icon} size={16}/></View>
    <View style={{ flex: 1 }}><Text style={d.title}>{title}</Text><Text style={d.subtitle}>{detail}</Text></View>
    {value && <Text style={[d.value, { color: colors.quiet }]}>{value}</Text>}
  </Pressable>;
}

// Presentation of the Rust snapshot only. A saved schedule is not proof that an action is
// currently spendable: the Rust application checks chain state again for every execution.
export default function VaultSummary({ vault, onMonthly, onEmergency, onPolicy }: {
  vault: JsonObject; onMonthly: () => void; onEmergency: () => void; onPolicy: () => void;
}) {
  return <View style={d.list}>
    {/* Nothing to set until there are savings to protect. */}
    {(vault.schedule || vault.balance_sats > 0) && <PolicyRow title={vault.schedule ? 'Annual renewal' : 'Set monthly allowance'} detail={vault.schedule ? 'Renew with your signer' : 'Approve with your signer'} icon="clock" onPress={onPolicy}/>}
    {vault.schedule?.emergency_access && <PolicyRow title="Emergency access" detail="Phone only · 1 week delay" value={`≤ ₿${formatBtc(vault.config.emergency_access_limit_sats)}`} icon="phone" onPress={onEmergency}/>}
    {!!vault.schedule?.entries.length && <PolicyRow title="Monthly allowance" detail={`${vault.schedule.entries.length} presigned withdrawals`} value={`−₿${formatBtc(vault.config.monthly_limit_sats)}`} icon="wallet" onPress={onMonthly}/>}
  </View>;
}
