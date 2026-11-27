/**
 * The pages the server renders (src/lib/serverPages.ts), on the built server
 * (dist/server/entry.mjs, after `pnpm build`), against one stub (node:http)
 * standing in for the API's internal listener, which answers the plans of a
 * fixture, and for Supabase Auth, which accepts every sign-in mail and refuses
 * every code: every page in every locale with its language, canonical URL,
 * alternates, language switch and content security policy, and no script;
 * then what /pro/ shows, and the sign-in forms. Signing in for real, by the
 * code and the link of a real mail, is script/e2e-sign-in.ts, against the
 * local stack.
 */
import assert from 'node:assert/strict';
import { type ChildProcess, spawn } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { createServer, type Server } from 'node:http';
import type { AddressInfo } from 'node:net';
import { after, before, test } from 'node:test';
import { defaultLocale, locales, type Locale } from '../src/i18n/config.ts';
import en from '../src/i18n/messages/en.ts';
import ru from '../src/i18n/messages/ru.ts';
import { formatPrice } from '../src/lib/price.ts';
import { serverPages } from '../src/lib/serverPages.ts';
import { siteUrl } from '../src/lib/site.ts';
import { cspProblems } from './csp.ts';
import { tags, text } from './html.ts';

const catalogues: Record<Locale, typeof en> = { en, ru };
const entry = new URL('../dist/server/entry.mjs', import.meta.url).pathname;
const origin = new URL(siteUrl).origin;
const fixture = JSON.parse(readFileSync(new URL('./fixtures/internal-plans.json', import.meta.url), 'utf8')) as {
  plans: { id: string; kind: string; name: string; price: { amount: number; currency: string } | null }[];
};

const pathOf = (locale: string, path: string) =>
  `${locale === defaultLocale ? '' : `/${locale}`}/${path}`.replace(/\/?$/, '/');
const urlOf = (locale: string, path: string) => new URL(pathOf(locale, path), origin).href;

/** What the stub of the internal API answers, set by each test, and what Supabase Auth was asked. */
const stub = { down: false, empty: false, available: [] as string[], asked: [] as string[] };
const auth: { path: string; query: URLSearchParams; body: Record<string, unknown> }[] = [];
/** The sign-in attempts the test server allows per client address in an hour. */
const LIMIT = 3;

let api: Server;
let site: ChildProcess;
let base: string;

async function listen(server: Server): Promise<number> {
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  return (server.address() as AddressInfo).port;
}

before(async () => {
  api = createServer((request, response) => {
    const url = new URL(request.url ?? '/', 'http://stub.invalid');
    if (url.pathname.startsWith('/auth/v1/')) {
      let body = '';
      request.on('data', chunk => (body += chunk));
      request.on('end', () => {
        auth.push({ path: url.pathname, query: url.searchParams, body: body ? JSON.parse(body) : {} });
        // A mail is always sent; a code is never right.
        const [status, answer] =
          url.pathname === '/auth/v1/otp' ? [200, '{}'] : [403, '{"code":"otp_expired","msg":"expired"}'];
        response.writeHead(status, { 'content-type': 'application/json' });
        response.end(answer);
      });
      return;
    }
    stub.asked.push(`${url.pathname}${url.search}`);
    if (url.pathname !== '/internal/plans' || stub.down) {
      response.writeHead(503, { 'content-type': 'application/json' });
      response.end('{"error":"unavailable","message":"service unavailable"}');
      return;
    }
    const plans = stub.empty
      ? []
      : fixture.plans.map(plan => ({ ...plan, available: stub.available.includes(plan.id) }));
    response.writeHead(200, { 'content-type': 'application/json' });
    response.end(JSON.stringify({ ...fixture, locale: url.searchParams.get('locale'), plans }));
  });
  const apiPort = await listen(api);

  const probe = createServer();
  const port = await listen(probe);
  await new Promise(resolve => probe.close(resolve));
  site = spawn(process.execPath, [entry], {
    env: {
      PATH: process.env.PATH,
      HOST: '127.0.0.1',
      PORT: String(port),
      DEVTAB_API_INTERNAL_URL: `http://127.0.0.1:${apiPort}`,
      DEVTAB_SUPABASE_URL: `http://127.0.0.1:${apiPort}`,
      // Not a key: the stub takes anything.
      DEVTAB_SUPABASE_PUBLISHABLE_KEY: 'sb_publishable_stub',
      DEVTAB_RATE_SIGN_IN_PER_IP: String(LIMIT),
    },
    stdio: 'ignore',
  });
  base = `http://127.0.0.1:${port}`;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      await fetch(`${base}/`);
      return;
    } catch {
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  }
  throw new Error('the built server did not start: run pnpm build first');
});

