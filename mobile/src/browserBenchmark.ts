// The GPU benchmark in the browser: WebGPU through wgpu, in a worker of its own that ends with
// the run.
import { BenchmarkResult, BenchmarkRunner } from './Benchmarks';

export const browserBenchmark: BenchmarkRunner = {
  run: seconds => new Promise<BenchmarkResult>((resolve, reject) => {
    if (!('gpu' in navigator)) { reject(new Error('This browser has no WebGPU.')); return; }
    const worker = new Worker('/runtime/benchmark-worker.mjs', { type: 'module' });
    worker.onmessage = ({ data }) => {
      worker.terminate();
      if (data.error) reject(new Error(data.error)); else resolve(data.ok);
    };
    worker.onerror = event => { worker.terminate(); reject(new Error(event.message || 'The benchmark worker failed')); };
    worker.postMessage({ seconds });
  }),
};
