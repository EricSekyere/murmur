// Run against scripts/ui-preview.cjs; Playwright is an optional development tool.
const assert = require('node:assert/strict');
const fs = require('node:fs');
const { execFileSync } = require('node:child_process');
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');
const base = process.env.MURMUR_PREVIEW_URL || 'http://127.0.0.1:4173';

async function setTheme(page, theme) {
  if (await page.locator('html').getAttribute('data-theme') !== theme) {
    await page.locator('#theme-toggle').click();
  }
}

async function checkLayouts(page) {
  let cases = 0;
  for (const size of [{ width: 760, height: 540 }, { width: 960, height: 640 }, { width: 1440, height: 900 }]) {
    await page.setViewportSize(size);
    for (const theme of ['dark', 'light']) {
      await setTheme(page, theme);
      for (const view of ['home', 'analytics', 'settings', 'diagnostics', 'help']) {
        await page.locator('#workspace-toggle').click();
        await page.locator(`#workspace-menu [data-view=${view}]`).click();
        assert(await page.locator(`#view-${view}`).evaluate(e => e.classList.contains('view--active')));
        assert.equal(await page.locator(`.nav__item[data-view=${view}]`).getAttribute('aria-current'), 'page');
        assert.equal(await page.locator('#workspace-toggle').getAttribute('aria-expanded'), 'false');
        assert.equal(await page.evaluate(() => document.activeElement.id), 'workspace-toggle');
        assert(await page.evaluate(() => document.documentElement.scrollWidth <= innerWidth));
        const rect = await page.locator('#workspace-toggle').boundingBox();
        assert(rect.y >= 0 && rect.y + rect.height <= 52);
        cases++;
      }
      await page.locator('#window-menu').click();
      await page.locator('#application-menu [data-menu-id=Dictation]').click();
      const parent = await page.locator('#application-menu').boundingBox();
      const child = await page.locator('#application-submenu').boundingBox();
      assert(child.x >= parent.x + parent.width);
      assert(child.x + child.width <= size.width && child.y + child.height <= size.height);
      await page.keyboard.press('Escape'); await page.keyboard.press('Escape');
      const button = await page.locator('#theme-toggle').boundingBox();
      assert(button.y + button.height <= size.height);
    }
  }
  return cases;
}

async function checkKeyboard(page) {
  const focusedView = () => page.evaluate(() => document.activeElement.dataset.view);
  await page.locator('.nav__item[data-view=home]').click();
  await page.locator('#workspace-toggle').focus();
  await page.keyboard.press('ArrowDown');
  assert.equal(await focusedView(), 'home');
  await page.keyboard.press('End');
  assert.equal(await focusedView(), 'help');
  await page.keyboard.press('ArrowDown');
  assert.equal(await focusedView(), 'home');
  await page.keyboard.press('ArrowUp');
  assert.equal(await focusedView(), 'help');
  await page.keyboard.press('Home');
  await page.keyboard.press('ArrowDown');
  await page.keyboard.press('Enter');
  assert.equal(await page.locator('#workspace-current').textContent(), 'Analytics');
  for (const key of ['Escape', 'Tab']) {
    await page.locator('#workspace-toggle').click();
    await page.keyboard.press(key);
    assert(await page.locator('#workspace-menu').isHidden());
  }
  await page.locator('#workspace-toggle').click();
  await page.locator('#status-badge').click();
  assert(await page.locator('#workspace-menu').isHidden());
  await page.evaluate(() => previewEmit('navigate-view', { view: 'settings' }));
  assert.equal(await page.locator('#workspace-current').textContent(), 'Settings');
}

async function checkWindowActions(page) {
  await page.locator('#window-maximize').click();
  await page.waitForFunction(() => document.getElementById('window-maximize').title === 'Restore');
  await page.locator('#window-maximize').click();
  await page.waitForFunction(() => document.getElementById('window-maximize').title === 'Maximize');
  await page.locator('#window-minimize').click();
  await page.locator('#window-close').click();
  await page.locator('#window-menu').click();
  assert(await page.locator('#application-menu').isVisible());
  await page.keyboard.press('Escape');
  const actions = await page.evaluate(() => previewWindowActions);
  assert.deepEqual(actions.filter(a => !a.startsWith('theme:')), ['maximize', 'maximize', 'minimize', 'close']);
}

async function checkPreservedControls(page) {
  const baseline = execFileSync('git', ['show', 'main:crates/murmur-app/frontend/index.html'], { encoding: 'utf8' });
  const contract = await page.evaluate(html => {
    const old = new DOMParser().parseFromString(html, 'text/html');
    const controls = doc => [...doc.querySelectorAll('input[id],select[id],textarea[id]')]
      .filter(e => e.id !== 'theme-select').map(e => ({
        id: e.id, type: e.getAttribute('type'), min: e.getAttribute('min'),
        max: e.getAttribute('max'), step: e.getAttribute('step'),
        options: e.id === 'audio-device-select' ? [] : [...e.querySelectorAll('option')].map(o => o.value),
      }));
    return {
      missing: [...old.querySelectorAll('[id]')].filter(e => !document.getElementById(e.id)).map(e => e.id),
      before: controls(old), after: controls(document),
    };
  }, baseline);
  assert.deepEqual(contract.missing, []);
  assert.deepEqual(contract.before, contract.after);
  return contract.before.length;
}

async function main() {
  const browser = await chromium.launch({
    headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}),
  });
  try {
    const page = await browser.newPage({ viewport: { width: 960, height: 640 } });
    const errors = [];
    page.on('pageerror', error => errors.push(error.message));
    let cases = 0;
    for (const platform of ['windows', 'linux', 'macos']) {
      await page.goto(`${base}/?platform=${platform}`);
      await page.waitForFunction(() => !!document.documentElement.dataset.platform);
      assert.equal(await page.locator('#window-controls').isVisible(), platform !== 'macos');
      cases += await checkLayouts(page);
      await checkKeyboard(page);
      if (platform !== 'macos') await checkWindowActions(page);
    }
    const preservedControls = await checkPreservedControls(page);
    await page.goto(base);
    await page.setViewportSize({ width: 1120, height: 800 });
    fs.mkdirSync('target', { recursive: true });
    for (const theme of ['dark', 'light']) {
      await setTheme(page, theme);
      await page.screenshot({ path: `target/chrome-${theme}.png` });
      await page.locator('#workspace-toggle').click();
      await page.screenshot({ path: `target/chrome-${theme}-menu.png` });
      await page.keyboard.press('Escape');
    }
    assert.deepEqual(errors, []);
    const report = { cases, preservedControls, errors, keyboard: true, windowActions: 'mocked; native verification separate' };
    fs.writeFileSync('target/chrome-browser-report.json', JSON.stringify(report, null, 2));
    console.log(report);
  } finally {
    await browser.close();
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
