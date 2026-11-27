/**
 * The pages the server renders (src/lib/serverPages.ts), on the built server
 * (dist/server/entry.mjs, after `pnpm build`), against one stub (node:http)
 * standing in for the API's internal listener, which answers the plans of a
 * fixture, and for Supabase Auth, which sends every mail, refuses every code
 * but the reset code of `stubAuth`, knows one password, and signs sessions
 * with tokens it checks itself: every page in every locale with its
 * language, canonical URL, alternates, language switch and content security
 * policy, and no script; then what /pro/ shows, and the forms of every
 * sign-in method, with every method on, then with only some (more servers,
 * the same stub). Signing in for real, by the code and the link of a real
 * mail, is script/e2e-sign-in.ts, against the local stack.
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
const auth: { method: string; path: string; query: URLSearchParams; body: Record<string, unknown> }[] = [];
/** The sign-in attempts the test servers allow per client address in an hour. */
const LIMIT = 3;

/** What the stub of Supabase Auth knows: one account with a password, and the code of a reset mail. */
const stubAuth = {
  email: 'person@example.invalid',
  password: 'correct horse',
  unconfirmed: 'waiting@example.invalid',
  resetCode: '654321',
  /** A PKCE code that the token endpoint exchanges (any verifier). */
  goodCode: 'good-code',
  userId: '00000000-0000-4000-8000-000000000001',
  sessionId: '00000000-0000-4000-8000-0000000000aa',
};
const b64 = (value: unknown) => Buffer.from(JSON.stringify(value)).toString('base64url');
/** A token of the stub's session: HS256, so that supabase-js asks the stub (`GET /user`) to check it. */
const stubToken = `${b64({ alg: 'HS256', typ: 'JWT' })}.${b64({
  sub: stubAuth.userId,
  email: stubAuth.email,
  session_id: stubAuth.sessionId,
  aud: 'authenticated',
  role: 'authenticated',
  exp: Math.floor(Date.now() / 1000) + 86_400,
})}.c3R1Yg`;
const stubUser = {
  id: stubAuth.userId,
  aud: 'authenticated',
  role: 'authenticated',
  email: stubAuth.email,
  app_metadata: {},
  user_metadata: {},
  created_at: '2026-11-01T00:00:00Z',
};
const stubSession = () => ({
  access_token: stubToken,
  token_type: 'bearer',
  expires_in: 86_400,
  expires_at: Math.floor(Date.now() / 1000) + 86_400,
  refresh_token: 'stub-refresh',
  user: stubUser,
});

/** Supabase Auth's answer to one request of the stub. */
function authAnswer(
  method: string,
  path: string,
  query: URLSearchParams,
  body: Record<string, unknown>,
  bearer: string,
) {
  const refuse = (status: number, code: string) => [status, { code, msg: code }] as const;
  switch (`${method} ${path}`) {
    case 'POST /auth/v1/otp':
    case 'POST /auth/v1/recover':
      return [200, {}] as const;
    case 'POST /auth/v1/verify':
      return body.type === 'recovery' && body.token === stubAuth.resetCode && body.email === stubAuth.email
        ? ([200, stubSession()] as const)
        : refuse(403, 'otp_expired');
    case 'POST /auth/v1/token':
      if (query.get('grant_type') === 'password') {
        if (body.email === stubAuth.unconfirmed) return refuse(400, 'email_not_confirmed');
        return body.email === stubAuth.email && body.password === stubAuth.password
          ? ([200, stubSession()] as const)
          : refuse(400, 'invalid_credentials');
      }
      return body.auth_code === stubAuth.goodCode && body.code_verifier
        ? ([200, stubSession()] as const)
        : refuse(403, 'flow_state_not_found');
    case 'POST /auth/v1/signup':
      // A project that confirms addresses: the new user, and no session until the mail's link or code.
      return body.password === 'short'
        ? refuse(422, 'weak_password')
        : ([200, { ...stubUser, email: body.email }] as const);
    case 'GET /auth/v1/user':
      return bearer === stubToken ? ([200, stubUser] as const) : refuse(401, 'bad_jwt');
    case 'PUT /auth/v1/user':
      if (bearer !== stubToken) return refuse(401, 'bad_jwt');
      return body.password === stubAuth.password ? refuse(422, 'same_password') : ([200, stubUser] as const);
    case 'POST /auth/v1/logout':
      return [204, null] as const;
    default:
      return refuse(404, 'not_found');
  }
}

