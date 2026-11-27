/**
 * Sign-in end to end, against the local stack, without any real sign-in
 * provider: the site's forms, the mail in the local mail catcher, the code
 * and the link from it, the session's cookies, and the API checking the
 * session's access token against the stack's JWKS; then, when the site
 * offers passwords (script/site-server does by default), signing up with a
 * password, confirming the address by the mail's code, signing in with it,
 * and resetting it by the code and by the link of a reset mail.
 *
 *   script/auth-up && script/api-dev     # the full local stack, then the API
 *   pnpm build && script/site-server     # the built site, with sign-in
 *   node script/e2e-sign-in.ts
 *
 * It starts nothing and stops nothing: script/auth-up starts the stack's
 * containers (Postgres, Auth, the gateway, the mail catcher); stop them
 * afterwards with `supabase --workdir <repository> stop`. The addresses:
 * SITE (http://127.0.0.1:4321), API (the internal listener,
 * http://127.0.0.1:8081), MAILPIT (http://127.0.0.1:54334). It signs in two
 * new addresses under example.invalid and leaves their accounts in the local
 * database (three with passwords). Tokens, codes and cookies are printed
 * masked.
 */
import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';

const site = process.env.SITE ?? 'http://127.0.0.1:4321';
const api = process.env.API ?? 'http://127.0.0.1:8081';
const mailpit = process.env.MAILPIT ?? 'http://127.0.0.1:54334';

/** Tokens and ids: their first 8 characters; a code: its first 2. */
const mask = (value: string) => `${value.slice(0, value.length > 12 ? 8 : 2)}…`;
const step = (name: string, result: string) => console.log(`ok  ${name}: ${result}`);

/** A browser's cookies for the site: what it set, sent back. */
class Jar {
  readonly cookies = new Map<string, string>();
  take(response: Response): void {
    for (const line of response.headers.getSetCookie()) {
      const [pair, ...attributes] = line.split(';');
      const name = pair.slice(0, pair.indexOf('='));
      const value = pair.slice(pair.indexOf('=') + 1);
      const expired = attributes.some(a => /max-age=0\b/i.test(a.trim()) || /expires=thu, 01 jan 1970/i.test(a.trim()));
      if (expired || value === '') this.cookies.delete(name);
      else this.cookies.set(name, value);
    }
  }
  header(): string {
    return [...this.cookies].map(([name, value]) => `${name}=${value}`).join('; ');
  }
}

