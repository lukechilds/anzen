import { expect, Locator, Page } from '@playwright/test';

export async function closeVaultPanel(wallet: Locator) {
  const panel = wallet.getByRole('dialog');
  if (await panel.count()) {
    const label = await panel.getAttribute('aria-label');
    // A tap sheet closes itself once the exchange finishes.
    if (label === 'Connect signer') { await expect(panel).toHaveCount(0); return; }
    await wallet.getByRole('button', { name: `Close ${label}`, exact: true }).click();
    await expect(panel).toHaveCount(0);
  }
}

// Visible navigation, not a shortcut into component state or the Rust API.
export async function openVaultPanel(wallet: Locator, name: string) {
  if (await wallet.getByRole('dialog', { name, exact: true }).count()) return;
  await closeVaultPanel(wallet);
  const vaultTab = wallet.getByRole('tab', { name: 'Vault', exact: true });
  if (await vaultTab.getAttribute('aria-selected') !== 'true') await vaultTab.click();
  const back = wallet.getByRole('button', { name: 'Back to vault', exact: true });
  if (await back.count()) await back.click();
  const button = (title: string) => wallet.getByRole('button', { name: title, exact: true });
  if (name === 'Vault settings') await button('Settings').click();
  else if (name === 'Vault policy') await wallet.getByRole('button', { name: /^(Annual renewal|Set monthly allowance)$/ }).click();
  else if (name === 'Vault deposit') await button('Deposit').click();
  else if (name === 'Move vault funds') { await button('Withdraw').click(); await button('Move all vault funds').click(); }
  else if (name === 'Saved messages') { await button('Settings').click(); await button('Saved messages').click(); }
  else if (name === 'Lost a device') { await button('Settings').click(); await button('Lost a device?').click(); }
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
  await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await openVaultPanel(wallet, 'Vault settings'); await button('Lost a device?').click();
  await button('Signer recovery and revocation').click();
  await expect(button('Check recovery timing')).toBeVisible();
}

// Put an earlier device back in its slot (a stolen phone, say). The playground keeps replaced
// devices but never lists them; this test hook brings one back.
// The device open in a slot, from where the emulator keeps each network's devices: a regtest host
// session's under its capability, Mutinynet's and mainnet's under the network.
export async function deviceId(page: Page, slot: 'wallet' | 'signer', network: 'regtest' | 'signet' | 'bitcoin' = 'regtest') {
  return page.evaluate(([slot, network]) => {
    const prefix = network === 'regtest' ? 'anzen-regtest-session:' : `anzen-static-session:${network}`;
    const key = Object.keys(localStorage).find(key => key.startsWith(prefix));
    return new URLSearchParams((key && localStorage.getItem(key)) || '').get(slot)!;
  }, [slot, network] as const);
}

export async function useDevice(page: Page, slot: 'wallet' | 'signer', id: string) {
  await page.evaluate(([slot, id]) => (window as unknown as { anzenPlayground: { use(slot: string, id: string): Promise<void> } }).anzenPlayground.use(slot, id), [slot, id]);
}

// The playground's chain status after funding an address.
export const fundedText = (sats: number | string) =>
  `Funded ₿${(Number(sats) / 100_000_000).toLocaleString('en-US', { minimumFractionDigits: 4, maximumFractionDigits: 8 })}`;

// The signer shows one fact per page. Jump through its pages until `text` is on screen.
export async function signerShows(page: Page, text: string | RegExp) {
  const signer = page.getByTestId('device-signer');
  const target = signer.getByText(text, typeof text === 'string' ? { exact: true } : undefined).first();
  const dots = signer.getByRole('button', { name: /^Page \d+ of \d+$/ });
  await expect(dots.first().or(target).first()).toBeVisible();
  const pages = await dots.count();
  for (let i = 0; i < Math.max(pages, 1); i++) {
    if (pages) await dots.nth(i).click();
    if (await target.isVisible()) return;
  }
  await expect(target).toBeVisible({ timeout: 1 });
}

// Approve on the signer as its user would: step to the last page, then press and hold ✓
// (signing approvals) or tap it (everything else).
export async function approveOnSigner(page: Page, label: string) {
  const signer = page.getByTestId('device-signer');
  const confirm = signer.getByRole('button', { name: label, exact: true });
  const next = signer.getByRole('button', { name: 'Next', exact: true });
  await expect(confirm.or(next).first()).toBeVisible();
  while (!await confirm.isVisible()) await next.click();
  if (await confirm.getAttribute('data-hold')) await confirm.click({ delay: 1500 }); else await confirm.click();
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
 
  await signer.getByRole('button', { name: 'Share vault details', exact: true }).click();
  await expect(wallet.getByRole('button', { name: 'Check recovery timing', exact: true })).toBeVisible();
}

// Set up a vault: one tap carries the signer's key, the vault and the recovery backup.
export async function setUpVault(page: Page, wallet: Locator) {
  await wallet.getByRole('tab', { name: 'Vault', exact: true }).click();
  await wallet.getByRole('button', { name: 'Set up vault', exact: true }).click();
 
  await expect(wallet.getByText('Your vault is ready', { exact: true })).toBeVisible();
  await wallet.getByRole('button', { name: 'Done', exact: true }).click();
}

// Helpers perform only visible user actions. All construction/signing happens in Rust.
export async function policyDevices(page: Page, url: string, {
  funding = '210000000', monthly = '0.1', emergency = '0.5',
} = {}) {
  await page.goto(url);
  const wallet = page.getByTestId('device-wallet');
  const signer = page.getByTestId('device-signer');
  const w = (name: string) => wallet.getByRole('button', { name, exact: true });
  const s = (name: string) => signer.getByRole('button', { name, exact: true });
  const node = async (name: string) => {
    const button = page.getByRole('button', { name, exact: true });
    await button.click();
    await expect(button).toBeEnabled({ timeout: name === 'Fund address' ? 120_000 : 30_000 });
  };
  await w('Get started').click();
  await setUpVault(page, wallet);
  const address = await vaultAddress(wallet);
  await page.getByLabel('Funding address', { exact: true }).fill(address);
  await page.getByLabel('Funding amount in sats', { exact: true }).fill(funding);
  await node('Fund address');
  await expect(page.getByText(fundedText(funding), { exact: true })).toBeVisible();
  await vaultCaughtUp(page, wallet);
  await openVaultPanel(wallet, 'Vault policy');
  await wallet.getByLabel('Monthly allowance in BTC', { exact: true }).fill(monthly);
  await wallet.getByLabel('Emergency access in BTC', { exact: true }).fill(emergency);
  await w('Continue on signer').click();
  await signerShows(page, 'Annual vault policy');
  const approve = async () => {
    await approveOnSigner(page, 'Approve and sign policy');
    await expect(wallet.getByText('Allowance approved.')).toBeVisible();
    await node('Mine confirmation'); await closeVaultPanel(wallet); await vaultCaughtUp(page, wallet);
  };
  return { wallet, signer, w, s, node, address, approve };
}

// Live updates replace the sync button. Wait until the revealed vault reflects the node's current
// tip, so a following step never acts on the view from before a block.
export async function vaultCaughtUp(page: Page, wallet: Locator) {
  const refresh = page.getByRole('button', { name: 'Refresh chain status', exact: true });
  await refresh.click(); await expect(refresh).toBeEnabled();
  const height = Number((await page.getByText(/^Block \d+$/).textContent())!.split(' ')[1]);
  await expect.poll(async () => Number(await wallet.getByTestId('device-screen').getAttribute('data-vault-height')), { timeout: 60_000 })
    .toBeGreaterThanOrEqual(height);
}