after(() => {
  site?.kill();
  api?.close();
});

async function get(path: string): Promise<{ status: number; headers: Headers; html: string }> {
  const response = await fetch(`${base}${path}`, { redirect: 'manual' });
  return { status: response.status, headers: response.headers, html: await response.text() };
}

test('every server page exists in every locale, with its language, canonical URL, alternates and policy, and no script', async () => {
  for (const [id, page] of Object.entries(serverPages)) {
    for (const locale of locales) {
      const where = `${id} (${locale})`;
      const { status, headers, html } = await get(pathOf(locale, page.path));
      if (page.signIn) {
        // Signed out: to the sign-in page of the same language, and back.
        assert.equal(status, 303, where);
        const next = encodeURIComponent(pathOf(locale, page.path));
        assert.equal(headers.get('location'), `${pathOf(locale, 'sign-in/')}?next=${next}`, where);
        continue;
      }
      assert.equal(status, 200, where);
      assert.equal(tags(html, 'html')[0]?.get('lang'), locale, `${where}: <html lang>`);
      assert.match(html, /<title>[^<]+<\/title>/, `${where}: title`);
      assert.equal((html.match(/<h1[\s>]/g) ?? []).length, 1, `${where}: one h1`);

      const links = tags(html, 'link');
      const canonical = links.filter(link => link.get('rel') === 'canonical').map(link => link.get('href'));
      assert.deepEqual(canonical, [urlOf(locale, page.path)], `${where}: canonical`);
      const alternates = links
        .filter(link => link.get('rel') === 'alternate' && link.has('hreflang'))
        .map(link => [link.get('hreflang'), link.get('href')]);
      assert.deepEqual(
        alternates,
        [...locales.map(other => [other, urlOf(other, page.path)]), ['x-default', urlOf(defaultLocale, page.path)]],
        `${where}: hreflang alternates`,
      );
      const switchLinks = tags(html, 'a').filter(link => link.has('hreflang'));
      assert.deepEqual(
        switchLinks.map(link => [link.get('hreflang'), link.get('href'), link.get('aria-current')]),
        locales.map(other => [other, pathOf(other, page.path), other === locale ? 'page' : undefined]),
        `${where}: language switch`,
      );
      const noindex = tags(html, 'meta').some(meta => meta.get('name') === 'robots');
      assert.equal(noindex, !page.index, `${where}: robots`);
      // The server sends the policy as a header; the static pages carry it in a meta tag.
      const policy = headers.get('content-security-policy') ?? undefined;
      assert.deepEqual(cspProblems(html, policy), [], `${where}: content security policy`);
      if (!page.script) assert.deepEqual(tags(html, 'script'), [], `${where}: no client-side script`);
    }
  }
});

test('/pro/ shows every plan with its price, and "soon" where a plan cannot be bought yet', async () => {
  Object.assign(stub, { down: false, empty: false, available: [], asked: [] });
  for (const locale of locales) {
    const t = catalogues[locale].pages.pro;
    const { status, headers, html } = await get(pathOf(locale, 'pro/'));
    assert.equal(status, 200);
    assert.equal(headers.get('cache-control'), 'public, max-age=300');
    const names = tags(html, 'h2').map(tag => tag.get('id'));
    assert.deepEqual(names, ['plan-free-name', 'plan-pro-monthly-name', 'plan-pro-annual-name'], locale);
    const page = text(html);
    for (const plan of fixture.plans) {
      assert.ok(page.includes(plan.name), `${locale}: ${plan.name}`);
      const price = plan.price && formatPrice(plan.price, locale).replace(/\s+/g, ' ');
      if (price) assert.ok(page.includes(price), `${locale}: the price of ${plan.id}, ${price}`);
    }
    assert.ok(page.includes(t.perMonth) && page.includes(t.perYear), `${locale}: periods`);
    assert.equal(page.split(t.soon).length - 1, 2, `${locale}: both paid plans say soon`);
    assert.ok(page.includes(t.notOpen), `${locale}: purchases not open`);
    assert.ok(!html.includes('/pro/checkout/'), `${locale}: no checkout link`);
    assert.equal(
      tags(html, 'ul')
        .find(list => list.get('role') === 'list')
        ?.get('aria-label'),
      t.listLabel,
    );
  }
  assert.deepEqual(stub.asked, ['/internal/plans?locale=en', '/internal/plans?locale=ru']);
});

