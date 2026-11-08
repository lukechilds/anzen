import { test, expect } from '@playwright/test';
import { PNG } from 'pngjs';
import jsQR from 'jsqr';
import { closeVaultPanel, openVaultPanel, policyDevices, fundedText } from './helpers';
import { startTestHost } from './host';

let host: Awaited<ReturnType<typeof startTestHost>>;
test.beforeAll(async () => { host = await startTestHost(18892); });
test.afterAll(() => host?.stop());

test('Dark screens keep private savings separate and render real, copyable address QR codes', async ({ page, context }) => {
  const errors: string[] = [];
  page.on('pageerror', error => errors.push(error.message));
  const { wallet, signer, w, address, approve } = await policyDevices(page, host.url);
  await signer.screenshot({ path: test.info().outputPath('signer-policy.png') });
  const frame = (await wallet.boundingBox())!;
  expect(frame.width).toBe(375); expect(frame.height).toBe(812);
  await wallet.screenshot({ path: test.info().outputPath('vault-unfunded-policy.png') });
  await approve();
  await wallet.screenshot({ path: test.info().outputPath('vault.png') });

  const checkQR = async (expected: string) => {
    const png = PNG.sync.read(await wallet.getByTestId('receive-qr').screenshot());
    const qr = jsQR(new Uint8ClampedArray(png.data), png.width, png.height, { inversionAttempts: 'attemptBoth' });
    expect(qr?.data).toBe(`bitcoin:${expected}`);
    await context.grantPermissions(['clipboard-read', 'clipboard-write'], { origin: 'http://localhost:8081' });
    await w('Copy address').click();
    await expect(wallet.getByText('Copied', { exact: true })).toBeVisible();
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(expected);
  };
  await openVaultPanel(wallet, 'Vault deposit');
  await expect(wallet.getByTestId('vault-address')).toHaveText(address);
  await checkQR(address);
  await wallet.screenshot({ path: test.info().outputPath('vault-deposit.png') });
  await closeVaultPanel(wallet);
  await openVaultPanel(wallet, 'Monthly allowance');
  await page.keyboard.press('Escape');
  await expect(wallet.getByRole('dialog')).toHaveCount(0);
  await w('Settings').click();
  await expect(wallet.getByRole('dialog', { name: 'Vault settings', exact: true })).toBeVisible();
  await closeVaultPanel(wallet);
  await w('Hide vault').click();
  await expect(wallet.getByRole('tab', { name: 'Wallet', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(wallet.getByText('Signed policy', { exact: true })).toHaveCount(0);
  await expect(wallet.getByText(address, { exact: true })).toHaveCount(0);
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0000');
  await w('↓ Receive').click();
  const hot = (await wallet.getByTestId('wallet-address').textContent())!;
  expect(hot).not.toBe(address); await checkQR(hot);
  await wallet.screenshot({ path: test.info().outputPath('wallet-receive.png') });
  await closeVaultPanel(wallet);
  await w('↑ Send').click();
  await wallet.screenshot({ path: test.info().outputPath('wallet-send.png') });
  await closeVaultPanel(wallet);
  for (const width of [390, 320]) {
    await page.setViewportSize({ width, height: 844 });
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth)).toBe(true);
    await w('↓ Receive').click();
    const sheet = (await wallet.getByRole('dialog').boundingBox())!, device = (await wallet.boundingBox())!;
    expect(sheet.x).toBeGreaterThan(device.x);
    expect(sheet.x + sheet.width).toBeLessThan(device.x + device.width);
    await closeVaultPanel(wallet);
    await wallet.screenshot({ path: test.info().outputPath(`wallet-${width}.png`) });
  }
  expect(errors).toEqual([]);
});

test('Dark setup, privacy gate and activity use real wallet state', async ({ page }) => {
  await page.goto(host.url);  const wallet = page.getByTestId('device-wallet'), signer = page.getByTestId('device-signer');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  await wallet.screenshot({ path: test.info().outputPath('setup-wallet.png') });
  await signer.screenshot({ path: test.info().outputPath('setup-signer.png') });
  await w('Get started').click();
  await w('Settings').click();
  await wallet.screenshot({ path: test.info().outputPath('wallet-settings.png') });
  await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  // No vault yet: setup is offered instead. The signer is connected over Bluetooth, so setup
  // completes in one exchange with nothing to do on either device.
  await expect(w('Reveal vault')).toHaveCount(0);
  await wallet.screenshot({ path: test.info().outputPath('vault-setup.png') });
  await w('Set up vault').click();
  await expect(wallet.getByText('Your vault is ready', { exact: true })).toBeVisible();
  await wallet.screenshot({ path: test.info().outputPath('vault-ready.png') });
  await w('Done').click();
  await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click(); await w('↓ Receive').click();
  const address = (await wallet.getByTestId('wallet-address').textContent())!;
  await closeVaultPanel(wallet);
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByLabel('Funding amount in sats', { exact: true }).fill('1234567');
  await page.getByRole('button', { name: 'Fund address', exact: true }).click();
  await expect(page.getByText(fundedText(1234567), { exact: true })).toBeVisible({ timeout: 120_000 });
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.01234567', { timeout: 60_000 });
  await expect(wallet.getByText('Bitcoin received', { exact: true })).toBeVisible();
  await wallet.screenshot({ path: test.info().outputPath('wallet-activity.png') });
  await wallet.getByRole('button', { name: /^Transaction [a-f0-9]{64}$/ }).click();
  await expect(wallet.getByRole('dialog')).toContainText('+₿0.01234567');
  await wallet.screenshot({ path: test.info().outputPath('transaction-details.png') });
  await closeVaultPanel(wallet);
  await page.reload();
  // Reopening the wallet syncs by itself.
  await expect(wallet.getByText('Bitcoin received', { exact: true })).toBeVisible();
  await expect(w('Sync wallet')).toHaveCount(0);
});

test('wallet balance updates live on regtest without pressing sync', async ({ page }) => {
  await page.goto(host.url);  const wallet = page.getByTestId('device-wallet');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  await w('Get started').click();
  // Opening the wallet syncs by itself: the empty balance appears without a manual sync.
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0000', { timeout: 60_000 });
  await w('↓ Receive').click();
  const address = (await wallet.getByTestId('wallet-address').textContent())!;
  await closeVaultPanel(wallet);
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByLabel('Funding amount in sats', { exact: true }).fill('7654321');
  await page.getByRole('button', { name: 'Fund address', exact: true }).click();
  await expect(page.getByText(fundedText(7654321), { exact: true })).toBeVisible({ timeout: 120_000 });
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.07654321', { timeout: 60_000 });
  await expect(wallet.getByText('Bitcoin received', { exact: true })).toBeVisible();
});