async function page(jar: Jar, path: string): Promise<{ status: number; text: string; location: string | null }> {
  const response = await fetch(new URL(path, site), { headers: { cookie: jar.header() }, redirect: 'manual' });
  jar.take(response);
  const html = await response.text();
  const text = html
    .replace(/<[^>]+>/g, ' ')
    .replace(/&#39;|&rsquo;|’/g, "'")
    .replace(/\s+/g, ' ');
  return { status: response.status, text, location: response.headers.get('location') };
}

async function post(jar: Jar, path: string, fields: Record<string, string>): Promise<Response> {
  const response = await fetch(new URL(path, site), {
    method: 'POST',
    headers: { cookie: jar.header(), origin: site, 'content-type': 'application/x-www-form-urlencoded' },
    body: new URLSearchParams(fields),
    redirect: 'manual',
  });
  jar.take(response);
  return response;
}

/** The newest mail to `address` in the mail catcher, once it has more than `seen` of them: its HTML. */
async function mailTo(address: string, seen = 0): Promise<string> {
  for (let attempt = 0; attempt < 50; attempt += 1) {
    const search = await fetch(`${mailpit}/api/v1/search?query=${encodeURIComponent(`to:"${address}"`)}`);
    const { messages } = (await search.json()) as { messages: { ID: string }[] };
    if (messages.length > seen) {
      const message = (await (await fetch(`${mailpit}/api/v1/message/${messages[0].ID}`)).json()) as {
        HTML: string;
        Subject: string;
      };
      step('mail arrived', `subject "${message.Subject}"`);
      return message.HTML;
    }
    await new Promise(resolve => setTimeout(resolve, 200));
  }
  throw new Error(`no mail to ${address} in the mail catcher`);
}

/** The access token in the session's cookies (`sb-<ref>-auth-token`, maybe in chunks, `base64-` JSON). */
function accessToken(jar: Jar): string {
  const chunks = [...jar.cookies]
    .filter(([name]) => /^sb-.+-auth-token(\.\d+)?$/.test(name))
    .sort(([a], [b]) => a.localeCompare(b, 'en', { numeric: true }))
    .map(([, value]) => decodeURIComponent(value));
  assert.ok(chunks.length > 0, 'no session cookie');
  const joined = chunks.join('');
  const json = joined.startsWith('base64-') ? Buffer.from(joined.slice(7), 'base64url').toString('utf8') : joined;
  return (JSON.parse(json) as { access_token: string }).access_token;
}

async function me(token: string | null): Promise<{ status: number; body: { user?: string; email?: string } }> {
  const response = await fetch(`${api}/internal/me`, {
    headers: token ? { authorization: `Bearer ${token}` } : {},
  });
  return { status: response.status, body: (await response.json()) as { user?: string; email?: string } };
}

const address = (what: string) => `e2e-${what}-${randomBytes(4).toString('hex')}@example.invalid`;

// 1. The page, with nobody signed in: the form, no script.
{
  const jar = new Jar();
  const response = await fetch(new URL('/sign-in/', site));
  const html = await response.text();
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('cache-control'), 'no-store');
  assert.match(html, /<form method="post" action="\/auth\/sign-in\/"/);
  const scripts = (html.match(/<script\b[^>]*>/g) ?? []).filter(tag => !tag.includes('data-theme-script'));
  assert.deepEqual(scripts, [], "no script but the theme's");
  jar.take(response);
  step('GET /sign-in/', "200, the e-mail form, no script but the theme's, Cache-Control: no-store");
}

// 2. Sign in by the code from the mail.
const coder = address('code');
const jar = new Jar();
{
  const sent = await post(jar, '/auth/sign-in/', { step: 'email', email: coder, locale: 'en' });
  assert.equal(sent.status, 303);
  assert.equal(sent.headers.get('location'), '/sign-in/');
  step('POST /auth/sign-in/ (e-mail)', `303 to /sign-in/, address ${coder}`);
  const waiting = await page(jar, '/sign-in/');
  assert.ok(waiting.text.includes(`We sent a sign-in link and a code to ${coder}.`), waiting.text);
  step('GET /sign-in/', 'the code form for that address');

  const html = await mailTo(coder);
  const code = html.match(/<strong>(\d{6,10})<\/strong>/)?.[1];
  assert.ok(code, 'a code in the mail');
  assert.match(html, /href="[^"]*\/auth\/v1\/verify\?token=/, 'a link in the mail');
  step('the mail', `a link and the code ${mask(code)}`);

  const wrong = await post(jar, '/auth/sign-in/', {
    step: 'code',
    code: code === '000000' ? '111111' : '000000',
    locale: 'en',
  });
  assert.equal(wrong.headers.get('location'), '/sign-in/?error=code');
  step('a wrong code', '303 to /sign-in/?error=code');

  const verified = await post(jar, '/auth/sign-in/', { step: 'code', code, locale: 'en' });
  assert.equal(verified.status, 303);
  assert.equal(verified.headers.get('location'), '/sign-in/');
  const signedIn = await page(jar, '/sign-in/');
  assert.ok(signedIn.text.includes(`You're signed in as ${coder}.`), signedIn.text);
  step('POST /auth/sign-in/ (code)', `303, then "You're signed in as ${coder}."`);
  const forward = await page(jar, '/sign-in/?next=/pro/');
  assert.deepEqual([forward.status, forward.location], [303, '/pro/']);
  step('GET /sign-in/?next=/pro/ signed in', '303 to /pro/');
}