test('/pro/ links a plan that can be bought to its checkout', async () => {
  Object.assign(stub, { down: false, empty: false, available: ['pro-monthly'] });
  const { html } = await get('/pro/');
  const choose = tags(html, 'a').filter(link => link.get('href')?.startsWith('/pro/checkout/'));
  assert.deepEqual(
    choose.map(link => link.get('href')),
    ['/pro/checkout/?plan=pro-monthly'],
  );
  const page = text(html);
  assert.ok(page.includes('Choose Monthly'));
  assert.equal(page.split(en.pages.pro.soon).length - 1, 1, 'the other paid plan still says soon');
  assert.ok(!page.includes(en.pages.pro.notOpen));
});

test('/pro/ says so when no plan is offered, and when the API cannot be reached, without being cached', async () => {
  Object.assign(stub, { down: false, empty: true, available: [] });
  const empty = await get('/ru/pro/');
  assert.equal(empty.status, 200);
  assert.ok(text(empty.html).includes(ru.pages.pro.empty));

  Object.assign(stub, { down: true, empty: false });
  const down = await get('/pro/');
  assert.equal(down.status, 503);
  assert.equal(down.headers.get('cache-control'), 'no-store');
  assert.ok(text(down.html).includes(en.pages.pro.unavailable));
  assert.deepEqual(cspProblems(down.html, down.headers.get('content-security-policy') ?? undefined), []);
  stub.down = false;
});

test("a server page under no locale, or under the default locale's own prefix, is not found", async () => {
  for (const path of ['/de/pro/', '/en/pro/', '/ru/ru/pro/']) {
    const { status, html } = await get(path);
    assert.equal(status, 404, path);
    assert.ok(text(html).includes(en.notFound.message), `${path}: the site's 404 page`);
  }
});

