import { test, expect } from '@playwright/test';
import { policyDevices, openVaultPanel, closeVaultPanel, vaultAddress, vaultCaughtUp, setUpVault, declinedOnSigner, recoverWithSigner, openSignerRecovery, approveOnSigner, signerShows, fundedText, useDevice, deviceId } from './helpers';
import { startTestHost } from './host';
import { readdir } from 'node:fs/promises';

let host: Awaited<ReturnType<typeof startTestHost>>;
let url: string;
test.beforeAll(async () => {
  host = await startTestHost(18887);
  url = host.url;
});
test.afterAll(() => host?.stop());

test('two device screens pair, fund, sign, activate, enforce a timelock and withdraw', async ({ page }) => {
  const errors: string[] = [];
  const backendRequests: { path: string; body: string }[] = [];
  page.context().on('request', request => {
    if (new URL(request.url()).port === new URLSearchParams(new URL(url).hash.slice(1)).get('port')) {
      backendRequests.push({ path: new URL(request.url()).pathname, body: request.postData() || '' });
    }
  });
  page.on('pageerror', error => errors.push(error.message));
  await page.goto(url);
  const wallet = page.getByTestId('device-wallet');
  const signer = page.getByTestId('device-signer');
  const walletButton = (name: string) => wallet.getByRole('button', { name, exact: true });
  const signerButton = (name: string) => signer.getByRole('button', { name, exact: true });
  // Setup is one tap: no messages to carry, and the recovery backup is saved to the cloud.
  await walletButton('Get started').click();
  await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await walletButton('Set up vault').click();
 
  await expect(wallet.getByText('Your vault is ready', { exact: true })).toBeVisible();
  await signerShows(page, 'Ready.');
  await expect(page.getByRole('button', { name: 'Load latest exported backup', exact: true })).toBeVisible();
  await walletButton('Done').click();
  const address = await vaultAddress(wallet);
  await expect(signer.getByTestId('signer-vault-address')).toHaveText(address);
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByRole('button', { name: 'Fund address', exact: true }).click();
  await expect(page.getByText(fundedText(210000000), { exact: true })).toBeVisible({ timeout: 120_000 });
  await expect(wallet.getByText('₿2.1000', { exact: true })).toBeVisible({ timeout: 60_000 });
  await openVaultPanel(wallet, 'Vault settings'); await walletButton('Recovery backup').click();
 
  await signerShows(page, 'Protect phone recovery');
  await approveOnSigner(page, 'Approve recovery backup');
  await expect(wallet.getByText('Recovery backup saved.')).toBeVisible();
  await openVaultPanel(wallet, 'Vault policy');
  await wallet.getByLabel('Monthly allowance in BTC', { exact: true }).fill('0.1');
  await wallet.getByLabel('Emergency access in BTC', { exact: true }).fill('0.5');
  await walletButton('Continue on signer').click();
 
  await signerShows(page, 'Annual vault policy');
  await signerShows(page, '₿0.1000 a month');
  await signerShows(page, 'Up to ₿0.5000');
  await approveOnSigner(page, 'Approve and sign policy');
  await expect(wallet.getByText('Allowance approved.')).toBeVisible();
  await page.getByRole('button', { name: 'Mine confirmation', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Mine confirmation', exact: true })).toBeEnabled();
  await closeVaultPanel(wallet); await vaultCaughtUp(page, wallet);
  await openVaultPanel(wallet, 'Monthly allowance'); await walletButton('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
  await page.getByRole('button', { name: 'Advance 31 days', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Advance 31 days', exact: true })).toBeEnabled();
  await openVaultPanel(wallet, 'Monthly allowance'); await walletButton('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
  await page.getByRole('button', { name: 'Mine confirmation', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Mine confirmation', exact: true })).toBeEnabled();
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await expect(wallet.getByText('₿0.1000', { exact: true })).toBeVisible({ timeout: 60_000 });
  await expect(wallet.getByText(/Protected by two independent keys|Vault policy/)).toHaveCount(0);
  await walletButton('↑ Send').click();
  await wallet.getByLabel('Destination address', { exact: true }).fill(address!);
  await wallet.getByLabel('Amount in sats', { exact: true }).fill('50000');
  await walletButton('Review payment').click();
  await expect(wallet.getByText('50000 sats', { exact: true })).toBeVisible();
  await expect(wallet.getByText(/^Fee: \d+ sats · Total: \d+ sats$/)).toBeVisible();
  await walletButton('Confirm and send').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
  await expect(walletButton('↓ Receive')).toBeEnabled();
  await expect(wallet.getByText('₿0.1000', { exact: true })).toHaveCount(0);
  await page.setViewportSize({ width: 390, height: 844 });
  await expect.poll(async () => {
    const amount = await wallet.getByTestId('bitcoin-balance').boundingBox();
    const frame = await wallet.boundingBox();
    return Boolean(amount && frame && amount.x >= frame.x && amount.x + amount.width <= frame.x + frame.width);
  }).toBe(true);
  await page.screenshot({ path: 'test-results/playground-funded-narrow.png', fullPage: true });
  await page.setViewportSize({ width: 1440, height: 1100 });
  // A late private-data response must not silently undo a user's Hide Vault/navigation action.
  let release!: () => void;
  let observed!: () => void;
  const hold = new Promise<void>(resolve => { release = resolve; });
  const requested = new Promise<void>(resolve => { observed = resolve; });
  await page.context().route('**/chain', async route => {
    if (route.request().postDataJSON()?.request?.method === 'scan_vault') {
      const response = await route.fetch();
      observed();
      await hold;
      await route.fulfill({ response });
    } else await route.continue();
  });
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await requested;
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  release();
  await expect(walletButton('↓ Receive')).toBeEnabled();
  // The late result is dropped: the wallet stays on screen and the vault stays closed.
  await expect(wallet.getByRole('tab', { name: 'Wallet', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(wallet.getByText('Monthly allowance', { exact: true })).toHaveCount(0);
  await expect(wallet.getByTestId('device-screen')).toHaveAttribute('data-vault-height', '');
  await page.context().unroute('**/chain');
  await page.screenshot({ path: 'test-results/playground-signed.png', fullPage: true });
  await page.reload();
 
  await expect(walletButton('Get started')).toHaveCount(0);
  await expect(walletButton('↓ Receive')).toBeVisible();
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  expect(await vaultAddress(wallet)).toBe(address);
  expect(errors).toEqual([]);
  expect(backendRequests.some(request => request.path === '/chain')).toBe(true);
  expect(backendRequests.some(request => ['/device', '/devices', '/devices/list'].includes(request.path))).toBe(false);
  for (const request of backendRequests) {
    expect(request.body).not.toMatch(/"(?:mnemonic|seed|private_key|symmetric_key)"\s*:|[xt]prv[1-9A-HJ-NP-Za-km-z]{50}/);
  }
  const hostFiles = await readdir(host.directory, { recursive: true });
  expect(hostFiles.some(file => /(?:phone|hww|device\.json|hot-wallet)/.test(file))).toBe(false);
});

test('the emulator reopens where it left off and asks once before switching to real bitcoin', async ({ page }) => {
  await page.goto(url);
  const wallet = page.getByTestId('device-wallet');
  // Opened from the host's link, the emulator starts on regtest with a fresh pair.
  await expect(page.getByRole('tab', { name: 'Regtest', exact: true })).toHaveAttribute('aria-selected', 'true');
  await wallet.getByRole('button', { name: 'Get started', exact: true }).click();
  const phone = await deviceId(page, 'wallet');
  expect(phone).toBeTruthy();
  // Only the host's capability and port stay in the URL.
  expect([...new URLSearchParams(new URL(page.url()).hash.slice(1)).keys()].sort()).toEqual(['playground', 'port']);
  await page.reload();
  await expect(wallet.getByTestId('bitcoin-balance')).toBeVisible();
  expect(await deviceId(page, 'wallet')).toBe(phone);
  // Mainnet asks first, and declining stays put.
  await page.getByRole('tab', { name: 'Mainnet', exact: true }).click();
  await expect(page.getByText('Use real bitcoin?', { exact: true })).toBeVisible();
  await page.getByRole('button', { name: 'Cancel', exact: true }).click();
  await expect(page.getByRole('tab', { name: 'Regtest', exact: true })).toHaveAttribute('aria-selected', 'true');
  await expect(wallet.getByTestId('bitcoin-balance')).toBeVisible();
  // Start over replaces the pair.
  await page.getByRole('button', { name: 'Start over', exact: true }).click();
  await expect(wallet.getByRole('button', { name: 'Get started', exact: true })).toBeVisible();
  expect(await deviceId(page, 'wallet')).not.toBe(phone);
});

test('monthly withdrawal applies a zero soft limit and returns funds to the vault', async ({ page }) => {
  const { wallet, w, node, approve } = await policyDevices(page, url, { emergency: '0' });
  await approve();
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
  await node('Mine confirmation');
  await w('More options').click();
  await wallet.getByLabel('Soft limit in sats', { exact: true }).fill('10000001');
  await w('Apply soft limit').click();
  await expect(wallet.getByRole('alert')).toContainText('soft limit cannot exceed');
  await wallet.getByLabel('Soft limit in sats', { exact: true }).fill('0');
  await w('Apply soft limit').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
  await node('Mine confirmation');
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await expect(wallet.getByText('₿0.0000', { exact: true })).toBeVisible({ timeout: 60_000 });
});

test('phone revokes monthly withdrawals before they become available', async ({ page }) => {
  const { wallet, w, node, approve } = await policyDevices(page, url, { emergency: '0' });
  await approve();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Revoke remaining allowances').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
  await node('Mine confirmation'); await node('Advance 31 days');
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
});

for (const cancel of [false, true]) {
  test(cancel ? 'phone cancels initiated emergency access' : 'emergency access unlocks only after one week', async ({ page }) => {
    const { wallet, w, node, approve } = await policyDevices(page, url);
    await approve(); await openVaultPanel(wallet, 'Emergency access'); await w('Start emergency access').click();
    await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
    await node('Mine confirmation'); await openVaultPanel(wallet, 'Emergency access'); await w('Withdraw emergency funds').click();
    await expect(wallet.getByRole('alert')).toBeVisible();
    if (cancel) {
      await openVaultPanel(wallet, 'Emergency access'); await w('Cancel emergency access').click();
      await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
      await node('Mine confirmation');
    }
    await node('Advance 1 week'); await openVaultPanel(wallet, 'Emergency access'); await w('Withdraw emergency funds').click();
    if (cancel) await expect(wallet.getByRole('alert')).toBeVisible();
    else {
      await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
      await node('Mine confirmation');
      await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
      await expect(wallet.getByText('₿0.5000', { exact: true })).toBeVisible({ timeout: 60_000 });
    }
  });
}

test('partial funding is explicitly reviewed and only the funded withdrawal exists', async ({ page }) => {
  const { wallet, signer, w, approve } = await policyDevices(page, url, { funding: '15000000', emergency: '0' });
  await signerShows(page, 'Only 1 of 12 monthly withdrawals can be funded. The policy funds the earliest withdrawals first.');
  await approve();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('More options').click(); await wallet.getByLabel('Allowance step', { exact: true }).fill('2');
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toContainText('no allowance exists for step 2');
});

test('on-time renewal preserves a non-default policy and resets the monthly delay', async ({ page }) => {
  const { wallet, signer, w, s, node, approve } = await policyDevices(page, url, { monthly: '0.08', emergency: '0.25' });
  await approve();
  // Advance close to the annual renewal date, well before either block-based fallback.
  for (let month = 0; month < 11; month++) await node('Advance 31 days');
  await openVaultPanel(wallet, 'Vault policy'); await w('Renew for another year').click();
  await signerShows(page, '₿0.0800 a month');
  await signerShows(page, 'Up to ₿0.2500');
  await approve();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
});

test('signer reviews public recovery requests, rejects early fallback, and revokes both actions', async ({ page }) => {
  await page.goto(url);
  const wallet = page.getByTestId('device-wallet');
  const signer = page.getByTestId('device-signer');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  const s = (name: string) => signer.getByRole('button', { name, exact: true });
  await w('Get started').click(); await setUpVault(page, wallet);
  const address = await vaultAddress(wallet);
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByRole('button', { name: 'Fund address', exact: true }).click();
  await expect(page.getByText(fundedText(210000000), { exact: true })).toBeVisible({ timeout: 120_000 });
  await vaultCaughtUp(page, wallet); await openVaultPanel(wallet, 'Vault policy');
  await wallet.getByLabel('Monthly allowance in BTC', { exact: true }).fill('0.1');
  await wallet.getByLabel('Emergency access in BTC', { exact: true }).fill('0.5');
  await w('Continue on signer').click(); await approveOnSigner(page, 'Approve and sign policy');
  await expect(wallet.getByText('Allowance approved.')).toBeVisible();
  await page.getByRole('button', { name: 'Mine confirmation', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Mine confirmation', exact: true })).toBeEnabled();
  await closeVaultPanel(wallet); await vaultCaughtUp(page, wallet);
  const originalPhone = await deviceId(page, 'wallet');
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await w('Get started').click();
  await recoverWithSigner(page); await w('Use this wallet’s fresh address').click();
  await w('Request HWW fallback').click();
  await signerShows(page, /Bitcoin accepts this only 388 days after each input confirmed/);
  await approveOnSigner(page, 'Approve offline action');
  await expect(wallet.getByRole('alert')).toContainText('HWW recovery is not mature');
  await w('Cancel').click();
  await w('Request all-action revocation').click();
  await signerShows(page, 'Revoke all vault actions');
  await s('Reject offline action').click(); await declinedOnSigner(page);
  await expect(signer.getByText(/^(Recover vault funds|Revoke all vault actions)$/)).toHaveCount(0);
  await w('Request all-action revocation').click(); await approveOnSigner(page, 'Approve offline action');
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
  await page.getByRole('button', { name: 'Mine confirmation', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Mine confirmation', exact: true })).toBeEnabled();
  // Return to the unavailable/stolen original phone: its retained presigned actions are revoked.
  await useDevice(page, 'wallet', originalPhone);
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await page.getByRole('button', { name: 'Advance 31 days', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Advance 31 days', exact: true })).toBeEnabled();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
  await openVaultPanel(wallet, 'Emergency access'); await w('Start emergency access').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
});

test('a fresh phone restores, rotates and preserves policy with explicit signer approval', async ({ page }) => {
  const { wallet, signer, w, s, node, address, approve } = await policyDevices(page, url, { monthly: '0.08', emergency: '0.25' });
  await approve();
  const originalPhone = await deviceId(page, 'wallet');
  // Distinct from the cold vault, these funds must also survive losing the original phone.
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await w('↓ Receive').click();
  const hotAddress = (await wallet.getByTestId('wallet-address').textContent())!;
  await page.getByLabel('Funding address', { exact: true }).fill(hotAddress);
  await page.getByLabel('Funding amount in sats', { exact: true }).fill('1000000');
  await node('Fund address');
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await openVaultPanel(wallet, 'Vault settings'); await w('Recovery backup').click(); await approveOnSigner(page, 'Approve recovery backup'); await expect(wallet.getByText('Recovery backup saved.')).toBeVisible();
  const savedBackup = page.getByRole('button', { name: 'Load latest exported backup', exact: true });
  await expect(savedBackup).toBeVisible();
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await w('Get started').click();
  await recoverWithSigner(page); await savedBackup.click();
  await w('Check restored key').click();
  await expect(wallet.getByRole('alert')).toContainText('failed to read');
  await w('Request phone key recovery').click();
  await signerShows(page, 'Unlock phone recovery');
  await signerShows(page, /^[a-f0-9]{64}$/);
  const newIdentity = (await signer.getByText(/^[a-f0-9]{64}$/).allTextContents())[0];
  await expect(wallet.getByText(newIdentity, { exact: true })).toBeVisible();
  await s('Reject phone recovery').click(); await declinedOnSigner(page);
  await expect(signer.getByText('Unlock phone recovery', { exact: true })).toHaveCount(0);
  await w('Request phone key recovery').click(); await approveOnSigner(page, 'Approve phone recovery');
  await expect(wallet.getByText('Old phone key recovered into encrypted storage. Your new wallet key is unchanged.')).toBeVisible();
  await expect(wallet.getByText(/^Old key recovered: .*Rotation is still required/)).toBeVisible();
  await w('Check restored key').click();
  await expect(wallet.getByText(newIdentity, { exact: true })).toBeVisible();
  // Recovery must survive losing transient React state, with no access to the old phone.
  await page.reload();
  await recoverWithSigner(page); await w('Check restored key').click();
  await expect(wallet.getByText(/^Old key recovered: .*Rotation is still required/)).toBeVisible();
  await w('Propose restored vault rotation').click();
  await signerShows(page, 'Rotate the recovered vault');
  await signerShows(page, /Preserve 0.0800 BTC monthly and 0.2500 BTC emergency/);
  await signerShows(page, /Revoke the old connectors and return their change to the new vault/);
  await page.screenshot({ path: 'test-results/playground-rotation-review.png', fullPage: true });
  await s('Reject phone rotation').click(); await declinedOnSigner(page);
  await expect(signer.getByText('Rotate the recovered vault', { exact: true })).toHaveCount(0);
  await w('Propose restored vault rotation').click(); await approveOnSigner(page, 'Approve phone rotation');
  await expect(wallet.getByText('Vault rotation broadcast. Once it confirms, finish on your signer.')).toBeVisible();
  await node('Mine confirmation');
  await w('Finish on signer').click();
  await approveOnSigner(page, 'Use the rotated vault');
  await expect(signer.getByText('Signer paired with the rotated vault. The spending limits are preserved.')).toBeVisible();
  await expect(wallet.getByText('−₿0.0800', { exact: true })).toBeVisible();
  await expect(wallet.getByText('≤ ₿0.2500', { exact: true })).toBeVisible();
  await expect(wallet.getByText(address, { exact: true })).toHaveCount(0);
  await vaultAddress(wallet);
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
  await node('Mine confirmation');
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await expect(wallet.getByText('₿0.0800', { exact: true })).toBeVisible({ timeout: 60_000 });
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await openVaultPanel(wallet, 'Vault settings');
  await w('Review recovered wallet funds').click();
  await expect(wallet.getByText('Recover the old wallet balance', { exact: true })).toBeVisible();
  await w('Cancel recovered wallet transfer').click();
  await expect(w('Confirm recovered wallet transfer')).toHaveCount(0);
  await w('Review recovered wallet funds').click(); await w('Confirm recovered wallet transfer').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible(); await node('Mine confirmation');
  await useDevice(page, 'wallet', originalPhone);
  await expect(wallet.getByText('₿0.0000', { exact: true })).toBeVisible({ timeout: 60_000 });
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await expect(wallet.getByText('₿0.0000', { exact: true })).toBeVisible();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click(); await expect(wallet.getByRole('alert')).toBeVisible();
  await openVaultPanel(wallet, 'Emergency access'); await w('Start emergency access').click(); await expect(wallet.getByRole('alert')).toBeVisible();
});

test('a signer protecting another vault refuses setup, leaving the phone free to use another', async ({ page }) => {
  await page.goto(url);
  const wallet = page.getByTestId('device-wallet');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  await w('Get started').click(); await setUpVault(page, wallet);
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await w('Get started').click(); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await w('Set up vault').click();
  await expect(wallet.getByRole('alert')).toHaveText('This signer already protects another vault.');
  await w('Cancel').click();
  // Nothing was paired: the phone still offers a fresh setup, which works with a new signer.
  await expect(w('Set up vault')).toBeVisible(); await expect(w('Finish setup')).toHaveCount(0);
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  await setUpVault(page, wallet);
  await expect(w('Deposit')).toBeVisible();
});

test('narrow playground remains usable without horizontal scrolling', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  await expect(page.getByTestId('device-wallet')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: 'test-results/playground-narrow.png', fullPage: true });
});

test('a lost signer is restored from its own backup, so the vault never changes signer', async ({ page }) => {
  const { wallet, w, approve } = await policyDevices(page, url);
  await approve();
  await openVaultPanel(wallet, 'Lost a device');
  await expect(wallet.getByText('Lost your signer?', { exact: true })).toBeVisible();
  await expect(wallet.getByText(/^Restore it from the backup its maker gave you\. It comes back as the same key/)).toBeVisible();
  await expect(wallet.getByText('Lost your phone?', { exact: true })).toBeVisible();
  await expect(wallet.getByText(/a year after your last renewal\.$/)).toBeVisible();
  await expect(w('Replace lost signer')).toHaveCount(0);
  await expect(w('Recover without signer')).toHaveCount(0);
  await page.screenshot({ path: 'test-results/playground-lost-device.png', fullPage: true });
});

// Keep the 13-month jump last: it moves this host's chain clock past the signer's time lock.
test('without the phone or its cloud backup, the signer alone recovers after 13 months', async ({ page }) => {
  const { wallet, signer, w, s, node, approve } = await policyDevices(page, url);
  await approve();
  // The new phone never loads the cloud backup: recovery here relies on the signer alone.
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click(); await w('Get started').click();
  await recoverWithSigner(page);
  await expect(wallet.getByLabel('Encrypted cloud backup JSON', { exact: true })).toHaveValue('');
  await expect(w('Request phone key recovery')).toBeDisabled();
  await expect(wallet.getByText(/^No cloud backup loaded\./)).toBeVisible();
  await w('Use this wallet’s fresh address').click();
  await expect(wallet.getByLabel('Recovery destination', { exact: true })).toHaveValue(/^bcrt1p/);
  const destination = await wallet.getByLabel('Recovery destination', { exact: true }).inputValue();
  await w('Check recovery timing').click();
  const days = Number((await wallet.getByTestId('source-hww-recovery-days').textContent())!.split(' ')[0]);
  expect(days).toBeGreaterThanOrEqual(388); expect(days).toBeLessThanOrEqual(389);
  await w('Request HWW fallback').click();
  await signerShows(page, destination);
  await signerShows(page, /Bitcoin accepts this only 388 days after each input confirmed/);
  await s('Reject offline action').click(); await declinedOnSigner(page); await expect(signer.getByText(/^(Recover vault funds|Revoke all vault actions)$/)).toHaveCount(0);
  await w('Request HWW fallback').click();
  const amount = (await signer.getByTestId('offline-action-amount').textContent())!.replace('₿', '');
  await approveOnSigner(page, 'Approve offline action');
  await expect(wallet.getByRole('alert')).toContainText('HWW recovery is not mature');
  await w('Cancel').click();
  // A year passes without a renewal. The approval the phone kept is now accepted.
  await node('Advance 13 months');
  await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-hww-recovery-days')).toHaveText('Available now');
  await w('Finish').click();
  await expect(wallet.getByText(/^Sent · [a-f0-9]{8}$/)).toBeVisible();
  await node('Mine confirmation'); await w('Check recovery timing').click();
  await expect(wallet.getByText('No confirmed vault outputs remain.', { exact: true })).toBeVisible();
  await w('Check restored key').click(); await expect(wallet.getByRole('alert')).toContainText('failed to read');
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText(`₿${amount}`, { timeout: 60_000 });
});
