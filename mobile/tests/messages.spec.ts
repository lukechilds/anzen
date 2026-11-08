import { test, expect } from '@playwright/test';
import { policyDevices, openVaultPanel, closeVaultPanel, vaultAddress, approveOnSigner, signerShows } from './helpers';
import { startTestHost } from './host';

let host: Awaited<ReturnType<typeof startTestHost>>;
test.beforeAll(async () => { host = await startTestHost(18890); });
test.afterAll(() => host?.stop());

test('saved requests and approvals survive reload and can be carried as files', async ({ page }) => {
  const { wallet, signer, w, s, node } = await policyDevices(page, host.url);
  await signerShows(page, 'Annual vault policy');
  // Lose both devices' in-memory request/review presentation before approving anything.
  await page.reload();
  await expect(w('Saved messages')).toHaveCount(0);
  await expect(signer.getByText('Annual vault policy', { exact: true })).toHaveCount(0);
  await s('Saved messages').click();
  await expect(signer.getByTestId('saved-message-policy-approved')).toHaveCount(0);
  await s('Close saved messages').click();

  await openVaultPanel(wallet, 'Saved messages');
  const proposal = wallet.getByTestId('saved-message-policy-proposal');
  await expect(proposal).toHaveCount(1);
  await proposal.getByRole('button').click();
  await expect(wallet.getByText('Copy shared.')).toBeVisible();
  await expect(signer.getByText('Annual vault policy', { exact: true })).toHaveCount(0);
  await s('Open received message').click();
  await signerShows(page, '₿0.1000 a month');
  await s('Reject policy').click();
  await proposal.getByRole('button').click(); await s('Open received message').click();
  await approveOnSigner(page, 'Approve and sign policy');
  await expect(s('Send to wallet')).toBeVisible();

  // Lose the signed response before sharing it. Restore its exact saved copy instead
  // of presenting another approval screen or silently broadcasting on resume.
  await page.reload();
  await expect(s('Send to wallet')).toHaveCount(0);
  await expect(w('Open received message')).toHaveCount(0);
  await s('Saved messages').click();
  const approval = signer.getByTestId('saved-message-policy-approved');
  await expect(approval).toHaveCount(1);
  await approval.getByRole('button').scrollIntoViewIfNeeded();
  await page.screenshot({ path: test.info().outputPath('saved-messages.png'), fullPage: true });
  await approval.getByRole('button').click();
  await expect(signer.getByText('Annual vault policy', { exact: true })).toHaveCount(0);
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await w('Open received message').click();
  await expect(wallet.getByText('Allowance approved.')).toBeVisible();
  await node('Mine confirmation');
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();

  // Message history is private vault context, never retained on the spending screen.
  await openVaultPanel(wallet, 'Saved messages'); await expect(proposal).toBeVisible();
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await expect(wallet.getByTestId('saved-messages')).toHaveCount(0);
  await expect(proposal).toHaveCount(0);
});
