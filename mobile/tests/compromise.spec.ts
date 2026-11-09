import { test, expect } from '@playwright/test';
import { policyDevices, openVaultPanel, closeVaultPanel, setUpVault, declinedOnSigner, recoverWithSigner, openSignerRecovery, approveOnSigner, signerShows, useDevice, deviceId } from './helpers';
import { startTestHost } from './host';

let host: Awaited<ReturnType<typeof startTestHost>>;
test.beforeAll(async () => { host = await startTestHost(18888); });
test.afterAll(() => host?.stop());

test('both compromised devices can jointly drain the vault immediately', async ({ page }) => {
  const { wallet, signer, w, s, node, approve } = await policyDevices(page, host.url);
  await approve();
  const original = await deviceId(page, 'wallet');
  // A separate phone supplies the attacker's destination. The original pair represents
  // both compromised keys; no private state is copied into the receiving device.
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click(); await w('Get started').click();
  const receiver = await deviceId(page, 'wallet');
  await w('↓ Receive').click();
  const destination = (await wallet.getByTestId('wallet-address').textContent())!;
  await useDevice(page, 'wallet', original);
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await openVaultPanel(wallet, 'Move vault funds');
  await wallet.getByLabel('Vault transfer destination', { exact: true }).fill(destination);
  await w('Propose vault transfer').click();
  await signerShows(page, destination);
  await signerShows(page, /^₿[\d,.]+$/);
  const amount = (await signer.getByText(/^₿[\d,.]+$/).textContent())!.replace('₿', '');
  await approveOnSigner(page, 'Approve vault transfer');
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible(); await node('Mine confirmation');
  await closeVaultPanel(wallet); await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0000', { timeout: 60_000 });
  await useDevice(page, 'wallet', receiver);
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText(`₿${amount}`, { timeout: 60_000 });
});

test('cloud ciphertext and public vault data cannot authorize unrelated attacker keys', async ({ page }) => {
  const { wallet, signer, w, s, approve } = await policyDevices(page, host.url);
  await approve();
  await openVaultPanel(wallet, 'Vault settings'); await w('Recovery backup').click(); await approveOnSigner(page, 'Approve recovery backup'); await expect(wallet.getByText('Recovery backup saved.')).toBeVisible();
  const originalSigner = await deviceId(page, 'signer');
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  await w('Get started').click(); await setUpVault(page, wallet);
  const attackerSigner = await deviceId(page, 'signer');
  // Public vault information is assumed exposed, as in the CLI cloud-compromise test.
  // The original signer only shares existing public information; it approves no new action.
  await useDevice(page, 'signer', originalSigner);
  await openSignerRecovery(wallet); await w('Load vault from signer').click();
  await approveOnSigner(page, 'Share vault details');
  await useDevice(page, 'signer', attackerSigner);
  await page.getByRole('button', { name: 'Load latest exported backup', exact: true }).click();
  await w('Request phone key recovery').click();
  await expect(signer.getByRole('alert')).toBeVisible();
  await expect(signer.getByText('Unlock phone recovery', { exact: true })).toHaveCount(0);
  await expect(wallet.getByText('Couldn’t finish', { exact: true })).toBeVisible(); await w('Cancel').click();
  await w('Check restored key').click(); await expect(wallet.getByRole('alert')).toContainText('failed to read');
  await w('Use this wallet’s fresh address').click();
  await expect(wallet.getByLabel('Recovery destination', { exact: true })).toHaveValue(/^bcrt1p/);
  await w('Request HWW fallback').click();
  await expect(signer.getByRole('alert')).toContainText('request does not belong to the paired vault');
  await expect(signer.getByText(/^(Recover vault funds|Revoke all vault actions)$/)).toHaveCount(0);
  await w('Cancel').click();
  await w('Check recovery timing').click();
  await expect(wallet.getByText(/^2\.[\d]+ BTC in the recovery vault$/)).toBeVisible();
});

