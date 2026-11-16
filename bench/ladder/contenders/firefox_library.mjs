// Persistent library endpoint for the matched Firefox operations.
// Python times the control pipe round-trip; verification reads are separate.
import { createInterface } from 'node:readline';
import { writeFile } from 'node:fs/promises';

const kind = process.argv[2];
let input;
let browser, page;
let version;
let closed = false;
async function start() {
  if (kind === 'playwright') {
    const { firefox } = await import('playwright');
    browser = await firefox.launch({ headless: true });
    page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    version = browser.version();
  } else if (kind === 'puppeteer') {
    const { default: puppeteer } = await import('puppeteer-core');
    browser = await puppeteer.launch({ browser: 'firefox', protocol: 'webDriverBiDi',
      executablePath: process.env.NAVIGERA_FIREFOX, headless: true });
    page = await browser.newPage();
    await page.setViewport({ width: 1440, height: 900 });
    version = await browser.version();
  } else if (kind === 'webdriverio') {
    const { remote } = await import('webdriverio');
    browser = await remote({ logLevel: 'error', capabilities: { browserName: 'firefox',
      webSocketUrl: true,
      'moz:firefoxOptions': { binary: process.env.NAVIGERA_FIREFOX, args: ['-headless'] } } });
    if (!browser.isBidi) throw new Error('WebdriverIO did not negotiate BiDi');
    await browser.setWindowSize(1440, 900);
    version = browser.capabilities.browserVersion;
  } else if (kind === 'vibium') {
    // Vibium requests Firefox's privileged accessibility endpoint. Only its
    // disposable benchmark browser inherits this flag.
    process.env.MOZ_REMOTE_ALLOW_SYSTEM_ACCESS = '1';
    const { firefox } = await import('vibium');
    browser = await firefox.start({ headless: true });
    page = await browser.page();
    version = await page.evaluate('navigator.userAgent');
  } else throw new Error(`unknown library ${kind}`);
  return { browser: version, interface: 'persistent library API (JSON-line control pipe included)',
    browser_build: kind === 'playwright' ? 'Playwright bundled patched Firefox' : kind === 'vibium' ? 'Vibium managed Firefox' : 'runner Firefox' };
}
async function evaluate(expr) {
  if (kind === 'webdriverio') return browser.execute(`return (${expr});`);
  return page.evaluate(expr);
}
function locator(selector) {
  const target = page.locator(selector);
  // Fixtures are static and rest between operations. Avoid Puppeteer's
  // extra two-frame stability wait; leave visibility/enabled checks intact.
  return kind === 'puppeteer' ? target.setWaitForStableBoundingBox(false) : target;
}
async function fill(selector, value) {
  if (kind === 'webdriverio') return (await browser.$(selector)).setValue(value);
  if (kind === 'vibium') return page.find(selector).fill(value);
  return locator(selector).fill(value);
}
async function click(selector) {
  if (kind === 'webdriverio') return (await browser.$(selector)).click();
  if (kind === 'vibium') return page.find(selector).click();
  return locator(selector).click();
}
async function close() {
  if (!browser || closed) return;
  closed = true;
  if (kind === 'webdriverio') await browser.deleteSession();
  else if (kind === 'vibium') await browser.stop();
  else await browser.close();
}
async function perform(req) {
  switch (req.op) {
    case 'eval': return evaluate(req.expr);
    case 'goto':
      if (kind === 'webdriverio') return browser.url(req.url);
      if (kind === 'vibium') return page.go(req.url);
      return page.goto(req.url, { waitUntil: 'load' }).then(() => null);
    case 'fill': return fill(req.selector, req.value);
    case 'click': return click(req.selector);
    case 'snapshot':
      if (kind === 'playwright') return page.locator('body').ariaSnapshot();
      if (kind === 'puppeteer') throw new Error('not run: Puppeteer BiDi has no accessibility snapshot API');
      if (kind === 'vibium') return JSON.stringify(await page.a11yTree());
      throw new Error('not run: WebdriverIO has no Firefox accessibility snapshot API');
    case 'screenshot':
      if (kind === 'webdriverio') await browser.saveScreenshot(req.path);
      else if (kind === 'vibium') await writeFile(req.path, await page.screenshot());
      else await page.screenshot({ path: req.path });
      return null;
    case 'form-batch':
      await fill('#cust', 'ada');
      await fill('#qty', '2');
      await click('#place');
      return evaluate("document.getElementById('status').textContent");
    case 'close': await close(); return null;
    default: throw new Error(`unknown operation ${req.op}`);
  }
}
try {
  console.log(JSON.stringify({ ok: true, info: await start() }));
  input = createInterface({ input: process.stdin });
  for await (const line of input) {
    const req = JSON.parse(line);
    const t0 = performance.now();
    try {
      const value = await perform(req);
      console.log(JSON.stringify({ ok: true, value: value ?? null, ms: performance.now() - t0 }));
    } catch (e) {
      console.log(JSON.stringify({ ok: false, error: String(e), ms: performance.now() - t0 }));
    }
    if (req.op === 'close') break;
  }
} catch (e) {
  console.log(JSON.stringify({ ok: false, error: String(e) }));
  process.exitCode = 1;
} finally {
  input?.close();
  process.stdin.pause();
  process.stdin.unref?.();
  await close().catch(e => console.error(String(e)));
}
