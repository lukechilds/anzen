// GPU benchmarks in the browser: WebGPU through wgpu, in a worker of their own that ends with
// the run. Browsers have no native Metal, so only wgpu is offered.
import { BenchmarkResult, BenchmarkRunner } from './Benchmarks';

export const browserBenchmark: BenchmarkRunner = {
  backends: ['wgpu'],
  run: (_backend, kernel, seconds) => new Promise<BenchmarkResult>((resolve, reject) => {
    if (!('gpu' in navigator)) { reject(new Error('This browser has no WebGPU.')); return; }
    const worker = new Worker('/runtime/benchmark-worker.mjs', { type: 'module' });
    worker.onmessage = ({ data }) => {
      worker.terminate();
      if (data.error) reject(new Error(data.error)); else resolve(data.ok);
    };
    worker.onerror = event => { worker.terminate(); reject(new Error(event.message || 'The benchmark worker failed')); };
    worker.postMessage({ kernel, seconds });
  }),
};
