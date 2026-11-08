import { test, expect, type Locator } from '@playwright/test';
import { policyDevices, openVaultPanel, openSignerRecovery, setUpVault, approveOnSigner, signerShows, useDevice } from './helpers';
import { startTestHost } from './host';

// Run after the ordinary wallet cases: these tests move the chain clock more than a year ahead.

let host: Awaited<ReturnType<typeof startTestHost>>;
test.beforeAll(async () => { host = await startTestHost(18889); });
test.afterAll(() => host?.stop());

// Retries until the refreshed timing replaces whatever the page showed before.
const locked = (wallet: Locator) => expect(wallet.getByTestId('source-hww-recovery-days')).toHaveText(/^38[89] days remaining · from /);

test('a missed renewal opens only the signer’s path, and a late renewal restarts it', async ({ page }) => {
  const { wallet, w, node, approve } = await policyDevices(page, host.url);
  await approve();
  await openSignerRecovery(wallet); await w('Check recovery timing').click();
  await locked(wallet);
  await node('Advance 13 months'); await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-hww-recovery-days')).toHaveText('Available now');
  // Nothing has moved: the owner still holds both keys and renews together, late.
  await openVaultPanel(wallet, 'Vault policy'); await w('Renew for another year').click();
  await signerShows(page, '₿0.1000 a month');
  await signerShows(page, 'Up to ₿0.5000');
  await approve(); await openSignerRecovery(wallet); await w('Check recovery timing').click();
  await locked(wallet);
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click(); await expect(wallet.getByRole('alert')).toBeVisible();
  await openSignerRecovery(wallet);
  await w('Use this wallet’s fresh address').click();
  await expect(wallet.getByLabel('Recovery destination', { exact: true })).toHaveValue(/^bcrt1p/);
  await w('Request HWW fallback').click();
  await approveOnSigner(page, 'Approve offline action');
  await expect(wallet.getByRole('alert')).toContainText('HWW recovery is not mature');
  await w('Cancel').click();
});

test('losing both devices and the signer’s own backup leaves the savings locked for good', async ({ page }) => {
  const { wallet, signer, w, node, approve } = await policyDevices(page, host.url);
  await approve();
  await openVaultPanel(wallet, 'Vault settings'); await w('Recovery backup').click(); await approveOnSigner(page, 'Approve recovery backup'); await expect(wallet.getByText('Recovery backup saved.')).toBeVisible();
  const originalSigner = new URLSearchParams(new URL(page.url()).hash.slice(1)).get('signer')!;
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  await w('Get started').click(); await setUpVault(page, wallet);
  const freshSigner = new URLSearchParams(new URL(page.url()).hash.slice(1)).get('signer')!;
  // Public vault details are assumed known; the original signer shares only those, then is gone.
  await useDevice(page, 'signer', originalSigner);
  await openSignerRecovery(wallet); await w('Load vault from signer').click();
  await approveOnSigner(page, 'Share vault details');
  await useDevice(page, 'signer', freshSigner);
  await page.getByRole('button', { name: 'Load latest exported backup', exact: true }).click();
  const refused = async () => {
    await w('Request phone key recovery').click();
    await expect(signer.getByRole('alert')).toBeVisible(); await expect(signer.getByText('Unlock phone recovery', { exact: true })).toHaveCount(0);
    await w('Cancel').click(); await w('Check restored key').click(); await expect(wallet.getByRole('alert')).toContainText('failed to read');
    await w('Use this wallet’s fresh address').click();
    await expect(wallet.getByLabel('Recovery destination', { exact: true })).toHaveValue(/^bcrt1p/);
    await w('Request HWW fallback').click();
    await expect(signer.getByRole('alert')).toContainText('request does not belong to the paired vault');
    await expect(signer.getByText(/^(Recover vault funds|Revoke all vault actions)$/)).toHaveCount(0);
    await w('Cancel').click();
  };
  await refused();
  await node('Advance 13 months'); await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-hww-recovery-days')).toHaveText('Available now');
  // Only the original signer's key could use it; a different signer still cannot.
  await refused();
  await w('Check recovery timing').click();
  await expect(wallet.getByText(/^2\.[\d]+ BTC in the recovery vault$/)).toBeVisible();
});
