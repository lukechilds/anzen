import React, { useState } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { colors, fonts, glass, styles as s } from './theme';

// The GPU vanity-search benchmark. Rust runs it on fixed public inputs with the same ramp guard
// as the desktop benchmark; nothing here touches keys or wallet state.
export interface BenchmarkResult {
  adapter: string; setup_seconds: number; batch: number; attempts: number; seconds: number; rate: number;
}
export interface BenchmarkRunner {
  run(seconds: number): Promise<BenchmarkResult>;
}

const durations = [5, 30, 60];
// "vault" after the address prefix: five bech32 characters, 2^25 tries on average.
const VAULT_TRIES = 2 ** 25;

const rate = (value: number) => value >= 1e6 ? `${(value / 1e6).toFixed(2)}M/s` : `${Math.round(value / 1e3)}k/s`;
const time = (seconds: number) => seconds < 90 ? `${seconds.toFixed(seconds < 10 ? 1 : 0)} s` : `${(seconds / 60).toFixed(1)} min`;

// The last result survives leaving and reopening settings while the app runs.
let last: BenchmarkResult | { error: string } | undefined;

export default function Benchmarks({ runner }: { runner: BenchmarkRunner }) {
  const [seconds, setSeconds] = useState(5);
  const [running, setRunning] = useState(false);
  const [result, setResult] = useState(last);
  async function run() {
    setRunning(true);
    try { last = await runner.run(seconds); }
    catch (error) { last = { error: error instanceof Error ? error.message : String(error) }; }
    setResult(last);
    setRunning(false);
  }
  return <View testID="gpu-benchmark" style={{ marginTop: 34 }}>
    <Text style={s.sheetEyebrow}>GPU benchmark</Text>
    <View style={{ flexDirection: 'row', justifyContent: 'center', gap: 8, marginTop: 16 }}>
      {durations.map(value => <Pressable key={value} accessibilityRole="button" accessibilityLabel={`Run for ${value} seconds`} aria-selected={seconds === value}
        disabled={running} onPress={() => setSeconds(value)}
        style={({ pressed }) => ({ height: 30, paddingHorizontal: 13, borderRadius: 15, justifyContent: 'center', backgroundColor: seconds === value ? 'rgba(255,255,255,.16)' : 'rgba(255,255,255,.06)', boxShadow: glass, opacity: running ? .5 : 1, transform: [{ scale: pressed ? .95 : 1 }] })}>
        <Text style={{ color: seconds === value ? colors.text : colors.secondary, fontSize: 12, fontFamily: fonts.number, fontVariant: ['tabular-nums'] }}>{value} s</Text>
      </Pressable>)}
    </View>
    <View style={{ flexDirection: 'row', alignItems: 'center', gap: 12, paddingVertical: 14, marginTop: 14, borderBottomColor: colors.border, borderBottomWidth: 1 }}>
      <View style={{ flex: 1 }}>
        <Text style={{ color: colors.text, fontSize: 14, fontWeight: '500' }}>Vanity address search</Text>
        {result && 'error' in result ? <Text selectable style={[s.errorText, { marginTop: 3 }]}>{result.error}</Text>
          : result ? <Text style={[s.tiny, { color: colors.quiet, marginTop: 3 }]}>“vault” in ~{time(VAULT_TRIES / result.rate)} on average · setup {time(result.setup_seconds)} · batch {result.batch.toLocaleString('en-US')}</Text>
          : <Text style={[s.tiny, { color: colors.quiet, marginTop: 3 }]}>Not run yet</Text>}
      </View>
      {running ? <ActivityIndicator color={colors.mint}/>
        : result && 'rate' in result ? <Text testID="gpu-benchmark-rate" style={{ color: colors.text, fontSize: 15, fontFamily: fonts.number, fontVariant: ['tabular-nums'] }}>{rate(result.rate)}</Text> : null}
      <Pressable accessibilityRole="button" accessibilityLabel="Run the GPU benchmark" disabled={running} onPress={() => void run()}
        style={({ pressed }) => ({ height: 30, paddingHorizontal: 13, borderRadius: 15, justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.08)', boxShadow: glass, opacity: running ? .4 : 1, transform: [{ scale: pressed ? .95 : 1 }] })}>
        <Text style={{ color: colors.text, fontSize: 12, fontWeight: '500' }}>Run</Text>
      </Pressable>
    </View>
    <Text style={[s.tiny, { color: colors.quiet, textAlign: 'center', marginTop: 8 }]}>
      Each run warms up for a second first. Keep the app open: the GPU only runs in the foreground.
    </Text>
    {result && 'adapter' in result && <Text style={[s.footnote, { marginTop: 18 }]}>{result.adapter}</Text>}
  </View>;
}
