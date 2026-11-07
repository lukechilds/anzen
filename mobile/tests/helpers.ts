import { expect, Locator, Page } from '@playwright/test';

export async function closeVaultPanel(wallet: Locator) {
  const panel = wallet.getByRole('dialog');
  if (await panel.count()) {
    const label = await panel.getAttribute('aria-label');
    await wallet.getByRole('button', { name: `Close ${label}`, exact: true }).click();
    await expect(panel).toHaveCount(0);
  }
}

// Visible navigation, not a shortcut into component state or the Rust API.
export async function openVaultPanel(wallet: Locator, name: string) {
  if (await wallet.getByRole('dialog', { name, exact: true }).count()) return;
  await closeVaultPanel(wallet);
  const back = wallet.getByRole('button', { name: 'Back to vault', exact: true });
  if (await back.count()) await back.click();
  const button = (title: string) => wallet.getByRole('button', { name: title, exact: true });
  if (name === 'Vault settings') await button('Settings').click();
  else if (name === 'Vault policy') await wallet.getByRole('button', { name: /^(Annual renewal|Set monthly allowance)$/ }).click();
  else if (name === 'Vault deposit') await button('Deposit').click();
  else if (name === 'Move vault funds') { await button('Withdraw').click(); await button('Move all vault funds').click(); }
  else if (name === 'Phone recovery') { await button('Settings').click(); await button('Recover without signer').click(); }
  else await button(name).click();
  await expect(wallet.getByRole('dialog', { name, exact: true })).toBeVisible();
}

export async function vaultAddress(wallet: Locator) {
  await openVaultPanel(wallet, 'Vault deposit');
  const address = (await wallet.getByTestId('vault-address').textContent())!;
  await closeVaultPanel(wallet);
  return address;
}

export async function openSignerRecovery(wallet: Locator) {
  await closeVaultPanel(wallet);
  const button = (name: string) => wallet.getByRole('button', { name, exact: true });
  if (await button('Check recovery timing').count()) return;
  if (await button('Back to vault').count()) await button('Back to vault').click();
  if (await wallet.getByRole('button', { name: /^(Annual renewal|Set monthly allowance)$/ }).count()) await openVaultPanel(wallet, 'Vault settings');
  await button('Signer recovery and revocation').click();
  await expect(button('Check recovery timing')).toBeVisible();
}

export async function openRetiredVaults(wallet: Locator) {
  await openVaultPanel(wallet, 'Vault settings');
  await wallet.getByRole('button', { name: 'Retired vaults', exact: true }).click();
}

// The playground's stand-in for holding the signer to the phone, once the phone asks for it.
export async function holdSignerToPhone(page: Page) {
  await page.getByRole('button', { name: 'Hold signer to phone', exact: true }).click();
}

// After the signer's user rejects a request, the phone reports it and closes the tap.
export async function declinedOnSigner(page: Page) {
  const wallet = page.getByTestId('device-wallet');
  await expect(wallet.getByText('Declined on your signer', { exact: true })).toBeVisible();
  await wallet.getByRole('button', { name: 'Close', exact: true }).click();
  await expect(wallet.getByTestId('tap-sheet')).toHaveCount(0);
}

// A phone without the vault asks the signer for its public recovery details.
export async function recoverWithSigner(page: Page) {
  const wallet = page.getByTestId('device-wallet'), signer = page.getByTestId('device-signer');
  await closeVaultPanel(wallet);
  await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await wallet.getByRole('button', { name: 'Recover with your signer', exact: true }).click();
  await holdSignerToPhone(page);
  await signer.getByRole('button', { name: 'Share vault details', exact: true }).click();
  await expect(wallet.getByRole('button', { name: 'Check recovery timing', exact: true })).toBeVisible();
}

// Set up a vault: one tap carries the signer's key, the vault and the recovery backup.
export async function setUpVault(page: Page, wallet: Locator) {
  await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await wallet.getByRole('button', { name: 'Set up vault', exact: true }).click();
  await holdSignerToPhone(page);
  await expect(wallet.getByText('Your vault is ready', { exact: true })).toBeVisible();
  await wallet.getByRole('button', { name: 'Done', exact: true }).click();
}

// Helpers perform only visible user actions. All construction/signing happens in Rust.
export async function policyDevices(page: Page, url: string, {
  funding = '210000000', monthly = '0.1', emergency = '0.5',
} = {}) {
  await page.goto(url);
  await page.getByRole('button', { name: 'Start playground', exact: true }).click();
  const wallet = page.getByTestId('device-wallet');
  const signer = page.getByTestId('device-signer');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  const s = (name: string) => signer.getByRole('button', { name, exact: true });
  const node = async (name: string) => {
    const button = page.getByRole('button', { name, exact: true });
    await button.click();
    await expect(button).toBeEnabled({ timeout: name === 'Fund address' ? 120_000 : 30_000 });
  };
  await w('Create wallet').click();
  await setUpVault(page, wallet);
  await w('Reveal vault').click();
  const address = await vaultAddress(wallet);
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByLabel('Funding amount in sats', { exact: true }).fill(funding);
  await node('Fund address');
  await expect(page.getByText(new RegExp(`"amount_sats": ${funding}`))).toBeVisible();
  await vaultCaughtUp(page, wallet);
  await openVaultPanel(wallet, 'Vault policy');
  await wallet.getByLabel('Monthly allowance in BTC', { exact: true }).fill(monthly);
  await wallet.getByLabel('Emergency access in BTC', { exact: true }).fill(emergency);
  await w('Continue on signer').click(); await holdSignerToPhone(page);
  await expect(signer.getByText('Annual vault policy', { exact: true })).toBeVisible();
  const approve = async () => {
    await s('Approve and sign policy').click();
    await expect(wallet.getByText('Policy activated. Confirm its rollover with the regtest controls.')).toBeVisible();
    await node('Mine confirmation'); await closeVaultPanel(wallet); await vaultCaughtUp(page, wallet);
  };
  return { wallet, signer, w, s, node, address, approve };
}

