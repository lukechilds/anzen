/* Local-only prototype. No wallet access, persistence, signing, or broadcasting. */
'use strict';

const paths = {
  home: '<path d="m3 10 9-7 9 7v10a1 1 0 0 1-1 1h-5v-7H9v7H4a1 1 0 0 1-1-1z"/>',
  vault: '<rect x="3" y="3" width="18" height="18" rx="5"/><circle cx="12" cy="12" r="4"/><path d="M12 6v3m0 6v3m-6-6h3m6 0h3"/>',
  activity: '<path d="M4 7h16M4 12h12M4 17h8"/>',
  shield: '<path d="m12 3 8 3v6c0 4-4 7-8 9-4-2-8-5-8-9V6z"/><path d="m8.5 12 2.5 2.5 4.5-5"/>',
  up: '<path d="M12 19V5m-6 6 6-6 6 6"/>',
  down: '<path d="M12 5v14m-6-6 6 6 6-6"/>',
  chevron: '<path d="m9 5 7 7-7 7"/>',
  arrow: '<path d="M5 12h14m-6-6 6 6-6 6"/>',
  clock: '<circle cx="12" cy="12" r="8.5"/><path d="M12 7v5l3 2"/>',
  check: '<path d="m5 12 4.5 4.5L19 7"/>',
  close: '<path d="m6 6 12 12M6 18 18 6"/>',
  phone: '<rect x="6" y="2" width="12" height="20" rx="3"/><path d="M11 18h2"/>',
  hardware: '<rect x="7" y="2" width="10" height="20" rx="3"/><path d="M10 6h4v5h-4zm1 11h2"/>',
  cloud: '<path d="M6 18a5 5 0 0 1-1-9.9 7 7 0 0 1 13.4.9A4.5 4.5 0 0 1 18 18H6z"/>',
  people: '<circle cx="9" cy="7" r="3"/><path d="M3 20v-2a6 6 0 0 1 12 0v2m1-16a3 3 0 0 1 0 6m2 4a5 5 0 0 1 3 4v2"/>',
  repeat: '<path d="M4 9a8 8 0 0 1 14-3l2 2m0-5v5h-5M20 15a8 8 0 0 1-14 3l-2-2m0 5v-5h5"/>',
  lock: '<rect x="5" y="10" width="14" height="11" rx="3"/><path d="M8 10V7a4 4 0 0 1 8 0v3m-4 5v2"/>',
  copy: '<rect x="8" y="8" width="12" height="13" rx="2"/><path d="M15 8V3H3v13h5"/>',
  more: '<circle cx="5" cy="12" r="1"/><circle cx="12" cy="12" r="1"/><circle cx="19" cy="12" r="1"/>',
  back: '<path d="M19 12H5m6-6-6 6 6 6"/>',
  eye: '<path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7S2 12 2 12z"/><circle cx="12" cy="12" r="3"/>',
};
const icon = (name, extra = '') => `<svg class="icon ${extra}" viewBox="0 0 24 24" aria-hidden="true">${paths[name] || paths.vault}</svg>`;
document.querySelectorAll('[data-icon]').forEach(el => { el.innerHTML = icon(el.dataset.icon); });
if (new URLSearchParams(location.search).has('embed')) document.body.classList.add('embedded');

