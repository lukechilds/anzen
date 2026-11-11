import React, { useState } from 'react';
import { ActivityIndicator, Pressable, View } from 'react-native';
import { Text } from './ui/Text';
import { colors, fonts, glass, styles as s } from './theme';

// Vanity-search benchmarks, on the GPU and on the CPU. Rust runs them on fixed public inputs (the
// GPU one with the same ramp guard as the desktop benchmark); nothing here touches keys or
// wallet state.
export type BenchmarkEngine = 'gpu' | 'cpu';
export interface BenchmarkResult {
  engine: BenchmarkEngine;
  // The GPU's name, or how many CPU threads ran.
  device: string;
  attempts: number; seconds: number; rate: number;
  // GPU only: opening the GPU and building its table, and the dispatch size the ramp reached.
  setup_seconds?: number; batch?: number;
}
export interface BenchmarkRunner {
  run(engine: BenchmarkEngine, seconds: number): Promise<BenchmarkResult>;
}

const engines: { engine: BenchmarkEngine; name: string }[] = [
  { engine: 'gpu', name: 'GPU' },
  { engine: 'cpu', name: 'CPU' },
];
const durations = [5, 30, 60];
// "vault" after the address prefix: five bech32 characters, 2^25 tries on average.
const VAULT_TRIES = 2 ** 25;

const rate = (value: number) => value >= 1e6 ? `${(value / 1e6).toFixed(2)}M/s` : `${Math.round(value / 1e3)}k/s`;
const time = (seconds: number) => seconds < 90 ? `${seconds.toFixed(seconds < 10 ? 1 : 0)} s` : `${(seconds / 60).toFixed(1)} min`;
const details = (result: BenchmarkResult) => [
  `“vault” in ~${time(VAULT_TRIES / result.rate)} on average`,
  result.setup_seconds === undefined ? result.device : `setup ${time(result.setup_seconds)}`,
  result.batch === undefined ? undefined : `batch ${result.batch.toLocaleString('en-US')}`,
].filter(Boolean).join(' · ');

// Results survive leaving and reopening settings while the app runs.
const results = new Map<BenchmarkEngine, BenchmarkResult | { error: string }>();

export default function Benchmarks({ runner }: { runner: BenchmarkRunner }) {
  const [seconds, setSeconds] = useState(5);
  const [running, setRunning] = useState<BenchmarkEngine | null>(null);
  const [, refresh] = useState(0);
  async function run(engine: BenchmarkEngine) {
    setRunning(engine);
    try { results.set(engine, await runner.run(engine, seconds)); }
    catch (error) { results.set(engine, { error: error instanceof Error ? error.message : String(error) }); }
    refresh(value => value + 1);
    setRunning(null);
  }
  const gpu = results.get('gpu');
  return <View testID="benchmarks" style={{ marginTop: 34 }}>
    <Text style={s.sheetEyebrow}>Vanity search benchmarks</Text>
    <View style={{ flexDirection: 'row', justifyContent: 'center', gap: 8, marginTop: 16 }}>
      {durations.map(value => <Pressable key={value} accessibilityRole="button" accessibilityLabel={`Run for ${value} seconds`} aria-selected={seconds === value}
        disabled={Boolean(running)} onPress={() => setSeconds(value)}
        style={({ pressed }) => ({ height: 30, paddingHorizontal: 13, borderRadius: 15, justifyContent: 'center', backgroundColor: seconds === value ? 'rgba(255,255,255,.16)' : 'rgba(255,255,255,.06)', boxShadow: glass, opacity: running ? .5 : 1, transform: [{ scale: pressed ? .95 : 1 }] })}>
        <Text style={{ color: seconds === value ? colors.text : colors.secondary, fontSize: 12, fontFamily: fonts.number, fontVariant: ['tabular-nums'] }}>{value} s</Text>
      </Pressable>)}
    </View>
    <View style={{ marginTop: 14 }}>
      {engines.map(({ engine, name }) => {
        const result = results.get(engine);
        return <View key={engine} testID={`benchmark-${engine}`} style={{ flexDirection: 'row', alignItems: 'center', gap: 12, paddingVertical: 14, borderBottomColor: colors.border, borderBottomWidth: 1 }}>
          <View style={{ flex: 1 }}>
            <Text style={{ color: colors.text, fontSize: 14, fontWeight: '500' }}>{name}</Text>
            {result && 'error' in result ? <Text selectable style={[s.errorText, { marginTop: 3 }]}>{result.error}</Text>
              : <Text style={[s.tiny, { color: colors.quiet, marginTop: 3 }]}>{result ? details(result) : 'Not run yet'}</Text>}
          </View>
          {running === engine ? <ActivityIndicator color={colors.mint}/>
            : result && 'rate' in result ? <Text testID={`benchmark-${engine}-rate`} style={{ color: colors.text, fontSize: 15, fontFamily: fonts.number, fontVariant: ['tabular-nums'] }}>{rate(result.rate)}</Text> : null}
          <Pressable accessibilityRole="button" accessibilityLabel={`Run the ${name} benchmark`} disabled={Boolean(running)} onPress={() => void run(engine)}
            style={({ pressed }) => ({ height: 30, paddingHorizontal: 13, borderRadius: 15, justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.08)', boxShadow: glass, opacity: running ? .4 : 1, transform: [{ scale: pressed ? .95 : 1 }] })}>
            <Text style={{ color: colors.text, fontSize: 12, fontWeight: '500' }}>Run</Text>
          </Pressable>
        </View>;
      })}
    </View>
    <Text style={[s.tiny, { color: colors.quiet, textAlign: 'center', marginTop: 8 }]}>
      GPU runs warm up for a second first. Keep the app open: phones only run the GPU in the foreground.
    </Text>
    {gpu && 'device' in gpu && <Text style={[s.footnote, { marginTop: 18 }]}>{gpu.device}</Text>}
  </View>;
}