/** A form posted from the site's own page, as a browser would, from client address `from`. */
async function post(path: string, fields: Record<string, string>, from: string, cookie = '', origin = base) {
  const response = await fetch(`${base}${path}`, {
    method: 'POST',
    headers: { origin, cookie, 'x-forwarded-for': from, 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams(fields),
    redirect: 'manual',
  });
  return {
    status: response.status,
    location: response.headers.get('location'),
    cookies: response.headers.getSetCookie(),
  };
}

test('the sign-in page is personal: never cached, and /pro/ never sets a cookie', async () => {
  for (const locale of locales) {
    const { headers, html } = await get(pathOf(locale, 'sign-in/'));
    assert.equal(headers.get('cache-control'), 'no-store');
    assert.ok(text(html).includes(catalogues[locale].pages.signIn.intro), locale);
    assert.ok(html.includes('action="/auth/sign-in/"'));
  }
  const response = await fetch(`${base}/pro/`, { headers: { cookie: 'sb-127-auth-token=base64-e30' } });
  assert.deepEqual(response.headers.getSetCookie(), []);
});

test('the e-mail form asks Supabase Auth for a mail with a link back and a code, and keeps the address for the code', async () => {
  auth.length = 0;
  const bad = await post('/auth/sign-in/', { step: 'email', email: 'not-an-address', locale: 'en' }, '192.0.2.1');
  assert.equal(bad.location, '/sign-in/?error=email');

  const sent = await post(
    '/auth/sign-in/',
    { step: 'email', email: 'Person@Example.invalid', locale: 'ru', next: '/ru/pro/' },
    '192.0.2.2',
  );
  assert.deepEqual([sent.status, sent.location], [303, '/ru/sign-in/?next=%2Fru%2Fpro%2F']);
  assert.equal(auth.length, 1);
  const [asked] = auth;
  assert.equal(asked.path, '/auth/v1/otp');
  assert.equal(asked.body.email, 'person@example.invalid');
  assert.deepEqual(asked.body.data, { locale: 'ru' });
  assert.ok(asked.body.code_challenge, 'the link is a PKCE one');
  assert.equal(asked.query.get('redirect_to'), `${base}/auth/callback/?locale=ru&next=%2Fru%2Fpro%2F`);
  const pending = sent.cookies.find(c => c.startsWith('devtab-sign-in='));
  assert.ok(pending && /HttpOnly/i.test(pending) && /SameSite=Lax/i.test(pending), pending);
  const verifier = sent.cookies.find(c => /^sb-[^=]+-auth-token-code-verifier=/.test(c));
  assert.ok(verifier && /HttpOnly/i.test(verifier), 'the PKCE verifier stays in this browser');

  const cookie = [pending, verifier].map(c => c!.split(';')[0]).join('; ');
  const waiting = await fetch(`${base}/ru/sign-in/?next=%2Fru%2Fpro%2F`, { headers: { cookie } });
  const page = text(await waiting.text());
  assert.ok(page.includes(ru.pages.signIn.sent.replace('{email}', 'person@example.invalid')), page);
  assert.ok(page.includes(ru.pages.signIn.codeLabel));

  const letters = await post('/auth/sign-in/', { step: 'code', code: 'abc', locale: 'ru' }, '192.0.2.2', cookie);
  assert.equal(letters.location, '/ru/sign-in/?error=code');
  assert.equal(auth.length, 1, 'a code that is not digits never reaches Supabase Auth');
  const wrong = await post('/auth/sign-in/', { step: 'code', code: '123456', locale: 'ru' }, '192.0.2.3', cookie);
  assert.equal(wrong.location, '/ru/sign-in/?error=code');
  assert.deepEqual(
    [auth[1].path, auth[1].body.email, auth[1].body.token, auth[1].body.type],
    ['/auth/v1/verify', 'person@example.invalid', '123456', 'email'],
  );
  const reset = await post('/auth/sign-in/', { step: 'reset', locale: 'en' }, '192.0.2.3', cookie);
  assert.equal(reset.location, '/sign-in/');
  assert.ok(
    reset.cookies.some(c => /^devtab-sign-in=/.test(c) && /Expires=Thu, 01 Jan 1970/i.test(c)),
    'the address is forgotten',
  );
});

test('the forms keep `next` to this site only, refuse other sites and limit attempts per address', async () => {
  const away = await post(
    '/auth/sign-in/',
    { step: 'email', email: 'a@example.invalid', locale: 'en', next: '//elsewhere.example.invalid/' },
    '192.0.2.10',
  );
  assert.equal(away.location, '/sign-in/');
  const forward = await get('/sign-in/?next=https://elsewhere.example.invalid/');
  assert.equal(forward.status, 200, 'not signed in: the page, and no next');
  assert.ok(!forward.html.includes('elsewhere'));

  const crossSite = await post(
    '/auth/sign-in/',
    { step: 'reset' },
    '192.0.2.11',
    '',
    'https://elsewhere.example.invalid',
  );
  assert.equal(crossSite.status, 403);
  const signOut = await post('/auth/signout/', {}, '192.0.2.11', '', 'https://elsewhere.example.invalid');
  assert.equal(signOut.status, 403);

  const attempt = () =>
    post('/auth/sign-in/', { step: 'email', email: 'b@example.invalid', locale: 'en' }, '192.0.2.12');
  for (let n = 0; n < LIMIT; n += 1) assert.equal((await attempt()).location, '/sign-in/');
  assert.equal((await attempt()).location, '/sign-in/?error=rate');
  const other = await post('/auth/sign-in/', { step: 'email', email: 'b@example.invalid', locale: 'en' }, '192.0.2.13');
  assert.equal(other.location, '/sign-in/', 'another address is counted apart');
});

test('a link back without a code, or with one this browser did not ask for, returns to the sign-in page', async () => {
  for (const [path, location] of [
    ['/auth/callback/?locale=ru', '/ru/sign-in/?error=link'],
    ['/auth/callback/?code=00000000-0000-0000-0000-000000000000&next=%2Fpro%2F', '/sign-in/?next=%2Fpro%2F&error=link'],
  ]) {
    const response = await get(path);
    assert.deepEqual([response.status, response.headers.get('location')], [303, location], path);
  }
});

test('/auth/connect/ answers only the site’s own pages, and only for a signed-in person', async () => {
  const post = (headers: Record<string, string>) =>
    fetch(`${base}/auth/connect/`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', ...headers },
      body: JSON.stringify({ inst: 'AAAAAAAAAAAAAAAAAAAAAA' }),
      redirect: 'manual',
    });
  const foreign = await post({ origin: 'https://elsewhere.example.invalid' });
  assert.equal(foreign.status, 403);
  const signedOut = await post({ origin: base });
  assert.equal(signedOut.status, 401);
  assert.equal(signedOut.headers.get('cache-control'), 'no-store');
});
