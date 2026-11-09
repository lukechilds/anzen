// Benchmarks in the browser, in a worker of their own that ends with the run: the GPU through
// WebGPU, or one CPU thread of WASM.
import { BenchmarkResult, BenchmarkRunner } from './Benchmarks';

export const browserBenchmark: BenchmarkRunner = {
  run: (engine, seconds) => new Promise<BenchmarkResult>((resolve, reject) => {
    if (engine === 'gpu' && !('gpu' in navigator)) { reject(new Error('This browser has no WebGPU.')); return; }
    const worker = new Worker('/runtime/benchmark-worker.mjs', { type: 'module' });
    worker.onmessage = ({ data }) => {
      worker.terminate();
      if (data.error) reject(new Error(data.error)); else resolve(data.ok);
    };
    worker.onerror = event => { worker.terminate(); reject(new Error(event.message || 'The benchmark worker failed')); };
    worker.postMessage({ engine, seconds });
  }),
};
