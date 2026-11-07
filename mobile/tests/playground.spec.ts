import { test, expect } from '@playwright/test';
import { policyDevices, enrollRecoveryContact, recoverLatestBackupWithContact, openVaultPanel, closeVaultPanel, vaultAddress, openRetiredVaults, vaultCaughtUp, holdSignerToPhone, setUpVault, declinedOnSigner, recoverWithSigner, openSignerRecovery } from './helpers';
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
  await page.getByRole('button', { name: 'Start playground', exact: true }).click();
  const wallet = page.getByTestId('device-wallet');
  const signer = page.getByTestId('device-signer');
  const walletButton = (name: string) => wallet.getByRole('button', { name, exact: true });
  const signerButton = (name: string) => signer.getByRole('button', { name, exact: true });
  // Setup is one tap: no messages to carry, and the recovery backup is saved to the cloud.
  await walletButton('Create wallet').click();
  await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await walletButton('Set up vault').click();
  await expect(wallet.getByText('Hold your signer to your phone', { exact: true })).toBeVisible();
  await holdSignerToPhone(page);
  await expect(wallet.getByText('Your vault is ready', { exact: true })).toBeVisible();
  await expect(signer.getByText('Ready.', { exact: true })).toBeVisible();
  await expect(page.getByRole('button', { name: 'Load latest exported backup', exact: true })).toBeVisible();
  await walletButton('Done').click();
  await walletButton('Reveal vault').click();
  const address = await vaultAddress(wallet);
  await expect(signer.getByTestId('signer-vault-address')).toHaveText(address);
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByRole('button', { name: 'Fund address', exact: true }).click();
  await expect(page.getByText(/"amount_sats": 210000000/)).toBeVisible({ timeout: 120_000 });
  await expect(wallet.getByText('₿2.1000', { exact: true })).toBeVisible({ timeout: 60_000 });
  await openVaultPanel(wallet, 'Vault settings'); await walletButton('Create encrypted phone backup').click();
  await holdSignerToPhone(page);
  await expect(signer.getByText('Protect phone recovery', { exact: true })).toBeVisible();
  await signerButton('Approve recovery backup').click();
  await expect(wallet.getByText('Encrypted cloud backup stored locally. The signer can recover the phone key.')).toBeVisible();
  await openVaultPanel(wallet, 'Vault policy');
  await wallet.getByLabel('Monthly allowance in BTC', { exact: true }).fill('0.1');
  await wallet.getByLabel('Emergency access in BTC', { exact: true }).fill('0.5');
  await walletButton('Continue on signer').click();
  await holdSignerToPhone(page);
  await expect(signer.getByText('Annual vault policy', { exact: true })).toBeVisible();
  await expect(signer.getByText('₿0.1000 a month', { exact: true })).toBeVisible();
  await expect(signer.getByText('Up to ₿0.5000', { exact: true })).toBeVisible();
  await signerButton('Approve and sign policy').click();
  await expect(wallet.getByText('Policy activated. Confirm its rollover with the regtest controls.')).toBeVisible();
  await page.getByRole('button', { name: 'Mine confirmation', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Mine confirmation', exact: true })).toBeEnabled();
  await closeVaultPanel(wallet); await vaultCaughtUp(page, wallet);
  await openVaultPanel(wallet, 'Monthly allowance'); await walletButton('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
  await page.getByRole('button', { name: 'Advance 31 days', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Advance 31 days', exact: true })).toBeEnabled();
  await openVaultPanel(wallet, 'Monthly allowance'); await walletButton('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
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
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
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
  await walletButton('Reveal vault').click();
  await requested;
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  release();
  await expect(walletButton('↓ Receive')).toBeEnabled();
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await expect(walletButton('Reveal vault')).toBeVisible();
  await expect(wallet.getByText('Vault policy', { exact: true })).toHaveCount(0);
  await page.context().unroute('**/chain');
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await page.screenshot({ path: 'test-results/playground-signed.png', fullPage: true });
  await page.reload();
  await page.getByRole('button', { name: 'Resume playground', exact: true }).click();
  await expect(walletButton('Create wallet')).toHaveCount(0);
  await expect(walletButton('↓ Receive')).toBeVisible();
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await expect(walletButton('Reveal vault')).toBeVisible();
  await walletButton('Reveal vault').click();
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

test('browser mainnet selection still requires host-side opt-in', async ({ page }) => {
  await page.goto(url);
  await page.getByRole('button', { name: 'Dangerously enable mainnet', exact: true }).click();
  await page.getByRole('button', { name: 'Start playground', exact: true }).click();
  const wallet = page.getByTestId('device-wallet');
  await wallet.getByRole('button', { name: 'Create wallet', exact: true }).click();
  await expect(wallet.getByRole('alert')).toContainText('mainnet requires explicit');
  await expect(page.getByRole('button', { name: 'Fund address', exact: true })).toHaveCount(0);
});

test('monthly withdrawal applies a zero soft limit and returns funds to the vault', async ({ page }) => {
  const { wallet, w, node, approve } = await policyDevices(page, url, { emergency: '0' });
  await approve();
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
  await node('Mine confirmation');
  await wallet.getByLabel('Soft limit in sats', { exact: true }).fill('10000001');
  await w('Apply soft limit').click();
  await expect(wallet.getByRole('alert')).toContainText('soft limit cannot exceed');
  await wallet.getByLabel('Soft limit in sats', { exact: true }).fill('0');
  await w('Apply soft limit').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
  await node('Mine confirmation');
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await expect(wallet.getByText('₿0.0000', { exact: true })).toBeVisible({ timeout: 60_000 });
});

test('phone revokes monthly withdrawals before they become available', async ({ page }) => {
  const { wallet, w, node, approve } = await policyDevices(page, url, { emergency: '0' });
  await approve();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Revoke remaining allowances').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
  await node('Mine confirmation'); await node('Advance 31 days');
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
});

for (const cancel of [false, true]) {
  test(cancel ? 'phone cancels initiated emergency access' : 'emergency access unlocks only after one week', async ({ page }) => {
    const { wallet, w, node, approve } = await policyDevices(page, url);
    await approve(); await openVaultPanel(wallet, 'Emergency access'); await w('Start emergency access').click();
    await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
    await node('Mine confirmation'); await openVaultPanel(wallet, 'Emergency access'); await w('Withdraw emergency funds').click();
    await expect(wallet.getByRole('alert')).toBeVisible();
    if (cancel) {
      await openVaultPanel(wallet, 'Emergency access'); await w('Cancel emergency access').click();
      await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
      await node('Mine confirmation');
    }
    await node('Advance 1 week'); await openVaultPanel(wallet, 'Emergency access'); await w('Withdraw emergency funds').click();
    if (cancel) await expect(wallet.getByRole('alert')).toBeVisible();
    else {
      await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
      await node('Mine confirmation');
      await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
      await expect(wallet.getByText('₿0.5000', { exact: true })).toBeVisible({ timeout: 60_000 });
    }
  });
}

test('partial funding is explicitly reviewed and only the funded withdrawal exists', async ({ page }) => {
  const { wallet, signer, w, approve } = await policyDevices(page, url, { funding: '15000000', emergency: '0' });
  await expect(signer.getByText('Only 1 of 12 monthly withdrawals can be funded. The policy funds the earliest withdrawals first.', { exact: true })).toBeVisible();
  await approve();
  await openVaultPanel(wallet, 'Monthly allowance'); await wallet.getByLabel('Allowance step', { exact: true }).fill('2');
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toContainText('no allowance exists for step 2');
});

test('on-time renewal preserves a non-default policy and resets the monthly delay', async ({ page }) => {
  const { wallet, signer, w, s, node, approve } = await policyDevices(page, url, { monthly: '0.08', emergency: '0.25' });
  await approve();
  // Advance close to the annual renewal date, well before either block-based fallback.
  for (let month = 0; month < 11; month++) await node('Advance 31 days');
  await openVaultPanel(wallet, 'Vault policy'); await w('Renew for another year').click(); await holdSignerToPhone(page);
  await expect(signer.getByText('₿0.0800 a month', { exact: true })).toBeVisible();
  await expect(signer.getByText('Up to ₿0.2500', { exact: true })).toBeVisible();
  await approve();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
});

test('signer reviews public recovery requests, rejects early fallback, and revokes both actions', async ({ page }) => {
  await page.goto(url);
  await page.getByRole('button', { name: 'Start playground', exact: true }).click();
  const wallet = page.getByTestId('device-wallet');
  const signer = page.getByTestId('device-signer');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  const s = (name: string) => signer.getByRole('button', { name, exact: true });
  await w('Create wallet').click(); await setUpVault(page, wallet);
  await w('Reveal vault').click();
  const address = await vaultAddress(wallet);
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByRole('button', { name: 'Fund address', exact: true }).click();
  await expect(page.getByText(/"amount_sats": 210000000/)).toBeVisible({ timeout: 120_000 });
  await vaultCaughtUp(page, wallet); await openVaultPanel(wallet, 'Vault policy');
  await wallet.getByLabel('Monthly allowance in BTC', { exact: true }).fill('0.1');
  await wallet.getByLabel('Emergency access in BTC', { exact: true }).fill('0.5');
  await w('Continue on signer').click(); await holdSignerToPhone(page); await s('Approve and sign policy').click();
  await expect(wallet.getByText('Policy activated. Confirm its rollover with the regtest controls.')).toBeVisible();
  await page.getByRole('button', { name: 'Mine confirmation', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Mine confirmation', exact: true })).toBeEnabled();
  await closeVaultPanel(wallet); await vaultCaughtUp(page, wallet);
  const originalPhone = new URLSearchParams(new URL(page.url()).hash.slice(1)).get('wallet')!;
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await w('Create wallet').click();
  await recoverWithSigner(page); await w('Use this wallet’s fresh address').click();
  await w('Request HWW fallback').click(); await holdSignerToPhone(page);
  await expect(signer.getByText(/Bitcoin requires 65535 blocks/)).toBeVisible();
  await s('Approve offline action').click();
  await expect(wallet.getByRole('alert')).toContainText('HWW recovery is not mature');
  await w('Cancel').click();
  await w('Request all-action revocation').click(); await holdSignerToPhone(page);
  await expect(signer.getByText('Revoke all vault actions', { exact: true })).toBeVisible();
  await s('Reject offline action').click(); await declinedOnSigner(page);
  await expect(s('Approve offline action')).toHaveCount(0);
  await w('Request all-action revocation').click(); await holdSignerToPhone(page); await s('Approve offline action').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
  await page.getByRole('button', { name: 'Mine confirmation', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Mine confirmation', exact: true })).toBeEnabled();
  // Return to the unavailable/stolen original phone: its retained presigned actions are revoked.
  await page.getByRole('button', { name: `Wallet ${originalPhone.slice(0, 6)}`, exact: true }).click();
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await w('Reveal vault').click();
  await page.getByRole('button', { name: 'Advance 31 days', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Advance 31 days', exact: true })).toBeEnabled();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
  await openVaultPanel(wallet, 'Emergency access'); await w('Start emergency access').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
});

for (const withContact of [false, true]) {
test(`a fresh phone restores, rotates and preserves policy with explicit signer approval${withContact ? ' and recovery contact' : ''}`, async ({ page }) => {
  const { wallet, signer, w, s, node, address, approve } = await policyDevices(page, url, { monthly: '0.08', emergency: '0.25' });
  await approve();
  const fingerprint = withContact ? await enrollRecoveryContact(page) : null;
  const originalPhone = new URLSearchParams(new URL(page.url()).hash.slice(1)).get('wallet')!;
  // Distinct from the cold vault, these funds must also survive losing the original phone.
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await w('↓ Receive').click();
  const hotAddress = (await wallet.getByTestId('wallet-address').textContent())!;
  await page.getByLabel('Funding address', { exact: true }).fill(hotAddress);
  await page.getByLabel('Funding amount in sats', { exact: true }).fill('1000000');
  await node('Fund address');
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click(); await w('Reveal vault').click();
  await openVaultPanel(wallet, 'Vault settings'); await w('Create encrypted phone backup').click(); await holdSignerToPhone(page); await s('Approve recovery backup').click(); await expect(wallet.getByText('Encrypted cloud backup stored locally. The signer can recover the phone key.')).toBeVisible();
  await openVaultPanel(wallet, 'Vault settings'); await w('Export encrypted cloud backup').click();
  await expect(wallet.getByText('Encrypted backup exported separately from the signer.')).toBeVisible();
  const savedBackup = page.getByRole('button', { name: 'Load latest exported backup', exact: true });
  await expect(savedBackup).toBeVisible();
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await w('Create wallet').click();
  await recoverWithSigner(page); await savedBackup.click();
  await w('Check restored key').click();
  await expect(wallet.getByRole('alert')).toContainText('failed to read');
  await w('Request phone key recovery').click(); await holdSignerToPhone(page);
  await expect(signer.getByText('Unlock phone recovery', { exact: true })).toBeVisible();
  const newIdentity = (await signer.getByText(/^[a-f0-9]{64}$/).allTextContents())[0];
  await expect(wallet.getByText(newIdentity, { exact: true })).toBeVisible();
  await s('Reject phone recovery').click(); await declinedOnSigner(page);
  await expect(s('Approve phone recovery')).toHaveCount(0);
  await w('Request phone key recovery').click(); await holdSignerToPhone(page); await s('Approve phone recovery').click();
  await expect(wallet.getByText('Old phone key recovered into encrypted storage. Your new wallet key is unchanged.')).toBeVisible();
  await expect(wallet.getByText(/^Old key recovered: .*Rotation is still required/)).toBeVisible();
  await w('Check restored key').click();
  await expect(wallet.getByText(newIdentity, { exact: true })).toBeVisible();
  // Recovery must survive losing transient React state, with no access to the old phone.
  await page.reload(); await page.getByRole('button', { name: 'Resume playground', exact: true }).click();
  await recoverWithSigner(page); await w('Check restored key').click();
  await expect(wallet.getByText(/^Old key recovered: .*Rotation is still required/)).toBeVisible();
  await w('Propose restored vault rotation').click(); await holdSignerToPhone(page);
  await expect(signer.getByText('Rotate the recovered vault', { exact: true })).toBeVisible();
  await expect(signer.getByText(`Recovery contacts: ${withContact ? 1 : 0}`, { exact: true })).toBeVisible();
  if (fingerprint) await expect(signer.getByText(fingerprint, { exact: true })).toBeVisible();
  await expect(signer.getByText(/Preserve 0.0800 BTC monthly and 0.2500 BTC emergency/)).toBeVisible();
  await expect(signer.getByText(/Revoke the old connectors and return their change to the new vault/)).toBeVisible();
  await page.screenshot({ path: 'test-results/playground-rotation-review.png', fullPage: true });
  await s('Reject phone rotation').click(); await declinedOnSigner(page);
  await expect(s('Approve phone rotation')).toHaveCount(0);
  await w('Propose restored vault rotation').click(); await holdSignerToPhone(page); await s('Approve phone rotation').click();
  await expect(wallet.getByText('Vault rotation broadcast. Once it confirms, finish on your signer.')).toBeVisible();
  await node('Mine confirmation');
  await w('Finish on signer').click(); await holdSignerToPhone(page);
  await s('Use the rotated vault').click();
  await expect(signer.getByText('Signer paired with the rotated vault. The spending limits are preserved.')).toBeVisible();
  await w('Reveal vault').click();
  await expect(wallet.getByText('−₿0.0800', { exact: true })).toBeVisible();
  await expect(wallet.getByText('≤ ₿0.2500', { exact: true })).toBeVisible();
  await expect(wallet.getByText(address, { exact: true })).toHaveCount(0);
  const rotatedAddress = await vaultAddress(wallet);
  await openVaultPanel(wallet, 'Vault settings'); await w('Export encrypted cloud backup').click();
  await expect(wallet.getByText('Encrypted backup exported separately from the signer.')).toBeVisible();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByRole('alert')).toBeVisible();
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible();
  await node('Mine confirmation');
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click();
  await expect(wallet.getByText('₿0.0800', { exact: true })).toBeVisible({ timeout: 60_000 });
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click(); await w('Reveal vault').click();
  await openVaultPanel(wallet, 'Vault settings');
  await w('Review recovered wallet funds').click();
  await expect(wallet.getByText('Recover the old wallet balance', { exact: true })).toBeVisible();
  await w('Cancel recovered wallet transfer').click();
  await expect(w('Confirm recovered wallet transfer')).toHaveCount(0);
  await w('Review recovered wallet funds').click(); await w('Confirm recovered wallet transfer').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible(); await node('Mine confirmation');
  await page.getByRole('button', { name: `Wallet ${originalPhone.slice(0, 6)}`, exact: true }).click();
  await expect(wallet.getByText('₿0.0000', { exact: true })).toBeVisible({ timeout: 60_000 });
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click(); await w('Reveal vault').click();
  await expect(wallet.getByText('₿0.0000', { exact: true })).toBeVisible();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click(); await expect(wallet.getByRole('alert')).toBeVisible();
  await openVaultPanel(wallet, 'Emergency access'); await w('Start emergency access').click(); await expect(wallet.getByRole('alert')).toBeVisible();
  if (fingerprint) await recoverLatestBackupWithContact(page, rotatedAddress, fingerprint);
});
}

test('a signer protecting another vault refuses setup, leaving the phone free to use another', async ({ page }) => {
  await page.goto(url);
  await page.getByRole('button', { name: 'Start playground', exact: true }).click();
  const wallet = page.getByTestId('device-wallet');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  await w('Create wallet').click(); await setUpVault(page, wallet);
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await w('Create wallet').click(); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await w('Set up vault').click(); await holdSignerToPhone(page);
  await expect(wallet.getByRole('alert')).toHaveText('This signer already protects another vault.');
  await w('Cancel').click();
  // Nothing was paired: the phone still offers a fresh setup, which works with a new signer.
  await expect(w('Set up vault')).toBeVisible(); await expect(w('Finish setup')).toHaveCount(0);
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  await setUpVault(page, wallet);
  await expect(w('Reveal vault')).toBeVisible();
});

test('narrow playground remains usable without horizontal scrolling', async ({ page }) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(url);
  await page.getByRole('button', { name: 'Start playground', exact: true }).click();
  await expect(page.getByTestId('device-wallet')).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  await page.screenshot({ path: 'test-results/playground-narrow.png', fullPage: true });
});

test('recovery mining reports progress and can be cancelled', async ({ page }) => {
  await page.goto(url);
  await page.getByRole('button', { name: 'Start playground', exact: true }).click();
  await page.getByLabel('Recovery mining blocks', { exact: true }).fill('3');
  await page.getByRole('button', { name: 'Start recovery mining', exact: true }).click();
  await expect(page.getByText('Mining · completed', { exact: true })).toBeVisible();
  await expect(page.getByText('3 / 3 real blocks', { exact: true })).toBeVisible();
  await page.getByLabel('Recovery mining blocks', { exact: true }).fill('61200');
  await page.getByRole('button', { name: 'Start recovery mining', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Cancel mining', exact: true })).toBeVisible();
  await page.reload();
  await page.getByRole('button', { name: 'Resume playground', exact: true }).click();
  await expect(page.getByRole('button', { name: 'Mine confirmation', exact: true })).toBeDisabled();
  await page.getByRole('button', { name: 'Cancel mining', exact: true }).click();
  await expect(page.getByText('Mining · cancelled', { exact: true })).toBeVisible();
});

// Keep long-height recovery last: ordinary wallet tests should not repeatedly rescan its chain.
test('a new signer is connected by tap and its replacement request can be declined', async ({ page }) => {
  const { wallet, signer, w, s, approve, address } = await policyDevices(page, url, { monthly: '0.08', emergency: '0.25' });
  await approve();
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  await expect(signer.getByText('Hold me near', { exact: false })).toBeVisible();
  await openVaultPanel(wallet, 'Vault settings'); await w('Replace lost signer').click();
  await w('Connect new signer').click();
  await expect(wallet.getByText('Hold your new signer to your phone', { exact: true })).toBeVisible();
  await holdSignerToPhone(page);
  await expect(w('Propose signer replacement')).toBeEnabled();
  // The fresh signer created its key on its first tap; it is not paired with anything yet.
  await expect(signer.getByText('Hold me near', { exact: false })).toBeVisible();
  await w('Propose signer replacement').click(); await holdSignerToPhone(page);
  await expect(s('Approve signer replacement')).toBeEnabled();
  await expect(signer.getByText(address, { exact: true })).toBeVisible();
  await expect(signer.getByText(/BTC to move$/)).toBeVisible();
  await expect(signer.getByText(/Preserve 0.0800 BTC monthly and 0.2500 BTC emergency/)).toBeVisible();
  await page.screenshot({ path: 'test-results/playground-signer-replacement-review.png', fullPage: true });
  await s('Reject signer replacement').click(); await declinedOnSigner(page);
  await expect(s('Approve signer replacement')).toHaveCount(0);
});

test('a separate recovery contact enrolls through explicit backup approval and survives reload', async ({ page }) => {
  const { wallet, signer, w, s, approve } = await policyDevices(page, url);
  await approve();
  await page.getByRole('button', { name: 'Add recovery contact device', exact: true }).click();
  const contact = page.getByTestId('device-contact');
  const c = (name: string) => contact.getByRole('button', { name, exact: true });
  await c('Create wallet').click(); await c('Settings').click(); await c('Be a recovery contact').click();
  await contact.getByLabel('Recovery contact name', { exact: true }).fill('Alice');
  await c('Create recovery contact key').click();
  await expect(contact.getByTestId('contact-fingerprint')).toBeVisible();
  const fingerprint = (await contact.getByTestId('contact-fingerprint').textContent())!;
  const propose = async () => {
    await c('Share contact public key').click(); await c('Send contact public key').click();
    await w('Open received message').click();
    await expect(wallet.getByText(fingerprint, { exact: true })).toBeVisible();
    await w('Propose recovery contact').click(); await holdSignerToPhone(page);
    await expect(signer.getByText('Recovery contacts: 1', { exact: true })).toBeVisible();
    await expect(signer.getByText(fingerprint, { exact: true })).toBeVisible();
  };
  await propose(); await s('Reject recovery backup').click(); await declinedOnSigner(page);
  await expect(s('Approve recovery backup')).toHaveCount(0);
  await expect(w('Export encrypted cloud backup')).toHaveCount(0);
  await propose(); await s('Approve recovery backup').click(); await expect(wallet.getByText('Encrypted cloud backup stored locally. The signer can recover the phone key.')).toBeVisible(); await openVaultPanel(wallet, 'Vault settings'); await w('Export encrypted cloud backup').click();
  await expect(wallet.getByText('Encrypted backup exported separately from the signer.')).toBeVisible();
  await page.reload(); await page.getByRole('button', { name: 'Resume playground', exact: true }).click();
  await c('Settings').click(); await c('Be a recovery contact').click();
  await expect(contact.getByTestId('contact-fingerprint')).toHaveText(fingerprint);
  await expect(c('Create recovery contact key')).toHaveCount(0);
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click(); await w('Reveal vault').click();
  await openVaultPanel(wallet, 'Vault settings'); await w('Create encrypted phone backup').click(); await holdSignerToPhone(page);
  await expect(signer.getByText('Recovery contacts: 1', { exact: true })).toBeVisible();
  await expect(signer.getByText(fingerprint, { exact: true })).toBeVisible();
  await page.screenshot({ path: 'test-results/playground-contact-enrollment.png', fullPage: true });
  await s('Approve recovery backup').click(); await expect(wallet.getByText('Encrypted cloud backup stored locally. The signer can recover the phone key.')).toBeVisible();
  await openVaultPanel(wallet, 'Vault settings'); await w('Export encrypted cloud backup').click();
  await expect(wallet.getByText('Encrypted backup exported separately from the signer.')).toBeVisible();
});

test('lost signer is replaced only after the real phone delay and preserves the policy', async ({ page }) => {
  test.setTimeout(1_200_000);
  const { wallet, signer, w, s, node, address, approve } = await policyDevices(page, url, { monthly: '0.08', emergency: '0.25' });
  await approve();
  const fingerprint = await enrollRecoveryContact(page);
  const originalSigner = new URLSearchParams(new URL(page.url()).hash.slice(1)).get('signer')!;
  // Lose the HWW first, then demonstrate that the surviving phone still executes its policy.
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible(); await node('Mine confirmation');
  // This later deposit used to postpone the entire replacement. It must wait separately.
  await node('Advance 31 days');
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByLabel('Funding amount in sats', { exact: true }).fill('100000'); await node('Fund address');
  await closeVaultPanel(wallet); await vaultCaughtUp(page, wallet);
  await openVaultPanel(wallet, 'Vault settings'); await w('Replace lost signer').click();
  await w('Connect new signer').click(); await holdSignerToPhone(page);
  await expect(wallet.getByText('Replace the lost signer', { exact: true })).toBeVisible();
  await expect(wallet.getByTestId('signer-replacement-blocks')).toBeVisible();
  const replacementSigner = new URLSearchParams(new URL(page.url()).hash.slice(1)).get('signer')!;
  const remainingText = (await wallet.getByTestId('signer-replacement-blocks').textContent())!;
  const remaining = Number(remainingText.match(/^([0-9]+) blocks remaining$/)![1]);
  expect(remaining).toBeGreaterThan(61_000);
  const selection = (await wallet.getByTestId('signer-replacement-selection').textContent())!.match(/^([\d.]+) of ([\d.]+) BTC/)!;
  const leftBehindSats = Math.round((Number(selection[2]) - Number(selection[1])) * 100_000_000);
  // The first monthly withdrawal also created a younger continuing allowance-chain output.
  expect(leftBehindSats).toBeGreaterThan(100_000);
  await w('Propose signer replacement').click(); await holdSignerToPhone(page);
  await expect(signer.getByText('Replace the unavailable signer', { exact: true })).toBeVisible();
  await expect(signer.getByText('Recovery contacts: 1', { exact: true })).toBeVisible();
  await expect(signer.getByText(fingerprint, { exact: true })).toBeVisible();
  await expect(signer.getByText(/Preserve 0.0800 BTC monthly and 0.2500 BTC emergency/)).toBeVisible();
  await s('Reject signer replacement').click(); await declinedOnSigner(page); await expect(s('Approve signer replacement')).toHaveCount(0);
  await w('Propose signer replacement').click(); await holdSignerToPhone(page);
  await expect(s('Approve signer replacement')).toBeEnabled();
  await s('Approve signer replacement').click();
  await expect(wallet.getByRole('alert')).toContainText('phone recovery is not mature');
  // The approved message is retained for retry; signing did not change the original vault.
  await w('Cancel').click(); await expect(w('Finish')).toBeVisible();
  await closeVaultPanel(wallet); expect(await vaultAddress(wallet)).toBe(address);
  await page.getByLabel('Recovery mining blocks', { exact: true }).fill(String(remaining - 1));
  await page.getByRole('button', { name: 'Start recovery mining', exact: true }).click();
  await expect(page.getByText('Mining · completed', { exact: true })).toBeVisible({ timeout: 900_000 });
  await w('Finish').click();
  await expect(wallet.getByRole('alert')).toContainText('phone recovery is not mature');
  await node('Mine confirmation');
  // During the phone-priority window, the retained stolen HWW is still timelocked.
  await page.getByRole('button', { name: `Signer ${originalSigner.slice(0, 6)}`, exact: true }).click();
  await openSignerRecovery(wallet);
  await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-phone-recovery-blocks')).toHaveText('0 blocks remaining');
  await expect(wallet.getByTestId('source-hww-recovery-blocks')).toHaveText('4335 blocks remaining');
  await w('Use this wallet’s fresh address').click();
  await expect(wallet.getByLabel('Recovery destination', { exact: true })).toHaveValue(/^bcrt1p/);
  await w('Request HWW fallback').click(); await holdSignerToPhone(page);
  await s('Approve offline action').click();
  await expect(wallet.getByRole('alert')).toContainText('HWW recovery is not mature');
  await w('Cancel').click();
  // Sharing consumed the previous in-memory approval. Explicitly review a fresh replacement
  // request on the legitimate new signer; never infer approval from the earlier exchange.
  await page.getByRole('button', { name: `Signer ${replacementSigner.slice(0, 6)}`, exact: true }).click();
  await openVaultPanel(wallet, 'Vault settings'); await w('Replace lost signer').click(); await w('Propose signer replacement').click(); await holdSignerToPhone(page); await s('Approve signer replacement').click();
  await expect(wallet.getByText('Signer replacement broadcast. Once it confirms, finish on your new signer.')).toBeVisible();
  await node('Mine confirmation'); await w('Finish on signer').click(); await holdSignerToPhone(page);
  await s('Use the replacement vault').click();
  await expect(signer.getByText('Replacement signer paired. The phone key and spending limits are unchanged.')).toBeVisible();
  await w('Reveal vault').click();
  expect(await vaultAddress(wallet)).not.toBe(address);
  await expect(wallet.getByText('−₿0.0800', { exact: true })).toBeVisible();
  await expect(wallet.getByText('≤ ₿0.2500', { exact: true })).toBeVisible();
  const rotatedAddress = await vaultAddress(wallet);
  await openRetiredVaults(wallet);
  const retired = wallet.getByTestId(/^previous-vault-[a-f0-9]{64}$/);
  await expect(retired.getByText(address, { exact: true })).toBeVisible();
  expect(Math.round(Number((await retired.getByTestId('previous-vault-balance').textContent())!.replace(' BTC', '')) * 100_000_000)).toBe(leftBehindSats);
  let oldHwwTarget = 0;
  const mineGroup = async (blocks: number) => {
    await page.getByLabel('Recovery mining blocks', { exact: true }).fill(String(blocks));
    await page.getByRole('button', { name: 'Start recovery mining', exact: true }).click();
    await expect(page.getByText(`${blocks} / ${blocks} real blocks`, { exact: true })).toBeVisible({ timeout: 180_000 });
    await expect(page.getByText('Mining · completed', { exact: true })).toBeVisible();
  };
  // Recover the continuing allowance-chain output, then the still-younger deposit.
  for (let group = 0; group < 2; group++) {
    const lateRemaining = Number((await retired.getByTestId('previous-phone-blocks').textContent())!.split(' ')[0]);
    oldHwwTarget = Math.max(oldHwwTarget, Number((await retired.getByTestId('previous-hww-height').textContent())!.split(': ')[1]));
    expect(lateRemaining).toBeGreaterThan(1);
    await w('Review old vault funds').click(); await w('Cancel previous vault transfer').click();
    await expect(w('Confirm previous vault transfer')).toHaveCount(0);
    await w('Review old vault funds').click(); await w('Confirm previous vault transfer').click();
    await expect(wallet.getByRole('alert')).toContainText('not mature');
    await mineGroup(lateRemaining - 1); await w('Confirm previous vault transfer').click();
    await expect(wallet.getByRole('alert')).toContainText('not mature');
    await node('Mine confirmation'); await w('Confirm previous vault transfer').click();
    await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible(); await node('Mine confirmation');
    await closeVaultPanel(wallet); await vaultCaughtUp(page, wallet);
    if (group === 0) await expect(retired.getByTestId('previous-vault-balance')).toHaveText('0.0010 BTC');
  }
  // Restart both views; archived monitoring is Rust state, not retained React state.
  await page.reload(); await page.getByRole('button', { name: 'Resume playground', exact: true }).click();
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Vault', exact: true }).click(); await w('Reveal vault').click();
  await openRetiredVaults(wallet);
  await expect(retired.getByTestId('previous-vault-balance')).toHaveText('0.0000 BTC');
  await openVaultPanel(wallet, 'Vault settings'); await w('Export encrypted cloud backup').click(); await expect(wallet.getByText('Encrypted backup exported separately from the signer.')).toBeVisible();
  await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click(); await expect(wallet.getByRole('alert')).toBeVisible();
  await node('Advance 31 days'); await openVaultPanel(wallet, 'Monthly allowance'); await w('Withdraw monthly allowance').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible(); await node('Mine confirmation');
  // Also pass the stolen HWW's actual deadline, including the later deposit's deadline.
  await mineGroup(4_335); await node('Refresh chain status');
  const tip = JSON.parse((await page.getByText(/"median_time":/).textContent())!);
  expect(tip.height + 1).toBeGreaterThanOrEqual(oldHwwTarget);
  await closeVaultPanel(wallet);
  await page.getByRole('button', { name: `Signer ${originalSigner.slice(0, 6)}`, exact: true }).click();
  await openSignerRecovery(wallet); await w('Load vault from signer').click(); await holdSignerToPhone(page);
  await s('Share vault details').click(); await expect(wallet.getByText(address, { exact: true })).toBeVisible();
  await w('Use this wallet’s fresh address').click(); await w('Request HWW fallback').click();
  await expect(wallet.getByRole('alert')).toContainText('no confirmed vault inputs');
  await w('Check recovery timing').click();
  await expect(wallet.getByText('No confirmed vault outputs remain.', { exact: true })).toBeVisible();
  await recoverLatestBackupWithContact(page, rotatedAddress, fingerprint);
});

test('contact recovery works after both vault devices are lost and enforces the Bitcoin delay', async ({ page }) => {
  test.setTimeout(1_200_000);
  const { wallet, signer, w, node, approve, address } = await policyDevices(page, url);
  const s = (name: string) => signer.getByRole('button', { name, exact: true });
  await approve();
  await page.getByRole('button', { name: 'Add recovery contact device', exact: true }).click();
  const contact = page.getByTestId('device-contact');
  const c = (name: string) => contact.getByRole('button', { name, exact: true });
  await c('Create wallet').click(); await c('Settings').click(); await c('Be a recovery contact').click();
  await contact.getByLabel('Recovery contact name', { exact: true }).fill('Alice');
  await c('Create recovery contact key').click(); await c('Share contact public key').click(); await c('Send contact public key').click();
  await w('Open received message').click(); await w('Propose recovery contact').click(); await holdSignerToPhone(page); await s('Approve recovery backup').click(); await expect(wallet.getByText('Encrypted cloud backup stored locally. The signer can recover the phone key.')).toBeVisible(); await openVaultPanel(wallet, 'Vault settings'); await w('Export encrypted cloud backup').click();
  await expect(wallet.getByText('Encrypted backup exported separately from the signer.')).toBeVisible();
  // Both original vault devices become unavailable. No public vault/config is imported afterwards.
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click();
  await page.getByRole('button', { name: 'New signer device', exact: true }).click();
  await w('Create wallet').click();
  await c('Share contact public key').click(); await c('Send contact public key').click(); await w('Open received message').click();
  await page.getByRole('button', { name: 'Load latest exported backup', exact: true }).click();
  await w('Request contact recovery').click(); await w('Send to contact').click(); await c('Open received message').click();
  const recipient = (await wallet.getByTestId('contact-recipient-identity').textContent())!;
  await expect(contact.getByText(recipient, { exact: true })).toBeVisible();
  await c('Reject contact recovery').click(); await expect(c('Approve contact recovery')).toHaveCount(0);
  await w('Request contact recovery').click(); await w('Send to contact').click(); await c('Open received message').click();
  await expect(c('Approve contact recovery')).toBeEnabled();
  await page.screenshot({ path: 'test-results/playground-contact-restore-review.png', fullPage: true });
  await c('Approve contact recovery').click(); await c('Send recovery approval').click(); await w('Open received message').click();
  await expect(wallet.getByText('Contact-assisted key recovery complete. Moving vault funds still requires the Bitcoin recovery delay.')).toBeVisible();
  await expect(wallet.getByText(address, { exact: true })).toBeVisible();
  await expect(wallet.getByText(/^Spending policy unavailable:/)).toBeVisible();
  await page.reload(); await page.getByRole('button', { name: 'Resume playground', exact: true }).click();
  await w('Settings').click(); await w('Recover with a contact').click(); await w('Check stored contact recovery').click();
  await expect(wallet.getByTestId('contact-recipient-identity')).toHaveText(recipient);
  await expect(signer.getByText('Hold me near', { exact: false })).toBeVisible(); // The replacement HWW has never been needed.
  await w('Check recovered vault balance and delay').click();
  const remaining = Number((await wallet.getByTestId('contact-recovery-blocks').textContent())!.match(/^(\d+) blocks remaining$/)![1]);
  expect(remaining).toBeGreaterThan(61_000);
  await w('Use new wallet address').click(); await w('Review recovered vault transfer').click();
  await w('Cancel recovered vault transfer').click(); await expect(w('Confirm recovered vault transfer')).toHaveCount(0);
  await w('Review recovered vault transfer').click();
  const sats = Number((await wallet.getByText(/^\d+ sats$/).textContent())!.split(' ')[0]);
  await w('Confirm recovered vault transfer').click(); await expect(wallet.getByRole('alert')).toContainText('phone recovery is not mature');
  await page.getByLabel('Recovery mining blocks', { exact: true }).fill(String(remaining - 1));
  await page.getByRole('button', { name: 'Start recovery mining', exact: true }).click();
  await expect(page.getByText('Mining · completed', { exact: true })).toBeVisible({ timeout: 720_000 });
  await w('Confirm recovered vault transfer').click(); await expect(wallet.getByRole('alert')).toContainText('phone recovery is not mature');
  await node('Mine confirmation'); await w('Confirm recovered vault transfer').click();
  await expect(wallet.getByText(/^Broadcast [a-f0-9]{64}$/)).toBeVisible(); await node('Mine confirmation');
  await w('Check recovered vault balance and delay').click(); await expect(wallet.getByText('0.0000 BTC recoverable', { exact: true })).toBeVisible();
  // Tens of thousands of blocks later, the wallet waits for an explicit catch-up rather than
  // walking them in the background.
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click(); await w('Catch up').click();
  await expect(w('Catch up')).toHaveCount(0, { timeout: 180_000 });
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText(`₿${(sats / 100_000_000).toLocaleString('en-US', { minimumFractionDigits: 4, maximumFractionDigits: 8 })}`);
});

test('lost phone without cloud backup recovers only after the real HWW delay', async ({ page }) => {
  test.setTimeout(1_200_000);
  const { wallet, signer, w, s, node, approve } = await policyDevices(page, url);
  await approve();
  // The new phone never loads the cloud backup: recovery here relies on the signer alone.
  await openVaultPanel(wallet, 'Vault settings'); await w('Create encrypted phone backup').click(); await holdSignerToPhone(page); await s('Approve recovery backup').click(); await expect(wallet.getByText('Encrypted cloud backup stored locally. The signer can recover the phone key.')).toBeVisible();
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click(); await w('Create wallet').click();
  await recoverWithSigner(page);
  await expect(wallet.getByLabel('Encrypted cloud backup JSON', { exact: true })).toHaveValue('');
  await expect(w('Request phone key recovery')).toBeDisabled();
  await expect(wallet.getByText(/^No cloud backup loaded\./)).toBeVisible();
  await w('Check restored key').click(); await expect(wallet.getByRole('alert')).toContainText('failed to read');
  await w('Use this wallet’s fresh address').click();
  await expect(wallet.getByLabel('Recovery destination', { exact: true })).toHaveValue(/^bcrt1p/);
  const destination = await wallet.getByLabel('Recovery destination', { exact: true }).inputValue();
  await w('Check recovery timing').click();
  const remaining = Number((await wallet.getByTestId('source-hww-recovery-blocks').textContent())!.split(' ')[0]);
  const phoneRemaining = Number((await wallet.getByTestId('source-phone-recovery-blocks').textContent())!.split(' ')[0]);
  expect(remaining).toBeGreaterThan(65_000); expect(remaining - phoneRemaining).toBe(4_335);
  await w('Request HWW fallback').click(); await holdSignerToPhone(page);
  await expect(signer.getByText(destination, { exact: true })).toBeVisible();
  await expect(signer.getByText(/Bitcoin requires 65535 blocks/)).toBeVisible();
  await s('Reject offline action').click(); await declinedOnSigner(page); await expect(s('Approve offline action')).toHaveCount(0);
  await w('Request HWW fallback').click(); await holdSignerToPhone(page);
  const amount = (await signer.getByTestId('offline-action-amount').textContent())!.replace(' BTC', '');
  await s('Approve offline action').click();
  await expect(wallet.getByRole('alert')).toContainText('HWW recovery is not mature');
  await w('Cancel').click();
  await page.getByLabel('Recovery mining blocks', { exact: true }).fill(String(remaining - 1));
  await page.getByRole('button', { name: 'Start recovery mining', exact: true }).click();
  await expect(page.getByText('Mining · completed', { exact: true })).toBeVisible({ timeout: 900_000 });
  await w('Check recovery timing').click();
  await expect(wallet.getByTestId('source-phone-recovery-blocks')).toHaveText('0 blocks remaining');
  await expect(wallet.getByTestId('source-hww-recovery-blocks')).toHaveText('1 blocks remaining');
  await w('Finish').click(); await expect(wallet.getByRole('alert')).toContainText('HWW recovery is not mature');
  await node('Mine confirmation'); await w('Finish').click();
  const broadcast = wallet.getByText(/^Broadcast [a-f0-9]{64}$/);
  await expect(broadcast).toBeVisible();
  await node('Mine confirmation'); await w('Check recovery timing').click();
  await expect(wallet.getByText('No confirmed vault outputs remain.', { exact: true })).toBeVisible();
  await w('Check restored key').click(); await expect(wallet.getByRole('alert')).toContainText('failed to read');
  // Tens of thousands of blocks later, the wallet waits for an explicit catch-up rather than
  // walking them in the background.
  await closeVaultPanel(wallet); await wallet.getByRole('tab', { name: 'Wallet', exact: true }).click(); await w('Catch up').click();
  await expect(w('Catch up')).toHaveCount(0, { timeout: 180_000 });
  await expect(wallet.getByTestId('bitcoin-balance')).toHaveText(`₿${amount}`);
});