test('a stolen phone exposes hot funds but the owner can rotate cold funds away', async ({ page }) => {
  const { wallet, signer, w, s, node, approve } = await policyDevices(page, host.url);
  await approve();
  const stolenPhone = await deviceId(page, 'wallet');
  await openVaultPanel(wallet, 'Vault settings'); await w('Recovery backup').click(); await approveOnSigner(page, 'Approve recovery backup'); await expect(wallet.getByText('Recovery backup saved.')).toBeVisible();
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click(); await w('↓ Receive').click();
  const hotAddress = (await wallet.getByTestId('wallet-address').textContent())!;
  await page.getByLabel('Funding address', { exact: true }).fill(hotAddress);
  await page.getByLabel('Funding amount in sats', { exact: true }).fill('1000000');
  await node('Fund address');

  // The attacker's receiving phone and the owner's replacement are separate devices.
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click(); await w('Get started').click();
  const attacker = await deviceId(page, 'wallet');
  await w('↓ Receive').click();
  const destination = (await wallet.getByTestId('wallet-address').textContent())!;
  await useDevice(page, 'wallet', stolenPhone);
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0100', { timeout: 60_000 });
  await w('↑ Send').click(); await wallet.getByLabel('Destination address', { exact: true }).fill(destination);
  await wallet.getByLabel('Amount in sats', { exact: true }).fill('500000');
  await w('Review payment').click(); await w('Confirm and send').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible(); await node('Mine confirmation');
  // The phone has no way to move savings alone: the thief can only ask the signer, which refuses.
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await openVaultPanel(wallet, 'Lost a device');
  await expect(w('Recover without signer')).toHaveCount(0);
  await openVaultPanel(wallet, 'Move vault funds');
  await wallet.getByLabel('Vault transfer destination', { exact: true }).fill(destination);
  await w('Propose vault transfer').click(); await s('Reject vault transfer').click(); await declinedOnSigner(page);
  await expect(signer.getByText('Move vault funds', { exact: true })).toHaveCount(0);
  await closeVaultPanel(wallet); await expect(wallet.getByTestId('bitcoin-balance')).toHaveText(/^₿2\./, { timeout: 60_000 });

  // The owner recovers using the surviving HWW and exported ciphertext, without opening
  // the compromised phone or approving its proposed destination.
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click(); await w('Get started').click();
  const owner = await deviceId(page, 'wallet');
  await recoverWithSigner(page);
  await page.getByRole('button', { name: 'Load latest exported backup', exact: true }).click();
  await w('Request phone key recovery').click();
  await signerShows(page, 'Unlock phone recovery');
  await signerShows(page, /^[a-f0-9]{64}$/);
  const recipient = (await signer.getByText(/^[a-f0-9]{64}$/).allTextContents())[0];
  await expect(wallet.getByText(recipient, { exact: true })).toBeVisible();
  await approveOnSigner(page, 'Approve phone recovery');
  await w('Propose restored vault rotation').click();
  await signerShows(page, /Preserve 0.1000 BTC monthly and 0.5000 BTC emergency/);
  await approveOnSigner(page, 'Approve phone rotation');
  await expect(wallet.getByText('Vault rotation broadcast. Once it confirms, finish on your signer.')).toBeVisible();
  await node('Mine confirmation'); await w('Finish on signer').click();
  await approveOnSigner(page, 'Use the rotated vault');
  await expect(wallet.getByText('−₿0.1000', { exact: true })).toBeVisible();
  await expect(wallet.getByText('≤ ₿0.5000', { exact: true })).toBeVisible();
  await openVaultPanel(wallet, 'Vault settings');
  await w('Review recovered wallet funds').click(); await w('Confirm recovered wallet transfer').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible(); await node('Mine confirmation');
  await node('Advance 31 days');
  await useDevice(page, 'wallet', stolenPhone);
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0000', { timeout: 60_000 });
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0000');
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click(); await expect(wallet.getByRole('alert')).toBeVisible();
  await openVaultPanel(wallet, 'Emergency access'); await w('Start emergency access').click(); await expect(wallet.getByRole('alert')).toBeVisible();
  // Rotating protects the remaining funds; it cannot undo the already-confirmed theft.
  await useDevice(page, 'wallet', attacker);
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText('₿0.0050', { timeout: 60_000 });
  await useDevice(page, 'wallet', owner);
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText(/^₿2\./);
});
