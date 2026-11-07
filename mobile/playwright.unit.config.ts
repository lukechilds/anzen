import { defineConfig } from '@playwright/test';

// Pure platform-adapter tests: no browser, Metro, Rust host or Bitcoin node needed.
export default defineConfig({ testDir: './tests/unit', workers: 1, timeout: 10_000 });
