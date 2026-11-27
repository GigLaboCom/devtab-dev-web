/**
 * `POST /auth/sign-in/`: the sign-in page's e-mail forms (the `email`
 * method), without client-side JavaScript. Each answers 303 back to the
 * sign-in page (or, signed in, to `next`); an error comes back as
 * `?error=<code>`, which the page words.
 *
 * - `step=email`: sends a sign-in mail through Supabase Auth: a link, which
 *   works in this browser (its PKCE verifier is a cookie of this response),
 *   and a one-time code, which works anywhere. The address waits in a cookie
 *   for the code.
 * - `step=code`: checks the code for that address; the session's cookies come
 *   with the answer. A sign-up's confirmation mail (the `password` method)
 *   carries the same kind of code, so this step serves either method.
 * - `step=reset`: forgets the address, to start again with another one.
 *
 * A step of a method that is off answers 404. At most
 * DEVTAB_RATE_SIGN_IN_PER_IP attempts an hour per client address (10),
 * before Supabase Auth's own limits. Astro's origin check refuses a form
 * posted from another site.
 */
import type { APIRoute } from 'astro';
import { authConfig, enabledMethods } from '../../lib/auth/config.ts';
import {
  CODE,
  PENDING_COOKIE,
  allowAttempt,
  callbackUrl,
  clearPending,
  clientAddress,
  emailOf,
  logAuthError,
  readAuthForm,
  setPending,
  signInHref,
} from '../../lib/auth/server.ts';

export const prerender = false;

const notFound = () => new Response(null, { status: 404 });

export const POST: APIRoute = async ({ request, url, cookies, locals, redirect, clientAddress: peer }) => {
  const { field, locale, next } = await readAuthForm(request);
  const back = (error?: string) => redirect(signInHref(locale, next, error), 303);
  const methods = enabledMethods();
  const step = field('step');
  if (step === 'email' ? !methods.has('email') : !methods.has('email') && !methods.has('password')) {
    return notFound();
  }

  if (step === 'reset') {
    clearPending(cookies);
    return back();
  }
  if (!authConfig() || !locals.supabase) return back('unavailable');
  if (!allowAttempt(clientAddress(request, peer))) return back('rate');

  if (step === 'email') {
    const email = emailOf(field('email'));
    if (!email) return back('email');
    const { error } = await locals.supabase.auth.signInWithOtp({
      email,
      options: { emailRedirectTo: callbackUrl(url.origin, locale, next), shouldCreateUser: true, data: { locale } },
    });
    if (error) {
      logAuthError('sending the mail failed', error);
      return back(error.status === 429 ? 'rate' : 'send');
    }
    setPending(cookies, url, email);
    return back();
  }

  if (step === 'code') {
    const email = cookies.get(PENDING_COOKIE)?.value;
    if (!email) return back();
    const code = field('code').replace(/\s+/g, '');
    if (!CODE.test(code)) return back('code');
    const { error } = await locals.supabase.auth.verifyOtp({ email, token: code, type: 'email' });
    if (error) {
      logAuthError('the code was refused', error);
      return back(error.status === 429 ? 'rate' : 'code');
    }
    clearPending(cookies);
    return redirect(next ?? signInHref(locale), 303);
  }

  return back();
};
