// GPU benchmarks run in their own short-lived worker: WebGPU readbacks are asynchronous, and the
// benchmark touches no device storage, so it never shares a device worker's blocking queue.
self.anzenBenchmarkPanic = message => self.postMessage({ error: message });
// wgpu cannot describe WebGPU's internal errors (it panics with "Unexpected error"), so report
// the browser's own message first.
if (self.GPUAdapter) {
  const requestDevice = GPUAdapter.prototype.requestDevice;
  GPUAdapter.prototype.requestDevice = async function (...args) {
    const device = await requestDevice.apply(this, args);
    device.addEventListener('uncapturederror', event => self.postMessage({ error: `WebGPU: ${event.error.message}` }));
    return device;
  };
}
self.onmessage = async ({ data }) => {
  try {
    const runtime = await import('../wasm/anzen_browser.js');
    await runtime.default();
    self.postMessage(JSON.parse(await runtime.benchmark(JSON.stringify(data))));
  } catch (error) {
    self.postMessage({ error: error?.message || String(error) });
  }
};
