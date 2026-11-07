import { test, expect } from '@playwright/test';
import { enrollRecoveryContact, policyDevices, recoverLatestBackupWithContact, openVaultPanel, closeVaultPanel, vaultAddress } from './helpers';
import { startTestHost } from './host';

let host: Awaited<ReturnType<typeof startTestHost>>;
test.beforeAll(async () => { host = await startTestHost(18891); });
test.afterAll(() => host?.stop());

test('contact recovery moves old hot funds immediately while the vault stays locked', async ({ page }) => {
  const { wallet, w, node, address, approve } = await policyDevices(page, host.url);
  await approve();
  const fingerprint = await enrollRecoveryContact(page);
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await w('↓ Receive').click();
  const oldHot = (await wallet.getByTestId('wallet-address').textContent())!;
  const fundOld = async (sats: number) => {
    await page.getByLabel('Funding address', { exact: true }).fill(oldHot);
    await page.getByLabel('Funding amount in sats', { exact: true }).fill(String(sats));
    await node('Fund address');
    await expect(page.getByText(new RegExp(`"amount_sats": ${sats}`))).toBeVisible();
  };
  await fundOld(1_000_000);
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click(); await w('Reveal vault').click();
  await openVaultPanel(wallet, 'Vault settings'); await w('Export encrypted cloud backup').click();
  await expect(wallet.getByText('Encrypted backup exported separately from the signer.')).toBeVisible();
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  // The helper creates a new wallet phone and restores only through the independent contact.
  await recoverLatestBackupWithContact(page, address, fingerprint);
  const recipient = await wallet.getByTestId('contact-recipient-identity').textContent();
  await page.reload(); await page.getByRole('button', { name: 'Resume playground', exact: true }).click();
  await w('Settings').click(); await w('Recover with a contact').click(); await w('Check stored contact recovery').click();
  await expect(wallet.getByTestId('contact-recipient-identity')).toHaveText(recipient!);
  await w('Check recovered vault balance and delay').click();
  const coldBalance = await wallet.getByText(/^[\d.]+ BTC recoverable$/).textContent();
  const remaining = Number((await wallet.getByTestId('contact-recovery-blocks').textContent())!.split(' ')[0]);
  expect(remaining).toBeGreaterThan(61_000);

  await w('Review recovered wallet funds').click();
  await w('Cancel recovered wallet transfer').click();
  await expect(w('Confirm recovered wallet transfer')).toHaveCount(0);
  let received = 0;
  for (const sats of [1_000_000, 200_000]) {
    if (sats === 200_000) await fundOld(sats);
    await w('Review recovered wallet funds').click();
    const summary = wallet.getByText(/^\d+ sats to this phone’s fresh wallet address\. Fee: \d+ sats\.$/);
    await expect(summary).toBeVisible();
    const [, amount, fee] = (await summary.textContent())!.match(/^(\d+) sats.*Fee: (\d+) sats\.$/)!;
    expect(Number(amount) + Number(fee)).toBe(sats);
    received += Number(amount);
    await w('Confirm recovered wallet transfer').click();
    await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
    await node('Mine confirmation');
    await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
    await expect(wallet.getByTestId('bitcoin-balance')).toHaveText(`₿${(received / 100_000_000).toLocaleString('en-US', { minimumFractionDigits: 4, maximumFractionDigits: 8 })}`, { timeout: 60_000 });
    await w('Settings').click(); await w('Recover with a contact').click(); await w('Check stored contact recovery').click();
  }
  await w('Check recovered vault balance and delay').click();
  await expect(wallet.getByText(coldBalance!, { exact: true })).toBeVisible();
  expect(Number((await wallet.getByTestId('contact-recovery-blocks').textContent())!.split(' ')[0])).toBeGreaterThan(61_000);
  await w('Review recovered wallet funds').click();
  await expect(wallet.getByRole('alert')).toContainText('no spendable funds');
  // The replacement signer was never needed: it has not been held to a phone.
  await expect(page.getByTestId('device-signer').getByText('Hold me near', { exact: false })).toBeVisible();
});