const INITIAL = { wallet: 2480000, vault: 210000000, monthly: 10000000, emergency: 50000000, funded: 12, monthlyReady: true, policyDate: '5 Aug 2026', renewDate: '5 Aug 2027', claimed: false, revoked: false, emergencyState: 'ready', activity: [] };
let state = structuredClone(INITIAL);
let currentView = 'home';
let activityFilter = 'all';
let vaultRevealed = false;
const privateViews = new Set(['vault', 'vault-activity', 'security']);
const publicActions = new Set(['send', 'confirm-send', 'receive', 'copy-address', 'wallet-options', 'reveal-vault', 'close']);
let focusBeforeSheet;
let toastTimer;
const content = document.querySelector('#content');
const layer = document.querySelector('#modal-layer');
const sheet = document.querySelector('#sheet');
const btc = sats => new Intl.NumberFormat('en-US', { maximumFractionDigits: 8 }).format(sats / 1e8);
// This fixed illustrative rate is not market data.
const fiat = sats => new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD', maximumFractionDigits: 0 }).format(sats / 1e8 * 109600);
const amount = (sats, css = 'balance') => `<p class="${css}">${btc(sats)} <span class="unit">BTC</span></p>`;
const action = (label, name, kind = 'primary', symbol = '') => `<button class="button ${kind}" data-action="${name}">${symbol ? icon(symbol) : ''}${label}</button>`;
const roundAction = (label, name, symbol, primary = false) => `<button class="round-action ${primary ? 'round-primary' : ''}" data-action="${name}"><span class="action-circle">${icon(symbol)}</span><span>${label}</span></button>`;
const mark = '<svg viewBox="0 0 190 180" aria-hidden="true"><path d="M95 0 190 180h-46L95 87.2 46 180H0Z"/></svg>';
const baseActivity = [
  { title: 'Sent to Alex', subtitle: 'Today · confirmed', value: '−0.002 BTC', detail: '$219', avatar: 'A', type: 'wallet' },
  { title: 'Bitrefill', subtitle: 'Yesterday · gift card', value: '−0.0027 BTC', detail: '$296', image: 'assets/bitrefill.png', type: 'wallet' },
  { title: 'Mullvad', subtitle: 'Yesterday · VPN', value: '−0.0005 BTC', detail: '$55', image: 'assets/mullvad.png', type: 'wallet' },
  { title: 'Received bitcoin', subtitle: '3 Sep · confirmed', value: '+0.03 BTC', detail: '$3,288', symbol: 'down', positive: true, type: 'wallet' },
  { title: 'Vault policy renewed', subtitle: '5 Aug · hardware approved', value: '2.1 BTC', detail: 'Protected in vault', symbol: 'shield', type: 'vault' },
];
function activityRows(items) {
  return items.map(item => `<div class="activity-row"><div class="avatar ${item.image ? 'merchant' : ''}">${item.image ? `<img src="${item.image}" alt="">` : item.symbol ? icon(item.symbol) : item.avatar}</div><div class="row-copy"><strong>${item.title}</strong><small>${item.subtitle}</small></div><div class="activity-amount ${item.positive ? 'positive' : ''}">${item.value}<small>${item.detail}</small></div></div>`).join('');
}
function logActivity(title, value, type = 'vault') {
  state.activity.unshift({ title, value, subtitle: 'Just now · demo confirmation', detail: type === 'vault' ? 'Vault' : 'Spending wallet', symbol: type === 'vault' ? 'vault' : 'up', type });
}
// Public screens intentionally receive only wallet activity, never the combined feed.
const walletActivity = () => [...state.activity, ...baseActivity].filter(item => item.type === 'wallet');
function homeView() {
  return `<section class="page wallet-page" aria-label="Wallet">
    <div class="hero"><p class="eyebrow">Your wallet</p>${amount(state.wallet)}<p class="fiat">${fiat(state.wallet)} <span class="fiat-separator">·</span> available to spend</p>
      <div class="round-actions">${roundAction('Send', 'send', 'up', true)}${roundAction('Receive', 'receive', 'down')}</div>
    </div>
    <div class="section-heading"><h2>Activity</h2><button class="text-button" data-view="activity" aria-label="View wallet activity">View all ${icon('arrow')}</button></div>
    ${activityRows(walletActivity().slice(0, 4))}
    <div class="wallet-signoff">A little simpler. Entirely yours.</div>
  </section>`;
}
function vaultGateView() {
  return `<section class="page vault-gate" aria-label="Hidden vault">
    <div class="eyebrow">A space of your own</div>
    <div class="vault-seal" aria-hidden="true"><div class="seal-inner">${mark}<span>${icon('lock')}</span></div></div>
    <h1>Your private vault.</h1><p>Your savings stay out of sight.<br>Reveal them when you’re ready.</p>
    ${action('Reveal vault', 'reveal-vault', 'primary', 'eye')}
    <span class="gate-note">Hidden again when you leave this space.</span>
  </section>`;
}
function vaultView() {
  const emergencyPending = state.emergencyState === 'pending';
  const emergencyCancelled = state.emergencyState === 'cancelled';
  const monthlyAvailable = !state.revoked && !state.claimed && state.monthlyReady && state.funded > 0 && state.monthly > 0;
  return `<section class="page vault-page" aria-label="Vault">
    <div class="hero"><p class="eyebrow">Your vault</p>${amount(state.vault)}<p class="fiat">${fiat(state.vault)} <span class="fiat-separator">·</span> in cold storage</p>
      <div class="round-actions">${roundAction('Add bitcoin', 'deposit', 'down')}${roundAction('Withdraw', 'withdraw', 'up')}</div>
    </div>
    <div class="section-heading"><h2>Your policy</h2><button class="text-button" data-action="edit-policy">Edit ${icon('arrow')}</button></div>
    <section class="policy-block" aria-label="Monthly withdrawal">
      <button class="policy-summary" data-action="monthly"><span><span class="policy-label">${icon('repeat')}Monthly withdrawal</span><span class="policy-amount">${btc(state.monthly)} BTC</span><span class="policy-description ${monthlyAvailable ? 'available' : ''}">${state.revoked ? 'Remaining withdrawals stopped' : state.claimed ? `1 of ${state.funded} withdrawn · next one waiting` : monthlyAvailable ? `Available now · ${state.funded} withdrawals approved` : state.monthly && state.funded ? 'Waiting for first withdrawal' : 'Not funded'}</span></span>${icon('chevron', 'chevron')}</button>
    </section>
    <section class="policy-block" aria-label="Emergency access">
      <button class="policy-summary" data-action="${emergencyPending ? 'emergency-status' : emergencyCancelled ? 'emergency-disabled' : state.emergency ? 'emergency' : 'edit-policy'}"><span><span class="policy-label">${icon('clock')}Emergency access</span><span class="policy-amount">${btc(state.emergency)} BTC <small>after 1 week</small></span><span class="policy-description">${emergencyPending ? 'Waiting period started · you can still cancel' : emergencyCancelled ? 'Cancelled · funds stay in your vault' : state.emergency ? 'Once per policy · no hardware wallet needed' : 'Not enabled in this policy'}</span></span>${icon('chevron', 'chevron')}</button>
    </section>
    <div class="policy-status">${icon('shield')}<div class="row-copy"><strong>Policy active</strong><small>Next renewal · ${state.renewDate}</small></div><button class="text-button" data-view="security">Recovery ${icon('arrow')}</button></div>
    ${!state.revoked && state.monthly && state.funded ? `<button class="policy-link" data-action="revoke">Stop remaining monthly withdrawals${icon('chevron')}</button>` : ''}
  </section>`;
}
function securityView() {
  return `<section class="page" aria-label="Security and recovery">
    <div class="security-hero"><h1 class="page-title">Recovery is set up.</h1><p class="subtext">Two keys, working together.<br>A way forward if either device is lost.</p></div>
    ${[['phone', 'Your phone', 'Everyday spending & vault controls', 'phone-info'], ['hardware', 'Hardware wallet', `Vault policy approved · ${state.policyDate}`, 'hardware-info'], ['cloud', 'Encrypted backup', 'Restorable with your hardware wallet', 'backup-info'], ['people', 'Recovery friends', 'Alice · backup access configured', 'friends-info']].map(([symbol, title, description, name]) => `<button class="security-row" data-action="${name}">${icon(symbol)}<span class="row-copy"><strong>${title}</strong><small>${description}</small></span><span class="status-dot" aria-label="Configured"></span></button>`).join('')}
    <div class="help-panel"><h2>Lost a device?</h2><p class="subtext">We’ll help you find the right recovery path, one step at a time.</p>${action('Get recovery help', 'recovery', 'full')}</div>
    <p class="fine-print">Keys stay with you.<br>Bitcoin enforces the vault policy.</p>
  </section>`;
}
function activityView() {
  const items = walletActivity().filter(item => activityFilter === 'all' || (activityFilter === 'received' ? item.positive : !item.positive));
  return `<section class="page" aria-label="Wallet activity"><h1 class="page-title">Your everyday activity.</h1><p class="subtext">Just the money in your wallet.</p><div class="segmented" aria-label="Filter wallet activity">${[['all', 'All'], ['sent', 'Sent'], ['received', 'Received']].map(([value, label]) => `<button data-filter="${value}" aria-pressed="${activityFilter === value}">${label}</button>`).join('')}</div>${activityRows(items)}</section>`;
}
function vaultActivityView() {
  return `<section class="page" aria-label="Private vault activity"><h1 class="page-title">Vault activity.</h1><p class="subtext">Kept here, separate from your wallet.</p>${activityRows([...state.activity, ...baseActivity].filter(item => item.type === 'vault'))}</section>`;
}
function renderChrome() {
  const privateSpace = privateViews.has(currentView);
  document.querySelector('.screen').classList.toggle('private', privateSpace);
  const headerLeft = privateSpace
    ? !vaultRevealed ? '<span class="private-label">Private</span>'
      : currentView === 'vault'
        ? `<button class="header-link" data-view="vault-activity" aria-label="View vault activity">${icon('clock')}<span>Activity</span></button>`
        : `<button class="header-link" data-view="vault">${icon('back')}<span>Vault</span></button>`
    : currentView === 'activity'
      ? `<button class="header-link" data-view="home">${icon('back')}<span>Wallet</span></button>`
      : '<button class="profile-button" data-action="wallet-options" aria-label="Wallet options">L</button>';
  document.querySelector('.app-header').innerHTML = `${headerLeft}<a class="brand" href="#home" aria-label="Anzen wallet">${mark}<span>anzen</span></a>${privateSpace ? (vaultRevealed ? `<button class="icon-button" data-action="hide-vault" aria-label="Hide vault">${icon('lock')}</button>` : '<span></span>') : `<button class="icon-button" data-action="wallet-options" aria-label="More wallet options">${icon('more')}</button>`}`;
  // Keep the same two controls mounted; detail pages belong to their parent tab.
  const nav = document.querySelector('.navigation');
  nav.querySelectorAll('button').forEach(button => {
    if (button.dataset.view === (privateSpace ? 'vault' : 'home')) button.setAttribute('aria-current', 'page');
    else button.removeAttribute('aria-current');
  });
  nav.style.setProperty('--nav-index', privateSpace ? '1' : '0');
}
function render(view = currentView) {
  if (!['home', 'vault', 'security', 'activity', 'vault-activity'].includes(view)) view = 'home';
  if (!privateViews.has(view)) vaultRevealed = false;
  if (privateViews.has(view) && !vaultRevealed) view = 'vault';
  currentView = view;
  renderChrome();
  content.innerHTML = privateViews.has(view) && !vaultRevealed ? vaultGateView() : ({ home: homeView, vault: vaultView, security: securityView, activity: activityView, 'vault-activity': vaultActivityView })[view]();
  content.scrollTop = 0;
}
function leavePrivateSpace() {
  clearTimeout(toastTimer);
  document.querySelector('#toast').hidden = true;
  closeSheet();
  vaultRevealed = false;
  delete handlers['approve-policy'];
  render('home');
}
function openSheet(title, body) {
  const wasOpen = !layer.hidden;
  const oldHeight = wasOpen ? sheet.getBoundingClientRect().height : 0;
  sheet.getAnimations().forEach(animation => animation.cancel());
  if (!wasOpen) focusBeforeSheet = document.activeElement;
  sheet.innerHTML = `<div class="sheet-heading"><h2 id="sheet-title">${title}</h2><button class="icon-button" data-close aria-label="Close dialog">${icon('close')}</button></div>${body}`;
  layer.hidden = false;
  content.inert = true;
  document.querySelector('.app-header').inert = true;
  document.querySelector('.navigation').inert = true;
  if (wasOpen && !matchMedia('(prefers-reduced-motion: reduce)').matches) {
    const newHeight = sheet.getBoundingClientRect().height;
    sheet.animate([{ height: `${oldHeight}px` }, { height: `${newHeight}px` }], { duration: 260, easing: 'cubic-bezier(.16,1,.3,1)' });
  }
  sheet.focus({ preventScroll: true });
}
function closeSheet() {
  layer.hidden = true;
  sheet.getAnimations().forEach(animation => animation.cancel());
  sheet.replaceChildren();
  content.inert = false;
  document.querySelector('.app-header').inert = false;
  document.querySelector('.navigation').inert = false;
  if (focusBeforeSheet?.isConnected) focusBeforeSheet.focus({ preventScroll: true });
  else content.focus({ preventScroll: true });
}
function notify(message) {
  const toast = document.querySelector('#toast');
  toast.textContent = message;
  toast.hidden = false;
  clearTimeout(toastTimer);
  toastTimer = setTimeout(() => { toast.hidden = true; }, 2600);
}
function success(title, message) {
  render();
  openSheet(title, `<div class="success-icon">${icon('check')}</div><p class="subtext">${message}</p><p class="notice">Demo only. No bitcoin has moved.</p>${action('Done', 'close', 'primary full')}`);
}
const details = rows => `<dl class="details">${rows.map(([label, value]) => `<div><dt>${label}</dt><dd>${value}</dd></div>`).join('')}</dl>`;
const recoverySteps = (title, steps, note) => openSheet(title, `<ol class="step-list">${steps.map(([heading, text]) => `<li>${heading}<small>${text}</small></li>`).join('')}</ol><p class="notice">${note}</p>${action('Back to recovery options', 'recovery', 'full')}`);