let api: Server;
let apiBase: string;
const sites: ChildProcess[] = [];
let base: string;

async function listen(server: Server): Promise<number> {
  await new Promise<void>(resolve => server.listen(0, '127.0.0.1', resolve));
  return (server.address() as AddressInfo).port;
}

/** Starts the built server against the stub, with `methods` as DEVTAB_SIGN_IN_METHODS (none: unset); its address. */
async function startSite(methods?: string): Promise<string> {
  const probe = createServer();
  const port = await listen(probe);
  await new Promise(resolve => probe.close(resolve));
  const site = spawn(process.execPath, [entry], {
    env: {
      PATH: process.env.PATH,
      HOST: '127.0.0.1',
      PORT: String(port),
      DEVTAB_API_INTERNAL_URL: apiBase,
      DEVTAB_SUPABASE_URL: apiBase,
      // Not a key: the stub takes anything.
      DEVTAB_SUPABASE_PUBLISHABLE_KEY: 'sb_publishable_stub',
      DEVTAB_RATE_SIGN_IN_PER_IP: String(LIMIT),
      ...(methods === undefined ? {} : { DEVTAB_SIGN_IN_METHODS: methods }),
    },
    stdio: 'ignore',
  });
  sites.push(site);
  const address = `http://127.0.0.1:${port}`;
  for (let attempt = 0; attempt < 100; attempt += 1) {
    try {
      await fetch(`${address}/`);
      return address;
    } catch {
      await new Promise(resolve => setTimeout(resolve, 100));
    }
  }
  throw new Error('the built server did not start: run pnpm build first');
}

