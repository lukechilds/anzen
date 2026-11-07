import { test, expect, Locator } from '@playwright/test';
import { resolve } from 'node:path';
import { pathToFileURL } from 'node:url';
import { startTestHost } from './host';

let host: Awaited<ReturnType<typeof startTestHost>>;
test.beforeAll(async () => { host = await startTestHost(18895); });
test.afterAll(() => host?.stop());

// Compare rendered styles with the retained design, not a second set of copied constants.
// Live balances/activity remain real; this intentionally does not inject the mockup's data.
test('Wallet geometry and typography match the Dark reference', async ({ page, context }) => {
  const reference = await context.newPage();
  try {
    await reference.goto(pathToFileURL(resolve('..', 'mockups', 'compare.html')).href, { waitUntil: 'domcontentloaded' });
    await page.goto(host.url);
    await page.getByRole('button', { name: 'Start playground', exact: true }).click();
    const wallet = page.getByTestId('device-wallet');
    await wallet.getByRole('button', { name: 'Create wallet', exact: true }).click();
    await expect(wallet.getByTestId('bitcoin-balance')).toBeVisible();

    async function styles(locator: Locator, properties: string[]) {
      return locator.evaluate((element, names) => {
        const style = getComputedStyle(element);
        return Object.fromEntries(names.map(name => [name, style.getPropertyValue(name)]));
      }, properties);
    }
    async function match(actual: Locator, selector: string, properties: string[]) {
      expect(await styles(actual, properties)).toEqual(await styles(reference.locator(`#themeDark ${selector}`).first(), properties));
    }
    await match(wallet, '.ph', ['width', 'height', 'border-radius', 'padding', 'background-color']);
    await match(wallet.locator(':scope > div').first(), '.screen', ['border-radius', 'background-color']);
    await match(wallet.getByText('Wallet', { exact: true }).first(), '.eyebrow', ['font-size', 'letter-spacing', 'text-align', 'text-transform', 'color']);
    await match(wallet.getByTestId('bitcoin-balance'), '.balance', ['font-size', 'font-weight', 'letter-spacing', 'color']);
    await match(wallet.getByRole('button', { name: '↑ Send', exact: true }), '.acts .pill', ['height', 'border-radius', 'gap']);
    await match(wallet.getByText('Send', { exact: true }), '.acts .pill', ['font-size', 'font-weight', 'color']);
    await match(wallet.getByRole('tab', { name: 'Wallet', exact: true }).locator('..'), '.tabbar', ['height', 'border-radius', 'left', 'right', 'bottom', 'background-color']);
    await wallet.screenshot({ path: test.info().outputPath('live-wallet.png') });
    await reference.locator('#themeDark .ph').screenshot({ path: test.info().outputPath('dark-reference.png') });
  } finally { await reference.close(); }
});
