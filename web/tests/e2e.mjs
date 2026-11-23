// End-to-end: the web build against a demo Relay in token mode, in headless Chromium.
//
//   node web/tests/e2e.mjs [--dist web/dist] [--relay-bin target/debug/ravenglass-relay] [--out web/tests/out]
//
// Starts `ravenglass-relay` (RAVENGLASS_RELAY_DEMO=1) on 127.0.0.1:8743 and serves the dist folder
// on 127.0.0.1:8744; build the dist with `tools/web-build.sh --relay http://127.0.0.1:8743`.
// Then, in token mode (demo accounts): sign in with the token, wait for list rows, `j`, `enter`,
// check the reader shows the message, reload (the tab keeps the token). In users mode (a fresh
// data dir, dev cookies on loopback): a wrong password is refused, the right one signs in with an
// HttpOnly cookie the page cannot read, and a reload stays signed in. Screenshots at
// deviceScaleFactor 1 go to --out.
// The canvas has no DOM, so the page's `data-rg-*` marks and its live region are read instead
// (app/src/web/host.rs, `mirror`). Exits non-zero on the first failed check.
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { readFile, mkdir, mkdtemp, rm } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { createRequire } from 'node:module';
import { extname, join, normalize, resolve } from 'node:path';
import { gzipSync } from 'node:zlib';

const require = createRequire(import.meta.url);
const { chromium } = require(process.env.PLAYWRIGHT_MODULE || 'playwright');

const arg = (name, fallback) => { const i = process.argv.indexOf(name); return i > 0 ? process.argv[i + 1] : fallback; };
const DIST = resolve(arg('--dist', 'web/dist'));
const RELAY_BIN = resolve(arg('--relay-bin', 'target/debug/ravenglass-relay'));
const OUT = resolve(arg('--out', 'web/tests/out'));
const RELAY_PORT = Number(process.env.RELAY_PORT || 8743);
const WEB_PORT = Number(process.env.WEB_PORT || 8744);
const ORIGIN = `http://127.0.0.1:${WEB_PORT}`;
// A test-only token for a Relay that lives as long as this script; at least 16 characters.
const TOKEN = 'e2e-demo-relay-token-0001';

const checks = [];
const fail = (message) => { throw new Error(message); };
const check = (name, ok, detail = '') => { checks.push({ name, ok, detail }); if (!ok) fail(`${name} ${detail}`); };

const TYPES = { '.html': 'text/html; charset=utf-8', '.js': 'text/javascript', '.mjs': 'text/javascript', '.css': 'text/css', '.wasm': 'application/wasm',
    '.json': 'application/json', '.webmanifest': 'application/manifest+json', '.svg': 'image/svg+xml', '.png': 'image/png', '.ttf': 'font/ttf' };

function serve(dir, port) {
    const server = createServer(async (req, res) => {
        const path = decodeURIComponent(new URL(req.url, ORIGIN).pathname);
        const file = normalize(join(dir, path.endsWith('/') ? `${path}index.html` : path));
        if (!file.startsWith(dir)) { res.writeHead(403).end(); return; }
        try {
            const body = await readFile(file);
            res.writeHead(200, { 'content-type': TYPES[extname(file)] || 'application/octet-stream', 'cache-control': 'no-store' }).end(body);
        } catch { res.writeHead(404).end('not found'); }
    });
    return new Promise((ok) => server.listen(port, '127.0.0.1', () => ok(server)));
}

const LISTEN = `127.0.0.1:${RELAY_PORT}`;
// Token mode: the demo accounts behind a shared token.
const TOKEN_MODE = { RAVENGLASS_RELAY_AUTH: 'token', RAVENGLASS_RELAY_TOKEN: TOKEN, RAVENGLASS_RELAY_DEMO: '1', RAVENGLASS_RELAY_LISTEN: LISTEN, RAVENGLASS_RELAY_ORIGIN: ORIGIN };
// Users mode: one user, no mail accounts, dev cookies (no `Secure`) on the loopback listener.
const USER = 'odin';
const PASSWORD = 'e2e-password-0001';
const usersMode = (dir) => ({ RAVENGLASS_RELAY_AUTH: 'users', RAVENGLASS_RELAY_DATA_DIR: dir, RAVENGLASS_RELAY_LISTEN: LISTEN, RAVENGLASS_RELAY_ORIGIN: ORIGIN, RAVENGLASS_RELAY_INSECURE_DEV_COOKIES: '1' });