const handlers = {
  close: closeSheet,
  'wallet-options'() { openSheet('Your everyday wallet', `<p class="subtext">Your balance, payments, and incoming bitcoin. Nothing more to keep track of.</p>${details([['Display currency', 'USD'], ['Network', 'Demo · regtest']])}<p class="notice">This is an interactive design preview. No wallet is connected and no payments are sent.</p>${action('Done', 'close', 'primary full')}`); },
  'reveal-vault'() { vaultRevealed = true; render('vault'); },
  'hide-vault'() { closeSheet(); vaultRevealed = false; render('vault'); },
  monthly() {
    if (state.revoked || !state.monthly) return openSheet('Monthly withdrawals are off', `<p class="subtext">Your funds remain in the vault. To enable withdrawals again, propose a new policy and approve it on your hardware wallet.</p>${action('Review policy', 'edit-policy', 'primary full')}`);
    if (!state.funded) return openSheet('No monthly withdrawals funded', `<p class="subtext">Your configured emergency reserve leaves too little for a monthly withdrawal. Add funds and renew, or propose a lower limit.</p>${action('Review policy', 'edit-policy', 'primary full')}`);
    if (!state.monthlyReady && !state.claimed) return openSheet('Your first allowance is waiting', `<p class="large-number">${btc(state.monthly)} <small>BTC</small></p><p class="subtext">It unlocks about 30 days after the renewed vault confirms. A hardware approval cannot skip this Bitcoin-enforced delay.</p>${action('Got it', 'close', 'primary full')}`);
    if (state.claimed && state.funded === 1) return openSheet('All funded withdrawals used', `<p class="subtext">Your policy funded one monthly withdrawal, which you’ve used. Renew with your hardware wallet to approve a new schedule.</p>${action('Review policy', 'edit-policy', 'primary full')}`);
    if (state.claimed) return openSheet('Next allowance is waiting', `<p class="large-number">${btc(state.monthly)} <small>BTC</small></p><p class="subtext">It becomes available about 30 days after your previous withdrawal confirms. Bitcoin enforces this delay.</p><p class="notice">The next withdrawal cannot be brought forward from the phone.</p>${action('Got it', 'close', 'primary full')}${action('Stop remaining withdrawals', 'revoke', 'full')}`);
    openSheet('Your monthly withdrawal', `<p class="large-number">${btc(state.monthly)} <small>BTC</small></p>${details([['From', 'Your vault'], ['To', 'Spending wallet'], ['Available', 'Now'], ['Hardware wallet', 'Not needed']])}<p class="notice">This withdrawal is already approved by your vault policy. The next one unlocks about 30 days after this transaction confirms. Fees are omitted in this demo.</p>${action('Confirm withdrawal', 'confirm-monthly', 'primary full')}`);
  },
  'confirm-monthly'() {
    if (state.claimed || state.revoked || !state.monthlyReady || !state.funded || !state.monthly || state.monthly > state.vault) return;
    state.wallet += state.monthly;
    state.vault -= state.monthly;
    state.claimed = true;
    logActivity('Monthly withdrawal', `${btc(state.monthly)} BTC`);
    state.activity.unshift({ title: 'Received bitcoin', subtitle: 'Just now · demo confirmation', value: `+${btc(state.monthly)} BTC`, detail: fiat(state.monthly), symbol: 'down', positive: true, type: 'wallet' });
    success('Ready for everyday life', `${btc(state.monthly)} BTC moved from your vault to your spending wallet in this demo.`);
  },
  revoke() {
    openSheet('Stop remaining withdrawals?', `<p class="subtext">This stops the rest of this policy’s monthly withdrawals—not just the next one. Funds that haven’t been withdrawn stay in your vault.</p><p class="notice warning">You’ll need your hardware wallet to approve a new policy before monthly withdrawals can resume. Revocation takes effect when confirmed.</p>${action('Stop withdrawals', 'confirm-revoke', 'danger full')}${action('Keep my policy', 'close', 'full')}`);
  },
  'confirm-revoke'() {
    state.revoked = true;
    logActivity('Monthly withdrawals stopped', 'Revoked');
    success('Withdrawals stopped', 'The remaining monthly transactions can no longer execute after this demo confirmation. Your vault balance hasn’t moved.');
  },
  emergency() {
    if (state.emergencyState !== 'ready') return handlers[state.emergencyState === 'pending' ? 'emergency-status' : 'emergency-disabled']();
    openSheet('Request emergency access', `<p class="large-number">${btc(state.emergency)} <small>BTC</small></p>${details([['Destination', 'Spending wallet'], ['Waiting period', '1 week after confirmation'], ['Hardware wallet', 'Not needed']])}<p class="notice">Starting access begins a Bitcoin-enforced waiting period. The funds stay in the vault until you withdraw them after the delay. You can cancel access while you wait.</p>${action('Start waiting period', 'start-emergency', 'primary full')}${action('Not now', 'close', 'full')}`);
  },
  'start-emergency'() {
    if (state.emergencyState !== 'ready') return;
    state.emergencyState = 'pending';
    logActivity('Emergency access requested', `${btc(state.emergency)} BTC`);
    render('vault');
    handlers['emergency-status']();
  },
  'emergency-status'() {
    openSheet('Emergency access is waiting', `<p class="large-number">${btc(state.emergency)} <small>BTC</small></p>${details([['Status', 'Waiting period started'], ['Available', '1 week after confirmation'], ['Funds', 'Still in your vault']])}<p class="notice">This preview does not advance blockchain time. In the wallet, you’ll see a live countdown to when you can withdraw.</p>${action('Cancel emergency access', 'cancel-emergency', 'danger full')}${action('Keep waiting', 'close', 'full')}`);
  },
  'cancel-emergency'() {
    openSheet('Cancel emergency access?', `<p class="subtext">Once cancellation confirms, this withdrawal can no longer execute. The ${btc(state.emergency)} BTC stays in its vault output.</p><p class="notice warning">This policy’s emergency access cannot be restarted. You’ll need your hardware wallet to enable it in a new policy.</p>${action('Confirm cancellation', 'confirm-cancel-emergency', 'danger full')}${action('Keep waiting', 'close', 'full')}`);
  },
  'confirm-cancel-emergency'() {
    state.emergencyState = 'cancelled';
    logActivity('Emergency access cancelled', 'Revoked');
    success('Access cancelled', 'The emergency withdrawal is disabled after this demo confirmation. The funds remain protected in your vault.');
  },
  'emergency-disabled'() { openSheet('Access has been cancelled', `<p class="subtext">Emergency access is single-use within each vault policy. To enable it again, renew your policy with your hardware wallet.</p>${action('Review a new policy', 'edit-policy', 'primary full')}`); },
  withdraw() { openSheet('Withdraw from your vault', `<p class="subtext">Choose the access that fits what you need.</p>${action('Monthly allowance', 'monthly', 'primary full', 'repeat')}${state.emergency ? action('Emergency access · 1 week', 'emergency', 'full', 'clock') : ''}${action('With my hardware wallet', 'cooperative', 'full', 'hardware')}`); },
  cooperative() { openSheet('Withdraw with both devices', `<p class="subtext">For immediate access beyond your signed policy, your phone prepares a transaction and your hardware wallet reviews and signs it.</p>${details([['Available from vault', `${btc(state.vault)} BTC`], ['Approval', 'Phone + hardware wallet'], ['Recovery delay', 'None']])}<p class="notice">You’ll review the destination, amount, and fee on the hardware wallet. This prototype doesn’t connect to a device.</p>${action('Got it', 'close', 'primary full')}`); },
  send() {
    openSheet('Send bitcoin', `<p class="subtext">From your spending wallet. No hardware wallet needed.</p>${details([['To', 'Alex · demo contact'], ['Available', `${btc(state.wallet)} BTC`]])}<form id="send-form"><label for="send-amount">Amount in BTC</label><input id="send-amount" name="amount" inputmode="decimal" autocomplete="off" placeholder="0.002" aria-describedby="send-error"><p id="send-error" class="field-error" role="alert"></p><p class="notice">Preview only. The next step simulates confirmation; no transaction is signed or sent. Fees are omitted.</p><button class="button primary full" type="submit">Preview send</button></form>`);
  },
  receive() { showAddress(false); },
  deposit() { showAddress(true); },
  async 'copy-address'() {
    try { await navigator.clipboard.writeText(document.querySelector('.address').textContent); notify('Demo regtest address copied'); }
    catch { notify('Select the address to copy it'); }
  },
  'edit-policy'() {
    openSheet('Propose a vault policy', `<p class="subtext">Choose your limits here. They only take effect after approval on your hardware wallet and confirmation of the renewed vault.</p><form id="policy-form"><label for="monthly-limit">Monthly withdrawal · BTC</label><input id="monthly-limit" name="monthly" inputmode="decimal" value="${btc(state.monthly)}"><label for="emergency-limit">Emergency access · BTC</label><input id="emergency-limit" name="emergency" inputmode="decimal" value="${btc(state.emergency)}"><p id="policy-error" class="field-error" role="alert"></p><p class="notice">Emergency access keeps its 1-week delay. Setting either amount to 0 disables it. The phone proposes; the hardware wallet approves.</p><button class="button primary full" type="submit">Review on hardware wallet</button></form>`);
  },
  'phone-info'() { openSheet('Your everyday key', `<p class="subtext">Your phone controls your spending wallet and can execute or revoke the vault policy you already approved.</p><p class="notice">It cannot increase vault limits on its own. Changes require approval from your hardware wallet.</p>${action('Done', 'close', 'primary full')}`); },
  'hardware-info'() { openSheet('Your policy approval key', `<p class="subtext">Your hardware wallet reviews the high-level policy and signs the transactions that make it possible. Keep it offline between policy changes and annual renewal.</p>${details([['Last policy approval', state.policyDate], ['Next renewal', `By ${state.renewDate}`]])}${action('Done', 'close', 'primary full')}`); },
  'backup-info'() { openSheet('A backup, not another key holder', `<p class="subtext">Your phone seed and vault descriptor are encrypted before backup. Your hardware wallet can decrypt them to restore a lost phone.</p><p class="notice">Cloud storage cannot read the backup or sign a transaction. This demo shows a configured backup; it does not access your files.</p>${action('Done', 'close', 'primary full')}`); },
  'friends-info'() { openSheet('Your recovery friend', `${details([['Configured friend', 'Alice']])}<p class="subtext">Alice can decrypt the phone backup using her recovery key. This helps if both devices are lost.</p><p class="notice warning">A recovery friend who obtains the backup can recover your phone key. The vault’s phone-recovery delay still applies. Only choose someone you trust.</p>${action('Done', 'close', 'primary full')}`); },
  recovery() { openSheet('Let’s find your way back', `<p class="subtext">What do you still have? We’ll explain the next steps before anything moves.</p>${action('I lost my phone', 'recover-phone', 'full', 'phone')}${action('I lost my hardware wallet', 'recover-hardware', 'full', 'hardware')}${action('I lost both devices', 'recover-both', 'full', 'people')}`); },
  'recover-phone'() { recoverySteps('Replace a lost phone', [['Restore your phone key', 'Use your hardware wallet to decrypt the encrypted backup on a replacement phone.'], ['Create a new phone key', 'Keep the hardware-wallet key and your current policy.'], ['Approve the rotation', 'Move vault and spending funds to the replacement keys.']], 'With your restored phone key and hardware wallet, you can rotate immediately.'); },
  'recover-hardware'() { recoverySteps('Replace a lost hardware wallet', [['Keep using your phone', 'Previously approved allowances remain available according to your policy.'], ['Check your recovery date', 'Phone-only recovery matures 61,200 blocks (~14 months) after each vault output confirms.'], ['Move to a replacement vault', 'Once mature, use your phone to recover into a vault with a new hardware-wallet key.']], 'The delay is enforced by Bitcoin. The app cannot shorten it.'); },
  'recover-both'() { recoverySteps('Recover with a friend', [['Contact Alice', 'Use her configured recovery key and the encrypted backup to restore your phone key.'], ['Wait for phone recovery', 'Vault outputs must reach their 61,200-block (~14-month) recovery delay.'], ['Start with fresh devices', 'Move recovered funds to a replacement vault.']], 'Recovery requires a surviving seed or a configured friend and the backup. Without either, the funds cannot be recovered.'); },
};

