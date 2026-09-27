// Compare the menu UI with the original Windows command inventory.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const baseline = require('./menu-baseline.json');
const previewUrl = process.env.MURMUR_PREVIEW_URL || 'http://127.0.0.1:4173';
const windowsGroups = ['File', 'Dictation', 'Edit', 'View', 'Help'];

function normalize(label) {
  return label.split('\t')[0].replaceAll('&', '').replaceAll('\u2026', '...')
    .replace(/^(Start|Stop) dictation$/, 'Dictation toggle')
    .replace(/^(Start Meeting|Start meeting|Meeting unavailable \(dictating\))$/, 'Meeting toggle');
}

async function dismiss(page, native = false) {
  await page.keyboard.press('Escape');
  if (!native) await page.keyboard.press('Escape');
}

async function openGroup(page, label) {
  await page.locator('#window-menu').click();
  await page.locator('#application-menu [data-menu-id]').filter({ hasText: label }).click();
}

async function checkInventory(page) {
  let count = 0;
  for (const [label, expected] of Object.entries(baseline)) {
    await page.locator('#window-menu').click();
    assert.deepEqual(await page.locator('#application-menu [data-menu-id] > span').allTextContents(), windowsGroups);
    await page.locator('#application-menu [data-menu-id]').filter({ hasText: label }).click();
    const selector = label === 'Edit' ? '#preview-app-submenu' : '#application-submenu';
    const labels = await page.locator(`${selector} [data-command] > span`).allTextContents();
    assert.deepEqual(labels.map(normalize), expected.map(normalize));
    count += labels.length;
    if (label === 'Dictation') await page.screenshot({ path: 'target/styled-menu-dark.png' });
    if (label !== 'Edit') assert(await page.locator('#application-menu').isVisible());
    await dismiss(page, label === 'Edit');
  }
  return count;
}

async function checkInteraction(page) {
  await page.locator('#window-menu').click();
  await page.keyboard.press('End');
  assert.equal(await page.evaluate(() => document.activeElement.textContent), 'Open system menu');
  await page.keyboard.press('Home');
  await page.keyboard.press('ArrowRight');
  assert.equal(await page.locator('#application-submenu').getAttribute('aria-label'), 'File');
  await page.keyboard.press('ArrowLeft');
  assert(await page.locator('#application-submenu').count() === 0);
  assert.equal(await page.evaluate(() => document.activeElement.dataset.menuId), 'File');
  await page.keyboard.press('ArrowRight');
  await page.keyboard.press('End');
  assert.equal(await page.evaluate(() => document.activeElement.dataset.command), 'app:quit');
  await page.keyboard.press('Home');
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('#workspace-current').textContent(), 'Settings');
  await page.locator('#theme-toggle').click();
  await openGroup(page, 'Dictation');
  await page.screenshot({ path: 'target/styled-menu-light.png' });
  await dismiss(page);
  await page.evaluate(() => __TAURI__.core.invoke('clear_history'));
  await openGroup(page, 'Dictation');
  assert(await page.locator('[data-command="app:copy-last"]').isDisabled());
  await dismiss(page);
}

async function checkPlatformGroups(page) {
  for (const platform of ['linux', 'macos']) {
    const url = new URL(previewUrl); url.searchParams.set('platform', platform);
    await page.goto(url.href);
    await page.waitForFunction(() => !!document.documentElement.dataset.platform);
    await page.locator('#window-menu').click();
    const groups = await page.locator('#application-menu [data-menu-id] > span').allTextContents();
    assert.deepEqual(groups, platform === 'macos'
      ? ['Murmur', 'Dictation', 'Edit', 'View', 'Window', 'Help'] : windowsGroups);
    await page.locator('#application-menu [data-menu-id=Dictation]').click();
    assert(await page.locator('[data-command="app:transcribe-file"]').isVisible());
    await dismiss(page);
  }
}

async function checkNativeFallback(page) {
  // Metadata failure must leave the complete native menu accessible.
  await page.evaluate(() => {
    const invoke = __TAURI__.core.invoke;
    __TAURI__.core.invoke = (command, args) => command === 'window_menu_groups'
      ? Promise.reject(Error('test failure')) : invoke(command, args);
  });
  await page.locator('#window-menu').click();
  assert(await page.locator('#preview-app-menu').isVisible());
  await page.keyboard.press('Escape');
}

async function checkDelayedStartup(page) {
  await page.addInitScript(() => {
    let bridge;
    Object.defineProperty(window, '__TAURI__', {
      configurable: true,
      get: () => bridge,
      set(value) {
        bridge = value; const invoke = value.core.invoke; let ready = false;
        window.menuAttempts = 0;
        value.core.invoke = (command, args) => {
          if (command === 'window_menu_groups' && !ready) {
            window.menuAttempts++; return Promise.reject('Application menu is not ready');
          }
          return invoke(command, args);
        };
        window.finishMenuStartup = () => { ready = true; previewEmit('application-menu-ready', {}); };
      },
    });
  });
  await page.goto(previewUrl);
  await page.waitForFunction(() => window.menuAttempts > 0);
  assert.equal(await page.locator('html').getAttribute('data-platform'), null);
  await page.evaluate(() => window.finishMenuStartup());
  await page.waitForFunction(() => document.documentElement.dataset.platform === 'windows');
  await openGroup(page, 'Dictation');
  assert(await page.locator('#application-submenu').isVisible());
  await dismiss(page);
}

(async () => {
  fs.mkdirSync('target', { recursive: true });
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}),
  });
  try {
    const page = await browser.newPage({ viewport: { width: 960, height: 760 } });
    const errors = []; page.on('pageerror', error => errors.push(error.message));
    await page.goto(previewUrl);
    await page.waitForFunction(() => document.documentElement.dataset.platform === 'windows');
    const count = await checkInventory(page);
    await checkInteraction(page);
    await checkPlatformGroups(page);
    await checkNativeFallback(page);
    await checkDelayedStartup(page);
    assert.deepEqual(errors, []);
    const report = { preservedProductionCommands: count, keyboard: true, disabledStateRefresh: true, nativeFallback: true, delayedStartup: true, errors };
    fs.writeFileSync('target/styled-menu-report.json', JSON.stringify(report, null, 2));
    console.log(report);
  } finally { await browser.close(); }
})().catch(error => { console.error(error); process.exitCode = 1; });
