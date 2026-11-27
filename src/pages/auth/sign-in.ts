/**
 * `POST /auth/sign-in/`: the sign-in page's forms, without client-side
 * JavaScript. Each answers 303 back to the sign-in page (or, signed in, to
 * `next`); an error comes back as `?error=<code>`, which the page words.
 *
 * - `step=email`: sends a sign-in mail through Supabase Auth: a link, which
 *   works in this browser (its PKCE verifier is a cookie of this response),
 *   and a one-time code, which works anywhere. The address waits in a cookie
 *   for the code.
 * - `step=code`: checks the code for that address; the session's cookies come
 *   with the answer.
 * - `step=reset`: forgets the address, to start again with another one.
 *
 * At most DEVTAB_RATE_SIGN_IN_PER_IP attempts an hour per client address
 * (10), before Supabase Auth's own limits. Astro's origin check refuses a
 * form posted from another site.
 */
import type { APIRoute } from 'astro';
import { defaultLocale, locales, type Locale } from '../../i18n/config.ts';
import { authConfig, safeNext } from '../../lib/auth/config.ts';
import {
  PENDING_COOKIE,
  allowAttempt,
  clearPending,
  clientAddress,
  setPending,
  signInHref,
} from '../../lib/auth/server.ts';

export const prerender = false;

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
const CODE = /^\d{6,10}$/;

export const POST: APIRoute = async ({ request, url, cookies, locals, redirect, clientAddress: peer }) => {
  const form = await request.formData().catch(() => new FormData());
  const field = (name: string) => form.get(name)?.toString().trim() ?? '';
  const locale: Locale = locales.find(l => l === field('locale')) ?? defaultLocale;
  const nextRaw = field('next');
  const next = nextRaw ? safeNext(nextRaw, '') || null : null;
  const back = (error?: string) => redirect(signInHref(locale, next, error), 303);

  if (field('step') === 'reset') {
    clearPending(cookies);
    return back();
  }
  if (!authConfig() || !locals.supabase) return back('unavailable');
  if (!allowAttempt(clientAddress(request, peer))) return back('rate');

  if (field('step') === 'email') {
    const email = field('email').toLowerCase();
    if (email.length > 254 || !EMAIL.test(email)) return back('email');
    const callback = new URL('/auth/callback/', url.origin);
    callback.searchParams.set('locale', locale);
    if (next) callback.searchParams.set('next', next);
    const { error } = await locals.supabase.auth.signInWithOtp({
      email,
      options: { emailRedirectTo: callback.href, shouldCreateUser: true, data: { locale } },
    });
    if (error) {
      console.error(`sign-in: sending the mail failed (${error.status ?? '-'} ${error.code ?? '-'})`);
      return back(error.status === 429 ? 'rate' : 'send');
    }
    setPending(cookies, url, email);
    return back();
  }

  if (field('step') === 'code') {
    const email = cookies.get(PENDING_COOKIE)?.value;
    if (!email) return back();
    const code = field('code').replace(/\s+/g, '');
    if (!CODE.test(code)) return back('code');
    const { error } = await locals.supabase.auth.verifyOtp({ email, token: code, type: 'email' });
    if (error) {
      console.error(`sign-in: the code was refused (${error.status ?? '-'} ${error.code ?? '-'})`);
      return back(error.status === 429 ? 'rate' : 'code');
    }
    clearPending(cookies);
    return redirect(next ?? signInHref(locale), 303);
  }

  return back();
};