function showAddress(vault) {
  // Public, disposable regtest examples from the project docs, never a live wallet.
  const address = vault ? 'bcrt1pg595m9x3thtd4pu6jvgaxswqg3xmwhnqs8ltdha96d67h9l8wvgsw5chrr' : 'bcrt1p9nfuddc7xj2ruerl9u476pue3tw2nl5ceztt7zekn72ew444s8nqnnwkjt';
  openSheet(vault ? 'Add bitcoin to your vault' : 'Receive in your spending wallet', `<p class="subtext">${vault ? 'Deposits go directly into cold storage. A new deposit joins your spending policy when the vault is renewed.' : 'Received funds are available to your phone once confirmed.'}</p><div class="address">${address}</div><p class="notice warning">Demo regtest address. Do not send real bitcoin. No live address or QR code is generated by this mockup.</p>${action('Copy demo address', 'copy-address', 'primary full', 'copy')}`);
}
function parseBtc(value) {
  if (!/^\d+(\.\d{1,8})?$/.test(value.trim())) return null;
  const sats = Math.round(Number(value) * 1e8);
  return Number.isSafeInteger(sats) ? sats : null;
}
document.addEventListener('click', event => {
  const target = event.target.closest('button, a.brand, [data-close]');
  if (!target || target.disabled) return;
  if (target.matches('a.brand')) { event.preventDefault(); leavePrivateSpace(); }
  else if (target.hasAttribute('data-close')) closeSheet();
  else if (target.dataset.view) { if (!privateViews.has(target.dataset.view)) leavePrivateSpace(); else closeSheet(); render(target.dataset.view); }
  else if (target.dataset.filter) { activityFilter = target.dataset.filter; render('activity'); }
  else if (target.dataset.action && handlers[target.dataset.action]) {
    if (!publicActions.has(target.dataset.action) && !vaultRevealed) { render('vault'); return; }
    handlers[target.dataset.action]();
  }
  else if (target.id === 'reset') { state = structuredClone(INITIAL); leavePrivateSpace(); notify('Demo reset'); }
});
document.addEventListener('submit', event => {
  event.preventDefault();
  if (event.target.id === 'send-form') {
    const sats = parseBtc(new FormData(event.target).get('amount'));
    if (sats === null || sats <= 0 || sats > state.wallet) {
      document.querySelector('#send-error').textContent = `Enter an amount above 0 and no more than ${btc(state.wallet)} BTC.`;
      return;
    }
    handlers['confirm-send'] = () => {
      if (sats > state.wallet) return;
      state.wallet -= sats;
      logActivity('Sent to Alex', `−${btc(sats)} BTC`, 'wallet');
      delete handlers['confirm-send'];
      success('Send preview complete', `${btc(sats)} BTC deducted from your fictional spending balance.`);
    };
    openSheet('Review your send', `${details([['To', 'Alex · demo contact'], ['Amount', `${btc(sats)} BTC`], ['From', 'Spending wallet'], ['Fee', 'Omitted in this preview']])}<p class="notice">This is a simulation. Nothing will be broadcast.</p>${action('Simulate send', 'confirm-send', 'primary full')}${action('Go back', 'send', 'full')}`);
  }
  if (event.target.id === 'policy-form') {
    if (!vaultRevealed) return;
    const form = new FormData(event.target);
    const monthly = parseBtc(form.get('monthly'));
    const emergency = parseBtc(form.get('emergency'));
    if (monthly === null || emergency === null || monthly > state.vault || emergency > state.vault) {
      document.querySelector('#policy-error').textContent = 'Enter a valid BTC amount between 0 and your vault balance.';
      return;
    }
    const funded = monthly ? Math.min(12, Math.floor((state.vault - emergency) / monthly)) : 0;
    handlers['approve-policy'] = () => {
      state.monthly = monthly;
      state.emergency = emergency;
      state.funded = funded;
      state.monthlyReady = false;
      state.policyDate = '5 Sep 2026';
      state.renewDate = '5 Sep 2027';
      state.claimed = false;
      state.revoked = false;
      state.emergencyState = 'ready';
      delete handlers['approve-policy'];
      logActivity('Vault policy renewed', 'Approved');
      render('vault');
      success('New policy approved', 'Hardware approval and vault confirmation have been simulated. The new policy is now shown on your phone.');
    };
    openSheet('Approve on your hardware wallet', `<p class="subtext">Check these values on the device—not just on your phone.</p>${details([['Vault balance', `${btc(state.vault)} BTC`], ['Monthly withdrawal', `${btc(monthly)} BTC`], ['Funded withdrawals', `${funded} of 12`], ['Emergency access', `${btc(emergency)} BTC`], ['Emergency delay', '1 week']])}${monthly && funded < 12 ? '<p class="notice warning">The vault cannot fund all 12 months. Only the funded withdrawals will be included.</p>' : ''}<p class="notice">One approval signs the annual policy and renews the vault. This button stands in for approval on a physical device.</p>${action('Simulate hardware approval', 'approve-policy', 'primary full')}${action('Back to proposal', 'edit-policy', 'full')}`);
  }
});
document.addEventListener('keydown', event => {
  if (layer.hidden) { if (event.key === 'Escape' && privateViews.has(currentView)) leavePrivateSpace(); return; }
  if (event.key === 'Escape') { event.preventDefault(); closeSheet(); }
  if (event.key === 'Tab') {
    const focusable = [...sheet.querySelectorAll('button:not(:disabled), input, a[href]')];
    const first = focusable[0];
    const last = focusable.at(-1);
    if (event.shiftKey && (document.activeElement === first || document.activeElement === sheet)) { event.preventDefault(); last?.focus(); }
    else if (!event.shiftKey && (document.activeElement === last || document.activeElement === sheet)) { event.preventDefault(); first?.focus(); }
  }
});
// Remove private DOM immediately on exit/backgrounding, rather than fading or blurring it.
// This is shoulder-surfing UX in a mockup, not an authentication/security boundary.
window.addEventListener('blur', () => { if (privateViews.has(currentView)) leavePrivateSpace(); });
document.addEventListener('visibilitychange', () => { if (document.hidden && privateViews.has(currentView)) leavePrivateSpace(); });
render();