// 3. The API trusts the session's access token, checked against the JWKS.
{
  const token = accessToken(jar);
  const header = JSON.parse(Buffer.from(token.split('.')[0], 'base64url').toString('utf8')) as { alg: string };
  assert.equal(header.alg, 'ES256');
  const answer = await me(token);
  assert.equal(answer.status, 200);
  assert.equal(answer.body.email, coder);
  step(
    'GET /internal/me with the access token',
    `200, user ${mask(answer.body.user ?? '')}, the account's address (token ${mask(token)}, ES256)`,
  );
  assert.equal((await me(null)).status, 401);
  const forged = `${token.slice(0, -4)}${token.endsWith('AAAA') ? 'BBBB' : 'AAAA'}`;
  assert.equal((await me(forged)).status, 401);
  step('GET /internal/me without a token, with a forged one', '401, 401');
}

// 4. Sign out.
{
  const out = await post(jar, '/auth/signout/', { locale: 'en' });
  assert.deepEqual([out.status, out.headers.get('location')], [303, '/sign-in/']);
  const after = await page(jar, '/sign-in/');
  assert.ok(after.text.includes('Send the e-mail'), after.text);
  step('POST /auth/signout/', '303, the e-mail form again');
}

// 5. Sign in by the link from the mail, in the same browser, in Russian.
{
  const linker = address('link');
  const browser = new Jar();
  const sent = await post(browser, '/auth/sign-in/', { step: 'email', email: linker, locale: 'ru', next: '/ru/pro/' });
  assert.equal(sent.headers.get('location'), '/ru/sign-in/?next=%2Fru%2Fpro%2F');
  const html = await mailTo(linker);
  assert.match(html, /<html lang="ru">/);
  const link = html.match(/href="([^"]*\/auth\/v1\/verify\?[^"]*)"/)?.[1]?.replace(/&amp;/g, '&');
  assert.ok(link, 'a link in the mail');
  const verify = await fetch(link, { redirect: 'manual' });
  const callback = verify.headers.get('location') ?? '';
  assert.match(callback, /\/auth\/callback\/\?.*code=/, callback);
  step(
    'the link',
    `Supabase Auth sends the browser to /auth/callback/ with a code (${mask(new URL(callback).searchParams.get('code') ?? '')})`,
  );
  const back = await page(browser, new URL(callback).pathname + new URL(callback).search);
  assert.deepEqual([back.status, back.location], [303, '/ru/pro/']);
  const signedIn = await page(browser, '/ru/sign-in/');
  assert.ok(signedIn.text.includes(`Вы вошли как ${linker}.`), signedIn.text);
  step('GET /auth/callback/', `303 to /ru/pro/, then "Вы вошли как ${linker}."`);

  // The same link in another browser: no verifier cookie, so no session.
  const other = new Jar();
  const replay = await page(other, new URL(callback).pathname + new URL(callback).search);
  assert.equal(replay.location, '/ru/sign-in/?next=%2Fru%2Fpro%2F&error=link');
  step('the link in another browser', '303 to /ru/sign-in/?…&error=link');
}

