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
    <PolicyRow title={vault.schedule ? 'Annual renewal' : 'Set monthly allowance'} detail={vault.schedule ? 'Renew with your signer' : 'Approve with your signer'} icon="clock" onPress={onPolicy}/>
    {vault.schedule?.emergency_access && <PolicyRow title="Emergency access" detail="Phone only · 1 week delay" value={`≤ ₿${formatBtc(vault.config.emergency_access_limit_sats)}`} icon="phone" onPress={onEmergency}/>}
    {!!vault.schedule?.entries.length && <PolicyRow title="Monthly allowance" detail={`${vault.schedule.entries.length} presigned withdrawals`} value={`−₿${formatBtc(vault.config.monthly_limit_sats)}`} icon="wallet" onPress={onMonthly}/>}
    {!vault.schedule && <Text style={[d.subtitle, { textAlign: 'center', marginVertical: 20, lineHeight: 19 }]}>{vault.balance_sats > 0 ? 'Choose how much reaches your wallet each month.\nYour signer approves it once a year.' : 'Deposit savings first, then choose\nyour monthly allowance.'}</Text>}
    <Text style={[d.subtitle, { textAlign: 'center', marginTop: 14, lineHeight: 18 }]}>{'Bitcoin checks timelocks and revocations\nwhen you execute a withdrawal.'}</Text>
  </View>;
}
