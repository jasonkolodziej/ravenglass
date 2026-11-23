// End-to-end: the web build against a demo Relay in token mode, in headless Chromium.
//
//   node web/tests/e2e.mjs [--dist web/dist] [--relay-bin target/debug/ravenglass-relay] [--out web/tests/out]
//
// Starts `ravenglass-relay` (RAVENGLASS_RELAY_DEMO=1) on 127.0.0.1:8743 and serves the dist folder
// on 127.0.0.1:8744; build the dist with `tools/web-build.sh --relay http://127.0.0.1:8743`.
// Then: sign in with the token, wait for list rows, `j`, `enter`, check the reader shows the
// message, reload (the tab keeps the token) and screenshot each step at deviceScaleFactor 1.
// The canvas has no DOM, so the page's `data-rg-*` marks and its live region are read instead
// (app/src/web/host.rs, `mirror`). Exits non-zero on the first failed check.
import { spawn } from 'node:child_process';
import { createServer } from 'node:http';
import { readFile, mkdir } from 'node:fs/promises';
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

function startRelay() {
    const relay = spawn(RELAY_BIN, [], {
        env: { ...process.env, RAVENGLASS_RELAY_TOKEN: TOKEN, RAVENGLASS_RELAY_DEMO: '1', RAVENGLASS_RELAY_LISTEN: `127.0.0.1:${RELAY_PORT}`, RAVENGLASS_RELAY_ORIGIN: ORIGIN },
        stdio: ['ignore', 'inherit', 'pipe'],
    });
    return new Promise((ok, no) => {
        relay.on('error', no);
        relay.stderr.on('data', (chunk) => { process.stderr.write(chunk); if (String(chunk).includes('listening')) ok(relay); });
        relay.on('exit', (code) => no(new Error(`the relay exited (${code})`)));
    });
}

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
const relay = await startRelay();
const web = await serve(DIST, WEB_PORT);
const browser = await chromium.launch({ headless: true, args: ['--enable-unsafe-webgpu'] });
const problems = [];
let ok = false;
try {
    const context = await browser.newContext({ viewport: { width: 1400, height: 880 }, deviceScaleFactor: 1, colorScheme: 'dark' });
    const page = await context.newPage();
    page.on('pageerror', (error) => problems.push(`pageerror: ${error.message}`));
    page.on('console', (msg) => { if (msg.type() === 'error') problems.push(`console: ${msg.text()}`); });
    const started = Date.now();
    await page.goto(`${ORIGIN}/`);
    await until(page, 'the sign-in screen', async () => (await mark(page, 'session')) === 'signin');
    const firstFrame = Date.now() - started;
    await page.screenshot({ path: join(OUT, '1-signin.png') });

    // The token field is the last; ↑ from the first field wraps to it.
    await page.mouse.click(700, 120);
    await page.keyboard.press('ArrowUp');
    await page.keyboard.type(TOKEN);
    await page.screenshot({ path: join(OUT, '2-token-typed.png') });
    const domHasToken = await page.evaluate((t) => document.documentElement.outerHTML.includes(t), TOKEN);
    check('the typed token is not in the DOM', !domHasToken);
    await page.keyboard.press('Enter');
    await until(page, 'a session', async () => (await mark(page, 'session')) === 'ready');
    const rows = Number(await until(page, 'list rows', async () => Number(await mark(page, 'rows')) > 0 && mark(page, 'rows')));
    check('the list shows rows from the relay', rows > 0, `rows=${rows}`);
    await page.waitForTimeout(500);
    await page.screenshot({ path: join(OUT, '3-list.png') });

    await page.keyboard.press('j');
    await until(page, 'the selection to move', async () => (await mark(page, 'selected')) === '1');
    check('j moves the selection down', true);
    await page.keyboard.press('Enter');
    const open = await until(page, 'the reader', async () => (await mark(page, 'body')) === 'shown' && mark(page, 'open'));
    const heard = await page.locator('#rg-status').textContent();
    check('enter opens the message in the reader', Boolean(open) && heard.includes('reading:'), `open=${open} status="${heard}"`);
    await page.waitForTimeout(800);
    await page.screenshot({ path: join(OUT, '4-reader.png') });

    const kept = await page.evaluate(() => [sessionStorage.length > 0, localStorage.getItem('ravenglass.relay-token')]);
    check('the token is kept in session storage only', kept[0] && kept[1] === null);
    await page.reload();
    await until(page, 'the session after a reload', async () => (await mark(page, 'session')) === 'ready');
    check('a reload in the same tab stays signed in', true);
    await until(page, 'list rows after the reload', async () => Number(await mark(page, 'rows')) > 0);
    await page.waitForTimeout(500);
    await page.screenshot({ path: join(OUT, '5-reloaded.png') });

    const csp = problems.filter((p) => p.includes('Content Security Policy'));
    check('no Content-Security-Policy violations', csp.length === 0, csp.join('\n'));
    const errors = problems.filter((p) => p.startsWith('pageerror'));
    check('no uncaught page errors', errors.length === 0, errors.join('\n'));
    ok = true;
    console.log(JSON.stringify({ ok, firstSignInScreenMs: firstFrame, rows, open, wasm: size, checks: checks.map((c) => c.name), console: problems }, null, 2));
} catch (error) {
    console.error(JSON.stringify({ ok, error: error.message, checks, problems, wasm: size }, null, 2));
    process.exitCode = 1;
} finally {
    await browser.close();
    web.close();
    relay.kill();
}