function startRelay(settings) {
    const relay = spawn(RELAY_BIN, [], { env: { ...process.env, ...settings }, stdio: ['ignore', 'inherit', 'pipe'] });
    return new Promise((ok, no) => {
        relay.on('error', no);
        relay.stderr.on('data', (chunk) => { process.stderr.write(chunk); if (String(chunk).includes('listening')) ok(relay); });
        relay.on('exit', (code) => no(new Error(`the relay exited (${code})`)));
    });
}

function addUser(settings) {
    return new Promise((ok, no) => {
        const add = spawn(RELAY_BIN, ['user', 'add', USER], { env: { ...process.env, ...settings }, stdio: ['pipe', 'inherit', 'inherit'] });
        add.on('error', no);
        add.on('exit', (code) => (code === 0 ? ok() : no(new Error(`user add exited (${code})`))));
        add.stdin.end(`${PASSWORD}\n`);
    });
}

function stop(relay) { return new Promise((ok) => { relay.removeAllListeners('exit'); relay.on('exit', ok); relay.kill(); }); }

const mark = (page, name) => page.evaluate((n) => document.documentElement.getAttribute(`data-rg-${n}`) ?? '', name);
async function until(page, what, test, timeout = 30000) {
    const start = Date.now();
    for (;;) {
        const value = await test();
        if (value) return value;
        if (Date.now() - start > timeout) fail(`timed out waiting for ${what}`);
        await page.waitForTimeout(100);
    }
}

await mkdir(OUT, { recursive: true });
const wasm = await readFile(join(DIST, 'app_bg.wasm'));
const size = { raw: wasm.length, gzip: gzipSync(wasm, { level: 9 }).length };
const web = await serve(DIST, WEB_PORT);
const browser = await chromium.launch({ headless: true, args: ['--enable-unsafe-webgpu'] });
const problems = [];
let relay = null;
let ok = false;
const summary = { wasm: size };

async function openPage() {
    const context = await browser.newContext({ viewport: { width: 1400, height: 880 }, deviceScaleFactor: 1, colorScheme: 'dark' });
    const page = await context.newPage();
    page.on('pageerror', (error) => problems.push(`pageerror: ${error.message}`));
    page.on('console', (msg) => { if (msg.type() === 'error') problems.push(`console: ${msg.text()}`); });
    return page;
}

async function tokenMode() {
    relay = await startRelay(TOKEN_MODE);
    const page = await openPage();
    const started = Date.now();
    await page.goto(`${ORIGIN}/`);
    await until(page, 'the sign-in screen', async () => (await mark(page, 'session')) === 'signin');
    summary.signInScreenMs = Date.now() - started;
    await page.screenshot({ path: join(OUT, '1-signin-token.png') });

    // A token-mode Relay offers only the token field, which has the focus.
    await page.mouse.click(700, 120);
    await page.keyboard.type(TOKEN);
    await page.screenshot({ path: join(OUT, '2-token-typed.png') });
    const domHasToken = await page.evaluate((t) => document.documentElement.outerHTML.includes(t), TOKEN);
    check('the typed token is not in the DOM', !domHasToken);
    await page.keyboard.press('Enter');
    await until(page, 'a session', async () => (await mark(page, 'session')) === 'ready');
    summary.rows = Number(await until(page, 'list rows', async () => Number(await mark(page, 'rows')) > 0 && mark(page, 'rows')));
    summary.firstRowsMs = Date.now() - started;
    check('the list shows rows from the relay', summary.rows > 0, `rows=${summary.rows}`);
    await page.waitForTimeout(500);
    await page.screenshot({ path: join(OUT, '3-list.png') });

    await page.keyboard.press('j');
    await until(page, 'the selection to move', async () => (await mark(page, 'selected')) === '1');
    check('j moves the selection down', true);
    await page.keyboard.press('Enter');
    summary.open = await until(page, 'the reader', async () => (await mark(page, 'body')) === 'shown' && mark(page, 'open'));
    const heard = await page.locator('#rg-status').textContent();
    check('enter opens the message in the reader', Boolean(summary.open) && heard.includes('reading:'), `open=${summary.open} status="${heard}"`);
    await page.waitForTimeout(800);
    await page.screenshot({ path: join(OUT, '4-reader.png') });

    const kept = await page.evaluate(() => [sessionStorage.length > 0, localStorage.getItem('ravenglass.relay-token')]);
    check('the token is kept in session storage only', kept[0] && kept[1] === null);
    await page.reload();
    await until(page, 'the session after a reload', async () => (await mark(page, 'session')) === 'ready');
    check('a reload in the same tab stays signed in (token)', true);
    await until(page, 'list rows after the reload', async () => Number(await mark(page, 'rows')) > 0);
    await page.waitForTimeout(500);
    await page.screenshot({ path: join(OUT, '5-reloaded.png') });

    // Offline: the service worker serves the app shell; the page says the Relay is out of reach.
    await page.evaluate(() => navigator.serviceWorker.ready.then(() => true));
    await page.context().setOffline(true);
    await page.reload();
    await until(page, 'an offline start from the service worker', async () => (await mark(page, 'session')) === 'signin-error', 60000);
    check('the app starts offline from the service worker', true);
    await page.waitForTimeout(300);
    await page.screenshot({ path: join(OUT, '8-offline.png') });
    await page.context().setOffline(false);
    await page.context().close();
    await stop(relay);
    relay = null;
}

