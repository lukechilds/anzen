import { defineConfig } from '@playwright/test';
import common from './playwright.config';

const url = `http://localhost:${process.env.ANZEN_STATIC_PORT || 8083}`;
export default defineConfig({
  ...common,
  testMatch: 'static-browser.spec.ts',
  testIgnore: '**/unit/**',
  use: { ...common.use, baseURL: url },
  webServer: { command: 'npm run preview:web', url, reuseExistingServer: false, timeout: 15_000 },
});
