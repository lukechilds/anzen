import { test, expect } from '@playwright/test';
import { closeVaultPanel, openVaultPanel, policyDevices } from './helpers';
import { startTestHost } from './host';

let host: Awaited<ReturnType<typeof startTestHost>>;
test.beforeAll(async () => { host = await startTestHost(18893); });
test.afterAll(() => host?.stop());

for (const interruption of ['lost reply', 'page reload'] as const) {
  test(`WASM resumes the same approved policy after an accepted broadcast and ${interruption}`, async ({ page, context }) => {
    const { wallet, signer, w, s, node } = await policyDevices(page, host.url);
    let accepted!: () => void, release!: () => void;
    const acceptedBroadcast = new Promise<void>(resolve => { accepted = resolve; });
    const heldResponse = new Promise<void>(resolve => { release = resolve; });
    const transactions: string[] = [];
    await context.route('**/chain', async route => {
      const request = route.request().postDataJSON()?.request;
      if (request?.method !== 'broadcast') { await route.continue(); return; }
      transactions.push(JSON.stringify(request.transaction));
      if (transactions.length > 1) { await route.continue(); return; }
      // Bitcoin accepts the real signed transaction, but the WASM caller never receives
      // that success. A retry must use its persisted intent, not create another policy.
      const response = await route.fetch();
      expect(response.ok()).toBe(true);
      accepted();
      if (interruption === 'page reload') {
        await heldResponse;
        // Reload terminated the requesting worker; the connection may already be gone.
        await route.abort().catch(() => {});
      } else {
        await route.fulfill({ status: 503, contentType: 'application/json', body: JSON.stringify({ error: 'test: broadcast reply lost' }) });
      }
    });
    // Approving answers the phone at once, and it broadcasts.
    await s('Approve and sign policy').click(); await acceptedBroadcast;
    if (interruption === 'lost reply') await expect(wallet.getByRole('alert')).toContainText('test: broadcast reply lost');
    await page.reload(); release();
    await page.getByRole('button', { name: 'Resume playground', exact: true }).click();
    await expect(w('↓ Receive')).toBeVisible();
    expect(transactions).toHaveLength(1); // No automatic spending on reload.
    await node('Mine confirmation'); // Retry must also handle the transaction now being confirmed.
    await s('Saved messages').click();
    await signer.getByTestId('saved-message-policy-approved').getByRole('button').click();
    await expect(s('Approve and sign policy')).toHaveCount(0);
    await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
    await w('Open received message').click();
    await expect(wallet.getByText('Policy activated. Confirm its rollover with the regtest controls.')).toBeVisible();
    expect(new Set(transactions).size).toBe(1);
    await context.unroute('**/chain');
    await w('Reveal vault').click();
    await expect(wallet.getByText('−₿0.1000', { exact: true })).toBeVisible();
    await expect(wallet.getByText('≤ ₿0.5000', { exact: true })).toBeVisible();
    await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance');
    await w('Withdraw monthly allowance').click();
    await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
    await closeVaultPanel(wallet);
  });
}