async function usersMode_() {
    const dir = await mkdtemp(join(tmpdir(), 'ravenglass-relay-e2e-'));
    const settings = usersMode(dir);
    await addUser(settings);
    relay = await startRelay(settings);
    const page = await openPage();
    await page.goto(`${ORIGIN}/`);
    await until(page, 'the users-mode sign-in screen', async () => (await mark(page, 'session')) === 'signin');
    await page.mouse.click(700, 120);
    await page.keyboard.type(USER);
    await page.keyboard.press('Tab');
    await page.keyboard.type('not-the-password');
    await page.keyboard.press('Enter');
    await until(page, 'the refusal', async () => (await mark(page, 'session')) === 'signin-error');
    check('a wrong password is refused', true);
    await page.screenshot({ path: join(OUT, '6-users-refused.png') });
    // The username stays; the password field has the focus again, empty.
    await page.keyboard.type(PASSWORD);
    await page.keyboard.press('Enter');
    await until(page, 'a users-mode session', async () => (await mark(page, 'session')) === 'ready');
    check('username and password sign in (cookie session)', true);
    const cookies = await page.context().cookies(`http://127.0.0.1:${RELAY_PORT}`);
    check('the session cookie is HttpOnly', cookies.some((c) => c.httpOnly && c.name.includes('ravenglass-session')), JSON.stringify(cookies.map((c) => c.name)));
    const scriptSees = await page.evaluate(() => document.cookie);
    check('the page cannot read the session cookie', !scriptSees.includes('ravenglass-session'));
    await page.waitForTimeout(500);
    await page.screenshot({ path: join(OUT, '7-users-signed-in.png') });
    await page.reload();
    await until(page, 'the cookie session after a reload', async () => (await mark(page, 'session')) === 'ready');
    check('a reload stays signed in (cookie)', true);
    await page.context().close();
    await stop(relay);
    relay = null;
    await rm(dir, { recursive: true, force: true });
}

try {
    await tokenMode();
    await usersMode_();
    // Rune's own console noise that is not ours: SwiftShader lacks some pipelines.
    const csp = problems.filter((p) => p.includes('Content Security Policy'));
    check('no Content-Security-Policy violations', csp.length === 0, csp.join('\n'));
    const errors = problems.filter((p) => p.startsWith('pageerror'));
    check('no uncaught page errors', errors.length === 0, errors.join('\n'));
    ok = true;
    console.log(JSON.stringify({ ok, ...summary, checks: checks.map((c) => c.name), console: problems }, null, 2));
} catch (error) {
    console.error(JSON.stringify({ ok, error: error.message, checks, problems, ...summary }, null, 2));
    process.exitCode = 1;
} finally {
    await browser.close();
    web.close();
    if (relay) relay.kill();
}