export async function enrollRecoveryContact(page: Page) {
  const w = (name: string) => page.getByTestId('device-wallet').getByRole('button', { name, exact: true });
  const s = (name: string) => page.getByTestId('device-signer').getByRole('button', { name, exact: true });
  await page.getByRole('button', { name: 'Add recovery contact device', exact: true }).click();
  const contact = page.getByTestId('device-contact');
  const c = (name: string) => contact.getByRole('button', { name, exact: true });
  await c('Create wallet').click(); await c('Settings').click(); await c('Be a recovery contact').click();
  await contact.getByLabel('Recovery contact name', { exact: true }).fill('Alice');
  await c('Create recovery contact key').click();
  const fingerprint = (await contact.getByTestId('contact-fingerprint').textContent())!;
  await c('Share contact public key').click(); await c('Send contact public key').click();
  await w('Open received message').click(); await w('Propose recovery contact').click(); await holdSignerToPhone(page);
  await expect(page.getByTestId('device-signer').getByText(fingerprint, { exact: true })).toBeVisible();
  await s('Approve recovery backup').click();
  await expect(page.getByTestId('device-wallet').getByText('Encrypted cloud backup stored locally. The signer can recover the phone key.')).toBeVisible();
  return fingerprint;
}

// Exercises decryption of the newly exported backup, not just presence of an old fingerprint.
export async function recoverLatestBackupWithContact(page: Page, vaultAddress: string, fingerprint: string) {
  const wallet = page.getByTestId('device-wallet');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  const contact = page.getByTestId('device-contact');
  const c = (name: string) => contact.getByRole('button', { name, exact: true });
  await page.getByRole('button', { name: 'New wallet phone', exact: true }).click(); await w('Create wallet').click();
  await contact.getByRole('tab', { name: 'Wallet', exact: true }).click(); await c('Settings').click(); await c('Be a recovery contact').click();
  await c('Share contact public key').click(); await c('Send contact public key').click(); await w('Open received message').click();
  // Opening the contact starts an asynchronous Rust inspection and changes the
  // screen. Do not click a different scroll region while that transition is still
  // finishing: RN Web can cancel the press when its scroll container moves.
  await expect(wallet.getByText(fingerprint, { exact: true })).toBeVisible();
  const backupInput = wallet.getByLabel('Social recovery cloud backup JSON', { exact: true });
  await expect(backupInput).toBeVisible();
  await expect(w('Check stored contact recovery')).toBeEnabled();
  const [loaded] = await Promise.all([
    page.waitForResponse(response => response.url().endsWith('/cloud')
      && response.request().postDataJSON()?.method === 'load'),
    page.getByRole('button', { name: 'Load latest exported backup', exact: true }).click(),
  ]);
  expect(loaded.ok()).toBe(true);
  await expect(backupInput).toHaveValue(JSON.stringify(await loaded.json()));
  await w('Request contact recovery').click(); await w('Send to contact').click(); await c('Open received message').click();
  const recipient = (await wallet.getByTestId('contact-recipient-identity').textContent())!;
  await expect(contact.getByText(recipient, { exact: true })).toBeVisible();
  await expect(contact.getByText(fingerprint, { exact: true })).toBeVisible();
  await c('Approve contact recovery').click(); await c('Send recovery approval').click(); await w('Open received message').click();
  await expect(wallet.getByText('Contact-assisted key recovery complete. Moving vault funds still requires the Bitcoin recovery delay.')).toBeVisible();
  await expect(wallet.getByText(vaultAddress, { exact: true })).toBeVisible();
}

// Live updates replace the sync button. Wait until the revealed vault reflects the node's current
// tip, so a following step never acts on the view from before a block.
export async function vaultCaughtUp(page: Page, wallet: Locator) {
  const refresh = page.getByRole('button', { name: 'Refresh chain status', exact: true });
  await refresh.click(); await expect(refresh).toBeEnabled();
  const height = Number(/"height": (\d+)/.exec((await page.getByText(/"height": \d+/).textContent())!)![1]);
  await expect.poll(async () => Number(await wallet.getByTestId('device-screen').getAttribute('data-vault-height')), { timeout: 60_000 })
    .toBeGreaterThanOrEqual(height);
}
