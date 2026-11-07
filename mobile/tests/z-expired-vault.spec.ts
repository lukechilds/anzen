import { test, expect, Page } from '@playwright/test';
import { policyDevices, openVaultPanel, openSignerRecovery, holdSignerToPhone, setUpVault } from './helpers';
import { startTestHost } from './host';

// Run after the ordinary wallet cases: these tests deliberately add 130k real blocks.

let host: Awaited<ReturnType<typeof startTestHost>>;
test.beforeAll(async () => { host = await startTestHost(18889); });
test.afterAll(() => host?.stop());

async function mineDelay(page: Page, blocks: number) {
  await page.getByLabel('Recovery mining blocks', { exact: true }).fill(String(blocks));
  await page.getByRole('button', { name: 'Start recovery mining', exact: true }).click();
  await expect(page.getByText(`${blocks} / ${blocks} real blocks`, { exact: true })).toBeVisible({ timeout: 1_200_000 });
  await expect(page.getByText('Mining · completed', { exact: true })).toBeVisible();
}

test('forgotten rollover exposes both recovery paths and late renewal resets both delays', async ({ page }) => {
  test.setTimeout(1_800_000);
  const { wallet, signer, w, s, node, approve } = await policyDevices(page, host.url);
  await approve();
  for (let month = 0; month < 16; month++) await node('Advance 31 days');
  await openSignerRecovery(wallet); await w('Check recovery timing').click();
  const phoneDelay = Number((await wallet.getByTestId('source-phone-recovery-blocks').textContent())!.split(' ')[0]);
  const hwwDelay = Number((await wallet.getByTestId('source-hww-recovery-blocks').textContent())!.split(' ')[0]);
  expect(phoneDelay).toBeGreaterThan(61_000); expect(hwwDelay - phoneDelay).toBe(4_335);
  await mineDelay(page, phoneDelay); await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-phone-recovery-blocks')).toHaveText('0 blocks remaining');
  await expect(wallet.getByTestId('source-hww-recovery-blocks')).toHaveText('4335 blocks remaining');
  await mineDelay(page, 4_335); await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-phone-recovery-blocks')).toHaveText('0 blocks remaining');
  await expect(wallet.getByTestId('source-hww-recovery-blocks')).toHaveText('0 blocks remaining');
  // Neither path is spent: the user still has both keys and can renew cooperatively.
  await openVaultPanel(wallet, 'Vault policy'); await w('Renew for another year').click(); await holdSignerToPhone(page);
  await expect(signer.getByText('₿0.1000 a month', { exact: true })).toBeVisible();
  await expect(signer.getByText('Up to ₿0.5000', { exact: true })).toBeVisible();
  await approve(); await openSignerRecovery(wallet); await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-phone-recovery-blocks')).toHaveText('61199 blocks remaining');
  await expect(wallet.getByTestId('source-hww-recovery-blocks')).toHaveText('65534 blocks remaining');
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click(); await expect(wallet.getByRole('alert')).toBeVisible();
  await openSignerRecovery(wallet);
  await w('Use this wallet’s fresh address').click();
  await expect(wallet.getByLabel('Recovery destination', { exact: true })).toHaveValue(/^bcrt1p/);
  const destination = await wallet.getByLabel('Recovery destination', { exact: true }).inputValue();
  await openVaultPanel(wallet, 'Phone recovery');
  await wallet.getByLabel('Vault transfer destination', { exact: true }).fill(destination);
  await w('Confirm phone recovery transfer').click();
  await expect(wallet.getByRole('alert')).toContainText('no vault UTXOs are mature');
  await w('Cancel vault transfer').click();
  await openSignerRecovery(wallet);
  await w('Request HWW fallback').click(); await holdSignerToPhone(page);
  await s('Approve offline action').click();
  await expect(wallet.getByRole('alert')).toContainText('HWW recovery is not mature');
  await w('Cancel').click();
});

test('both devices lost without a recovery contact stays unrecoverable after both deadlines', async ({ page }) => {
  test.setTimeout(1_800_000);
  const { wallet, signer, w, s, approve } = await policyDevices(page, host.url);
  await approve();
  await openVaultPanel(wallet, 'Vault settings'); await w('Create encrypted phone backup').click(); await holdSignerToPhone(page); await s('Approve recovery backup').click(); await expect(wallet.getByText('Encrypted cloud backup stored locally. The signer can recover the phone key.')).toBeVisible();
  await openVaultPanel(wallet, 'Vault settings'); await w('Export encrypted cloud backup').click();
  await expect(wallet.getByText('Encrypted backup exported separately from the signer.')).toBeVisible();
  const originalSigner = new URLSearchParams(new URL(page.url()).hash.slice(1)).get('signer')!;
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  await w('Create wallet').click(); await setUpVault(page, wallet);
  const freshSigner = new URLSearchParams(new URL(page.url()).hash.slice(1)).get('signer')!;
  // Share existing public information only, then set aside both original devices. Neither
  // private key signs any recovery request, and no contact was enrolled in the original backup.
  await page.getByRole('button', { name: `Signer ${originalSigner.slice(0, 6)}`, exact: true }).click();
  await openSignerRecovery(wallet); await w('Load vault from signer').click(); await holdSignerToPhone(page);
  await s('Share vault details').click();
  await page.getByRole('button', { name: `Signer ${freshSigner.slice(0, 6)}`, exact: true }).click();
  await page.getByRole('button', { name: 'Load latest exported backup', exact: true }).click();
  const rejectRecovery = async () => {
    await w('Request phone key recovery').click(); await holdSignerToPhone(page);
    await expect(signer.getByRole('alert')).toBeVisible(); await expect(s('Approve phone recovery')).toHaveCount(0);
    await w('Cancel').click(); await w('Check restored key').click(); await expect(wallet.getByRole('alert')).toContainText('failed to read');
    await expect(w('Propose restored vault rotation')).toHaveCount(0);
  };
  await rejectRecovery();
  await w('Check recovery timing').click();
  const remaining = Number((await wallet.getByTestId('source-hww-recovery-blocks').textContent())!.split(' ')[0]);
  expect(remaining).toBeGreaterThan(65_000);
  await mineDelay(page, remaining); await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-phone-recovery-blocks')).toHaveText('0 blocks remaining');
  await expect(wallet.getByTestId('source-hww-recovery-blocks')).toHaveText('0 blocks remaining');
  await rejectRecovery();
  await w('Use this wallet’s fresh address').click();
  await expect(wallet.getByLabel('Recovery destination', { exact: true })).toHaveValue(/^bcrt1p/);
  await w('Request HWW fallback').click(); await holdSignerToPhone(page);
  await expect(signer.getByRole('alert')).toContainText('request does not belong to the paired vault');
  await expect(s('Approve offline action')).toHaveCount(0);
  await w('Cancel').click(); await w('Check recovery timing').click();
  await expect(wallet.getByText(/^2\.[\d]+ BTC in the recovery vault$/)).toBeVisible();
});
