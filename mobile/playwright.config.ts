import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: './tests',
  testIgnore: ['**/unit/**', '**/static-browser.spec.ts'],
  workers: 1,
  timeout: 180_000,
  expect: { timeout: 30_000 },
  use: {
    // Long mining/sync waits have explicit budgets. A missing UI control should fail
    // promptly rather than consuming a recovery test's entire 20-minute allowance.
    actionTimeout: 30_000,
    viewport: { width: 1440, height: 1100 },
    screenshot: 'only-on-failure',
    trace: 'retain-on-failure',
    launchOptions: process.env.ANZEN_CHROMIUM_PATH ? { executablePath: process.env.ANZEN_CHROMIUM_PATH } : {},
  },
  webServer: { command: 'npm run web -- --localhost', url: 'http://localhost:8081', reuseExistingServer: true, timeout: 60_000 },
});