// 6. Passwords, when the site offers them: sign up, confirm by the mail's code, sign in.
const offersPasswords = (await (await fetch(new URL('/sign-in/', site))).text()).includes('action="/auth/password/"');
if (!offersPasswords) {
  step('passwords', 'not offered by this site (DEVTAB_SIGN_IN_METHODS): skipped');
} else {
  const holder = address('password');
  const first = `first ${randomBytes(6).toString('hex')}`;
  const second = `second ${randomBytes(6).toString('hex')}`;
  const browser = new Jar();
  const signedInAs = async () => (await page(browser, '/sign-in/')).text.includes(`You're signed in as ${holder}.`);
  const signOut = async () => {
    await post(browser, '/auth/signout/', { locale: 'en' });
    assert.ok(!(await signedInAs()));
  };
  const signIn = (password: string) =>
    post(browser, '/auth/password/', { step: 'sign-in', email: holder, password, locale: 'en' });

  const made = await post(browser, '/auth/password/', {
    step: 'sign-up',
    email: holder,
    password: first,
    confirm: first,
    locale: 'en',
  });
  assert.deepEqual([made.status, made.headers.get('location')], [303, '/sign-in/']);
  assert.equal((await signIn(first)).headers.get('location'), '/sign-in/?error=unconfirmed');
  step('POST /auth/password/ (sign-up)', `303 to /sign-in/, address ${holder}; unconfirmed, the password waits`);
  const confirmation = await mailTo(holder);
  const code = confirmation.match(/<strong>(\d{6,10})<\/strong>/)?.[1];
  assert.ok(code, 'a code in the confirmation mail');
  const confirmed = await post(browser, '/auth/sign-in/', { step: 'code', code, locale: 'en' });
  assert.equal(confirmed.headers.get('location'), '/sign-in/');
  assert.ok(await signedInAs());
  step('the confirmation mail', `its code ${mask(code)} confirms the address and signs in`);
  await signOut();

  assert.equal((await signIn('not the password')).headers.get('location'), '/sign-in/?error=credentials');
  assert.equal((await signIn(first)).headers.get('location'), '/sign-in/');
  assert.ok(await signedInAs());
  step('POST /auth/password/ (sign-in)', 'a wrong password is refused, the right one signs in');
  await signOut();

  // 7. A forgotten password, by the reset mail's code, in another browser than the one that asked.
  const asker = new Jar();
  const forgot = await post(asker, '/auth/password/', { step: 'forgot', email: holder, locale: 'en' });
  assert.equal(forgot.headers.get('location'), '/forgot-password/');
  const reset = await mailTo(holder, 1);
  assert.match(reset, /A new password for DevTab\.Dev/);
  const resetCode = reset.match(/<strong>(\d{6,10})<\/strong>/)?.[1];
  assert.ok(resetCode, 'a code in the reset mail');
  const verified = await post(asker, '/auth/password/', { step: 'code', code: resetCode, locale: 'en' });
  assert.equal(verified.headers.get('location'), '/reset-password/');
  const form = await page(asker, '/reset-password/');
  assert.ok(form.text.includes(`Choose a new password for ${holder}.`), form.text);
  step('the reset mail', `its code ${mask(resetCode)} opens /reset-password/ for ${holder}`);
  const saved = await post(asker, '/auth/password/', {
    step: 'update',
    password: second,
    confirm: second,
    locale: 'en',
  });
  assert.equal(saved.headers.get('location'), '/sign-in/?notice=password');
  assert.equal((await signIn(first)).headers.get('location'), '/sign-in/?error=credentials');
  assert.equal((await signIn(second)).headers.get('location'), '/sign-in/');
  step('POST /auth/password/ (update)', 'the new password signs in, the old one no longer');
  await signOut();

  // 8. The reset mail's link, in the browser that asked, in Russian.
  await new Promise(resolve => setTimeout(resolve, 1100)); // Auth sends one mail a second to an address.
  const linkBrowser = new Jar();
  await post(linkBrowser, '/auth/password/', { step: 'forgot', email: holder, locale: 'ru', next: '/ru/pro/' });
  const again = await mailTo(holder, 2);
  const link = again.match(/href="([^"]*\/auth\/v1\/verify\?[^"]*)"/)?.[1]?.replace(/&amp;/g, '&');
  assert.ok(link, 'a link in the reset mail');
  const callback = (await fetch(link, { redirect: 'manual' })).headers.get('location') ?? '';
  assert.match(
    callback,
    /\/auth\/callback\/\?.*via=recovery.*code=|\/auth\/callback\/\?.*code=.*via=recovery/,
    callback,
  );
  const landed = await page(linkBrowser, new URL(callback).pathname + new URL(callback).search);
  assert.deepEqual([landed.status, landed.location], [303, '/ru/reset-password/?next=%2Fru%2Fpro%2F']);
  const ready = await page(linkBrowser, '/ru/reset-password/?next=%2Fru%2Fpro%2F');
  assert.ok(ready.text.includes(`Выберите новый пароль для ${holder}.`), ready.text);
  step('the reset link', '/auth/callback/ sends this browser on to /ru/reset-password/ with the form');
  const elsewhere = await page(new Jar(), '/ru/reset-password/');
  assert.ok(!elsewhere.text.includes('Выберите новый пароль'), 'another browser gets no form');
  step('/reset-password/ in another browser', 'no form');
}

console.log('sign-in: every step passed');
