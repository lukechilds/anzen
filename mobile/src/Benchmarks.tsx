import React, { useState } from 'react';
import { ActivityIndicator, Pressable, Text, View } from 'react-native';
import { colors, fonts, glass, styles as s } from './theme';

// GPU vanity-search benchmarks. Rust runs them on fixed public inputs with the same ramp guard
// as the desktop benchmarks; nothing here touches keys or wallet state.
export type BenchmarkBackend = 'wgpu' | 'metal';
export type BenchmarkKernel = 'key' | 'nonce';
export interface BenchmarkResult {
  backend: BenchmarkBackend; kernel: BenchmarkKernel; adapter: string;
  setup_seconds: number; batch: number; attempts: number; seconds: number; rate: number;
}
export interface BenchmarkRunner {
  // What this platform can run: wgpu everywhere, and native Metal in the iOS app.
  backends: BenchmarkBackend[];
  run(backend: BenchmarkBackend, kernel: BenchmarkKernel, seconds: number): Promise<BenchmarkResult>;
}

const kernels: { kernel: BenchmarkKernel; name: string }[] = [
  { kernel: 'key', name: 'Key grinding' },
  { kernel: 'nonce', name: 'Script nonce grinding' },
];
const backendNames: Record<BenchmarkBackend, string> = { wgpu: 'wgpu', metal: 'Metal' };
const durations = [5, 30, 60];
// "vault" after the address prefix: five bech32 characters, 2^25 tries on average.
const VAULT_TRIES = 2 ** 25;

const rate = (value: number) => value >= 1e6 ? `${(value / 1e6).toFixed(2)}M/s` : `${Math.round(value / 1e3)}k/s`;
const time = (seconds: number) => seconds < 90 ? `${seconds.toFixed(seconds < 10 ? 1 : 0)} s` : `${(seconds / 60).toFixed(1)} min`;

// Results survive leaving and reopening settings while the app runs.
const results = new Map<string, BenchmarkResult | { error: string }>();

export default function Benchmarks({ runner }: { runner: BenchmarkRunner }) {
  const [seconds, setSeconds] = useState(5);
  const [running, setRunning] = useState<string | null>(null);
  const [, refresh] = useState(0);
  const variants = runner.backends.flatMap(backend => kernels.map(({ kernel, name }) => ({ backend, kernel, name, id: `${backend}-${kernel}` })));
  async function run(list: typeof variants) {
    for (const variant of list) {
      setRunning(variant.id);
      try { results.set(variant.id, await runner.run(variant.backend, variant.kernel, seconds)); }
      catch (error) { results.set(variant.id, { error: error instanceof Error ? error.message : String(error) }); }
      refresh(value => value + 1);
    }
    setRunning(null);
  }
  const adapter = [...results.values()].find((result): result is BenchmarkResult => 'adapter' in result)?.adapter;
  return <View testID="gpu-benchmarks" style={{ marginTop: 34 }}>
    <Text style={s.sheetEyebrow}>GPU benchmarks</Text>
    <View style={{ flexDirection: 'row', justifyContent: 'center', gap: 8, marginTop: 16 }}>
      {durations.map(value => <Pressable key={value} accessibilityRole="button" accessibilityLabel={`Run for ${value} seconds`} aria-selected={seconds === value}
        disabled={Boolean(running)} onPress={() => setSeconds(value)}
        style={({ pressed }) => ({ height: 30, paddingHorizontal: 13, borderRadius: 15, justifyContent: 'center', backgroundColor: seconds === value ? 'rgba(255,255,255,.16)' : 'rgba(255,255,255,.06)', boxShadow: glass, opacity: running ? .5 : 1, transform: [{ scale: pressed ? .95 : 1 }] })}>
        <Text style={{ color: seconds === value ? colors.text : colors.secondary, fontSize: 12, fontFamily: fonts.number, fontVariant: ['tabular-nums'] }}>{value} s</Text>
      </Pressable>)}
    </View>
    <View style={{ marginTop: 14 }}>
      {variants.map(variant => {
        const result = results.get(variant.id);
        return <View key={variant.id} testID={`benchmark-${variant.id}`} style={{ flexDirection: 'row', alignItems: 'center', gap: 12, paddingVertical: 14, borderBottomColor: colors.border, borderBottomWidth: 1 }}>
          <View style={{ flex: 1 }}>
            <Text style={{ color: colors.text, fontSize: 14, fontWeight: '500' }}>{backendNames[variant.backend]} · {variant.name}</Text>
            {result && 'error' in result ? <Text selectable style={[s.errorText, { marginTop: 3 }]}>{result.error}</Text>
              : result ? <Text style={[s.tiny, { color: colors.quiet, marginTop: 3 }]}>“vault” in ~{time(VAULT_TRIES / result.rate)} on average · setup {time(result.setup_seconds)} · batch {result.batch.toLocaleString('en-US')}</Text>
              : <Text style={[s.tiny, { color: colors.quiet, marginTop: 3 }]}>Not run yet</Text>}
          </View>
          {running === variant.id ? <ActivityIndicator color={colors.mint}/>
            : result && 'rate' in result ? <Text testID={`benchmark-${variant.id}-rate`} style={{ color: colors.text, fontSize: 15, fontFamily: fonts.number, fontVariant: ['tabular-nums'] }}>{rate(result.rate)}</Text> : null}
          <Pressable accessibilityRole="button" accessibilityLabel={`Run ${backendNames[variant.backend]} ${variant.name}`} disabled={Boolean(running)} onPress={() => void run([variant])}
            style={({ pressed }) => ({ height: 30, paddingHorizontal: 13, borderRadius: 15, justifyContent: 'center', backgroundColor: 'rgba(255,255,255,.08)', boxShadow: glass, opacity: running ? .4 : 1, transform: [{ scale: pressed ? .95 : 1 }] })}>
            <Text style={{ color: colors.text, fontSize: 12, fontWeight: '500' }}>Run</Text>
          </Pressable>
        </View>;
      })}
    </View>
    <Pressable accessibilityRole="button" accessibilityLabel="Run all benchmarks" disabled={Boolean(running)} onPress={() => void run(variants)}
      style={({ pressed }) => [s.button, { marginTop: 18, opacity: running ? .4 : pressed ? .7 : 1 }]}>
      <Text style={s.buttonText}>{running ? 'Running…' : 'Run all'}</Text>
    </Pressable>
    <Text style={[s.tiny, { color: colors.quiet, textAlign: 'center', marginTop: 8 }]}>
      Each run warms up for a second first. Keep the app open: the GPU only runs in the foreground.
    </Text>
    {adapter && <Text style={[s.footnote, { marginTop: 18 }]}>{adapter}</Text>}
  </View>;
}