before(async () => {
  api = createServer((request, response) => {
    const url = new URL(request.url ?? '/', 'http://stub.invalid');
    if (url.pathname.startsWith('/auth/v1/')) {
      let body = '';
      request.on('data', chunk => (body += chunk));
      request.on('end', () => {
        const method = request.method ?? 'GET';
        const parsed = body ? (JSON.parse(body) as Record<string, unknown>) : {};
        auth.push({ method, path: url.pathname, query: url.searchParams, body: parsed });
        const bearer = request.headers.authorization?.replace(/^Bearer /, '') ?? '';
        const [status, answer] = authAnswer(method, url.pathname, url.searchParams, parsed, bearer);
        // Errors as Supabase Auth words them since this API version: `{"code": "<error code>", "msg": …}`.
        const headers = { 'x-supabase-api-version': '2024-01-01' };
        response.writeHead(status, answer === null ? headers : { ...headers, 'content-type': 'application/json' });
        response.end(answer === null ? undefined : JSON.stringify(answer));
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
  apiBase = `http://127.0.0.1:${await listen(api)}`;
  // Every method on: every page of signing in exists.
  base = await startSite('email,password,google');
});

after(() => {
  for (const site of sites) site.kill();
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
      // Every page runs the theme's script in <head>; a page with `script` one more of its own.
      const scripts = tags(html, 'script').filter(script => !script.has('data-theme-script'));
      if (!page.script) assert.deepEqual(scripts, [], `${where}: no client-side script but the theme's`);
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
    const names = tags(html, 'h3').map(tag => tag.get('id'));
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

/** The cookies a browser keeps from a series of answers, as one `Cookie` header. */
class Jar {
  readonly cookies = new Map<string, string>();
  take(setCookies: string[]): this {
    for (const line of setCookies) {
      const [pair = '', ...attributes] = line.split(';');
      const name = pair.slice(0, pair.indexOf('='));
      const value = pair.slice(pair.indexOf('=') + 1);
      const gone = attributes.some(a => /^\s*(max-age=0|expires=thu, 01 jan 1970)/i.test(a));
      if (gone || value === '') this.cookies.delete(name);
      else this.cookies.set(name, value);
    }
    return this;
  }
  has(name: string | RegExp): boolean {
    return [...this.cookies.keys()].some(key => (typeof name === 'string' ? key === name : name.test(key)));
  }
  get header(): string {
    return [...this.cookies].map(([name, value]) => `${name}=${value}`).join('; ');
  }
}

async function page(path: string, cookie = '', at = base) {
  const response = await fetch(`${at}${path}`, { headers: { cookie }, redirect: 'manual' });
  const html = await response.text();
  return { status: response.status, html, text: text(html), location: response.headers.get('location') };
}

const hrefs = (html: string) => tags(html, 'a').map(link => link.get('href') ?? '');
const actions = (html: string) => tags(html, 'form').map(form => form.get('action') ?? '');

test('with every method on, the sign-in page offers Google, a password and the e-mail, and links the other pages', async () => {
  for (const locale of locales) {
    const t = catalogues[locale].pages.signIn;
    const next = pathOf(locale, 'pro/');
    const { html, text: words } = await page(`${pathOf(locale, 'sign-in/')}?next=${encodeURIComponent(next)}`);
    const query = new URLSearchParams({ locale, next });
    assert.ok(hrefs(html).includes(`/auth/google/?${query}`), `${locale}: the Google link`);
    assert.deepEqual(actions(html), ['/auth/password/', '/auth/sign-in/'], locale);
    assert.ok(hrefs(html).includes(`${pathOf(locale, 'forgot-password/')}?next=${encodeURIComponent(next)}`));
    assert.ok(hrefs(html).includes(`${pathOf(locale, 'sign-up/')}?next=${encodeURIComponent(next)}`));
    for (const phrase of [t.google, t.passwordLabel, t.forgot, t.intro, t.noAccount]) {
      assert.ok(words.includes(phrase), `${locale}: ${phrase}`);
    }
    // The page's own error words, and none it does not know.
    const wrong = await page(`${pathOf(locale, 'sign-in/')}?error=credentials`);
    assert.ok(wrong.text.includes(t.errorCredentials), locale);
    assert.ok(!(await page(`${pathOf(locale, 'sign-in/')}?error=%3Cb%3E`)).html.includes('role="alert"'));
  }
});

test('a password signs in, and a wrong or unconfirmed one says so without a session', async () => {
  auth.length = 0;
  const wrong = await post(
    '/auth/password/',
    { step: 'sign-in', email: stubAuth.email, password: 'wrong', locale: 'en' },
    '198.51.100.1',
  );
  assert.deepEqual([wrong.status, wrong.location], [303, '/sign-in/?error=credentials']);
  assert.ok(!wrong.cookies.some(c => c.startsWith('sb-')), 'no session');
  const [asked] = auth;
  assert.deepEqual(
    [asked.method, asked.path, asked.query.get('grant_type'), asked.body.email, asked.body.password],
    ['POST', '/auth/v1/token', 'password', stubAuth.email, 'wrong'],
  );
  const waiting = await post(
    '/auth/password/',
    { step: 'sign-in', email: stubAuth.unconfirmed, password: 'anything', locale: 'ru' },
    '198.51.100.1',
  );
  assert.equal(waiting.location, '/ru/sign-in/?error=unconfirmed');
  const bad = await post(
    '/auth/password/',
    { step: 'sign-in', email: 'nope', password: 'x', locale: 'en' },
    '198.51.100.2',
  );
  assert.equal(bad.location, '/sign-in/?error=email');

  const right = await post(
    '/auth/password/',
    { step: 'sign-in', email: 'Person@Example.invalid', password: stubAuth.password, locale: 'ru', next: '/ru/pro/' },
    '198.51.100.2',
  );
  assert.deepEqual([right.status, right.location], [303, '/ru/pro/']);
  const jar = new Jar().take(right.cookies);
  assert.ok(jar.has(/^sb-127-auth-token(\.\d+)?$/), 'the session');
  assert.ok(right.cookies.filter(c => c.startsWith('sb-')).every(c => /HttpOnly/i.test(c)));
  const signedIn = await page('/ru/sign-in/', jar.header);
  assert.ok(signedIn.text.includes(ru.pages.signIn.signedIn.replace('{email}', stubAuth.email)), signedIn.text);
  const out = await post('/auth/signout/', { locale: 'ru' }, '198.51.100.2', jar.header);
  assert.equal(out.location, '/ru/sign-in/');
  assert.ok(new Jar().take(right.cookies).take(out.cookies).cookies.size === 0, 'signed out: no cookie left');
});

test('signing up checks the passwords, asks Supabase Auth to confirm the address, and waits for the code', async () => {
  auth.length = 0;
  const fields = { step: 'sign-up', email: 'New@Example.invalid', locale: 'ru', next: '/ru/pro/' };
  const mismatch = await post(
    '/auth/password/',
    { ...fields, password: 'one two', confirm: 'two one' },
    '198.51.100.3',
  );
  assert.equal(mismatch.location, '/ru/sign-up/?next=%2Fru%2Fpro%2F&error=password-mismatch');
  const long = 'x'.repeat(73);
  const tooLong = await post('/auth/password/', { ...fields, password: long, confirm: long }, '198.51.100.3');
  assert.equal(tooLong.location, '/ru/sign-up/?next=%2Fru%2Fpro%2F&error=password-long');
  assert.equal(auth.length, 0, 'never reaches Supabase Auth');
  const weak = await post('/auth/password/', { ...fields, password: 'short', confirm: 'short' }, '198.51.100.3');
  assert.equal(weak.location, '/ru/sign-up/?next=%2Fru%2Fpro%2F&error=password-weak');
  const wording = await page(`/ru/sign-up/?error=password-weak`);
  assert.ok(wording.text.includes(ru.pages.signIn.errorPasswordWeak));

  const made = await post(
    '/auth/password/',
    { ...fields, password: 'a long one', confirm: 'a long one' },
    '198.51.100.4',
  );
  assert.deepEqual([made.status, made.location], [303, '/ru/sign-in/?next=%2Fru%2Fpro%2F']);
  const asked = auth.at(-1)!;
  assert.equal(asked.path, '/auth/v1/signup');
  assert.deepEqual(
    [asked.body.email, asked.body.password, asked.body.data],
    ['new@example.invalid', 'a long one', { locale: 'ru' }],
  );
  assert.ok(asked.body.code_challenge, "the mail's link is a PKCE one");
  assert.equal(asked.query.get('redirect_to'), `${base}/auth/callback/?locale=ru&next=%2Fru%2Fpro%2F`);
  const jar = new Jar().take(made.cookies);
  assert.ok(
    jar.has('devtab-sign-in') && jar.has(/code-verifier$/),
    'the address waits for the code, the verifier for the link',
  );
  const waiting = await page('/ru/sign-in/?next=%2Fru%2Fpro%2F', jar.header);
  assert.ok(waiting.text.includes(ru.pages.signIn.sent.replace('{email}', 'new@example.invalid')), waiting.text);
  assert.ok(waiting.text.includes(ru.pages.signIn.codeLabel));
});

test('a forgotten password: the mail, its code, the new password, only in the session the reset started', async () => {
  auth.length = 0;
  const bad = await post('/auth/password/', { step: 'forgot', email: 'nope', locale: 'en' }, '198.51.100.5');
  assert.equal(bad.location, '/forgot-password/?error=email');
  const sent = await post('/auth/password/', { step: 'forgot', email: stubAuth.email, locale: 'en' }, '198.51.100.5');
  assert.deepEqual([sent.status, sent.location], [303, '/forgot-password/']);
  const asked = auth.at(-1)!;
  assert.deepEqual([asked.path, asked.body.email], ['/auth/v1/recover', stubAuth.email]);
  assert.ok(asked.body.code_challenge);
  assert.equal(asked.query.get('redirect_to'), `${base}/auth/callback/?locale=en&via=recovery`);
  const jar = new Jar().take(sent.cookies);
  assert.ok(jar.has('devtab-reset-address'));
  const waiting = await page('/forgot-password/', jar.header);
  assert.ok(waiting.text.includes(en.pages.forgotPassword.sent.replace('{email}', stubAuth.email)));
  assert.deepEqual(actions(waiting.html), ['/auth/password/', '/auth/password/']);

  const wrong = await post(
    '/auth/password/',
    { step: 'code', code: '111111', locale: 'en' },
    '198.51.100.5',
    jar.header,
  );
  assert.equal(wrong.location, '/forgot-password/?error=code');
  const right = await post(
    '/auth/password/',
    { step: 'code', code: stubAuth.resetCode, locale: 'en' },
    '198.51.100.6',
    jar.header,
  );
  assert.deepEqual([right.status, right.location], [303, '/reset-password/']);
  jar.take(right.cookies);
  assert.ok(!jar.has('devtab-reset-address'), 'the address is forgotten');
  assert.equal(jar.cookies.get('devtab-reset'), stubAuth.sessionId, 'the reset is bound to its session');
  assert.ok(jar.has(/^sb-127-auth-token(\.\d+)?$/));

  const form = await page('/reset-password/', jar.header);
  assert.ok(form.text.includes(en.pages.resetPassword.intro.replace('{email}', stubAuth.email)), form.text);
  assert.deepEqual(actions(form.html), ['/auth/password/']);
  // The same session without the mark: no form.
  const session = [...jar.cookies]
    .filter(([name]) => name.startsWith('sb-'))
    .map(([n, v]) => `${n}=${v}`)
    .join('; ');
  const unmarked = await page('/reset-password/', session);
  assert.ok(unmarked.text.includes(en.pages.resetPassword.expired));
  assert.deepEqual(actions(unmarked.html), []);
  const refused = await post(
    '/auth/password/',
    { step: 'update', password: 'p1', confirm: 'p1', locale: 'en' },
    '198.51.100.7',
    session,
  );
  assert.equal(refused.location, '/reset-password/');

  const update = (password: string, confirm = password) =>
    post('/auth/password/', { step: 'update', password, confirm, locale: 'en' }, '198.51.100.8', jar.header);
  assert.equal((await update('new one', 'other')).location, '/reset-password/?error=password-mismatch');
  assert.equal((await update(stubAuth.password)).location, '/reset-password/?error=password-same');
  const saved = await update('a new password');
  assert.deepEqual([saved.status, saved.location], [303, '/sign-in/?notice=password']);
  assert.deepEqual(
    [auth.at(-1)!.method, auth.at(-1)!.path, auth.at(-1)!.body.password],
    ['PUT', '/auth/v1/user', 'a new password'],
  );
  jar.take(saved.cookies);
  assert.ok(!jar.has('devtab-reset'), 'the mark is gone');
  const notice = await page('/sign-in/?notice=password', jar.header);
  assert.ok(notice.text.includes(en.pages.signIn.passwordChanged));
});

test("a reset mail's link comes back through the callback to the new password, and fails to the reset page", async () => {
  const sent = await post(
    '/auth/password/',
    { step: 'forgot', email: stubAuth.email, locale: 'ru', next: '/ru/pro/' },
    '198.51.100.9',
  );
  const jar = new Jar().take(sent.cookies);
  const back = await page(
    `/auth/callback/?code=${stubAuth.goodCode}&locale=ru&next=%2Fru%2Fpro%2F&via=recovery`,
    jar.header,
  );
  assert.deepEqual([back.status, back.location], [303, '/ru/reset-password/?next=%2Fru%2Fpro%2F']);
  const response = await fetch(`${base}/auth/callback/?code=${stubAuth.goodCode}&locale=ru&via=recovery`, {
    headers: { cookie: jar.header },
    redirect: 'manual',
  });
  jar.take(response.headers.getSetCookie());
  assert.equal(jar.cookies.get('devtab-reset'), stubAuth.sessionId);

  const elsewhere = await page(`/auth/callback/?code=${stubAuth.goodCode}&locale=ru&via=recovery`);
  assert.equal(elsewhere.location, '/ru/forgot-password/?error=link', 'no verifier in another browser');
  const google = await page('/auth/callback/?error=access_denied&locale=en&next=%2Fpro%2F&via=google');
  assert.equal(google.location, '/sign-in/?next=%2Fpro%2F&error=google');
});

test('Google starts at Supabase Auth with a PKCE verifier in this browser, only from the site itself', async () => {
  const start = (query: string, site?: string, from = '198.51.100.10') =>
    fetch(`${base}/auth/google/?${query}`, {
      headers: { 'x-forwarded-for': from, ...(site ? { 'sec-fetch-site': site } : {}) },
      redirect: 'manual',
    });
  const response = await start('locale=ru&next=%2Fru%2Fpro%2F', 'same-origin');
  assert.equal(response.status, 303);
  const location = new URL(response.headers.get('location') ?? '');
  assert.equal(`${location.origin}${location.pathname}`, `${apiBase}/auth/v1/authorize`);
  assert.equal(location.searchParams.get('provider'), 'google');
  assert.equal(
    location.searchParams.get('redirect_to'),
    `${base}/auth/callback/?locale=ru&next=%2Fru%2Fpro%2F&via=google`,
  );
  assert.ok(location.searchParams.get('code_challenge'));
  assert.equal(location.searchParams.get('skip_http_redirect'), null);
  const verifier = response.headers.getSetCookie().find(c => /^sb-[^=]+-auth-token-code-verifier=/.test(c));
  assert.ok(verifier && /HttpOnly/i.test(verifier), 'the verifier stays in this browser');

  const away = await start('locale=en&next=%2F%2Felsewhere.example.invalid%2F');
  assert.ok(!new URL(away.headers.get('location') ?? '').searchParams.get('redirect_to')?.includes('elsewhere'));
  for (const site of ['cross-site', 'same-site']) {
    const foreign = await start('locale=ru&next=%2Fru%2Fpro%2F', site);
    assert.deepEqual(
      [foreign.status, foreign.headers.get('location')],
      [303, '/ru/sign-in/?next=%2Fru%2Fpro%2F'],
      site,
    );
    assert.deepEqual(foreign.headers.getSetCookie(), [], `${site}: nothing started`);
  }
  for (let n = 0; n < LIMIT; n += 1) await start('locale=en', 'none', '198.51.100.11');
  assert.equal((await start('locale=en', 'none', '198.51.100.11')).headers.get('location'), '/sign-in/?error=rate');
});

test('the password forms refuse other sites and count attempts with the other forms', async () => {
  const crossSite = await post(
    '/auth/password/',
    { step: 'sign-in', email: stubAuth.email, password: 'x' },
    '198.51.100.12',
    '',
    'https://elsewhere.example.invalid',
  );
  assert.equal(crossSite.status, 403);
  const from = '198.51.100.13';
  await post('/auth/sign-in/', { step: 'email', email: 'c@example.invalid', locale: 'en' }, from);
  for (let n = 1; n < LIMIT; n += 1) {
    await post('/auth/password/', { step: 'forgot', email: 'c@example.invalid', locale: 'en' }, from);
  }
  const over = await post(
    '/auth/password/',
    { step: 'sign-in', email: 'c@example.invalid', password: 'x', locale: 'en' },
    from,
  );
  assert.equal(over.location, '/sign-in/?error=rate');
});

test('a method that is off is not offered, and its pages and endpoints are not found', async () => {
  // DEVTAB_SIGN_IN_METHODS unset: the e-mail alone.
  const emailOnly = await startSite();
  const signIn = await page('/sign-in/', '', emailOnly);
  assert.deepEqual(actions(signIn.html), ['/auth/sign-in/']);
  assert.ok(!hrefs(signIn.html).some(href => /google|sign-up|forgot/.test(href)), 'no other method');
  for (const path of ['/sign-up/', '/ru/forgot-password/', '/reset-password/']) {
    const { status, text: words } = await page(path, '', emailOnly);
    assert.equal(status, 404, path);
    assert.ok(words.includes(en.notFound.message), `${path}: the site's 404 page`);
  }
  const google = await fetch(`${emailOnly}/auth/google/?locale=en`, { redirect: 'manual' });
  assert.equal(google.status, 404);
  const password = await fetch(`${emailOnly}/auth/password/`, {
    method: 'POST',
    headers: { origin: emailOnly, 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams({ step: 'sign-in', email: stubAuth.email, password: stubAuth.password }),
    redirect: 'manual',
  });
  assert.equal(password.status, 404);

  // A password alone: no e-mail form, but the code of a sign-up's confirmation still works.
  const passwordOnly = await startSite('password');
  const page_ = await page('/sign-in/', '', passwordOnly);
  assert.deepEqual(actions(page_.html), ['/auth/password/']);
  assert.ok(!page_.text.includes(en.pages.signIn.intro));
  const form = (fields: Record<string, string>) =>
    fetch(`${passwordOnly}/auth/sign-in/`, {
      method: 'POST',
      headers: {
        origin: passwordOnly,
        cookie: 'devtab-sign-in=new%40example.invalid',
        'content-type': 'application/x-www-form-urlencoded',
      },
      body: new URLSearchParams(fields),
      redirect: 'manual',
    });
  assert.equal((await form({ step: 'email', email: 'd@example.invalid' })).status, 404);
  const code = await form({ step: 'code', code: '123456' });
  assert.deepEqual([code.status, code.headers.get('location')], [303, '/sign-in/?error=code']);

  // Nothing known: no way to sign in, and the page says so.
  const none = await startSite('carrier-pigeon');
  const nothing = await page('/sign-in/', '', none);
  assert.deepEqual(actions(nothing.html), []);
  assert.ok(nothing.text.includes(en.pages.signIn.unavailable));
});
