/**
 * `POST /auth/password/`: the forms of the `password` sign-in method, without
 * client-side JavaScript. Each answers 303 back to its page (or, signed in,
 * to `next`); an error comes back as `?error=<code>`, which the page words.
 *
 * - `step=sign-in` (/sign-in/): an address and its password.
 * - `step=sign-up` (/sign-up/): a new account. Supabase Auth mails a link
 *   back to `/auth/callback/` and a code, as the `email` method's mail does,
 *   to confirm the address; the address waits for the code in the same
 *   cookie, and the sign-in page asks for it. A project that does not ask
 *   for a confirmation signs the person in at once.
 * - `step=forgot` (/forgot-password/): mails a link back to
 *   `/auth/callback/` (`via=recovery`) and a code; the address waits for the
 *   code in a cookie of its own.
 * - `step=code`: checks that code; `step=again` forgets that address.
 * - `step=update` (/reset-password/): the new password, only in the session
 *   that the reset mail's link or code started (`RESET_COOKIE`).
 *
 * All of it answers 404 when the method is off. At most
 * DEVTAB_RATE_SIGN_IN_PER_IP attempts an hour per client address, counted
 * with the other forms of signing in. Astro's origin check refuses a form
 * posted from another site. A password is never logged or put in an address.
 */
import type { APIRoute } from 'astro';
import { authConfig, enabledMethods } from '../../lib/auth/config.ts';
import {
  type AuthPageId,
  CODE,
  RECOVERY_PENDING_COOKIE,
  RESET_COOKIE,
  allowAttempt,
  authHref,
  callbackUrl,
  clearPending,
  clientAddress,
  emailOf,
  inResetSession,
  logAuthError,
  readAuthForm,
  setPending,
  setResetSession,
  signInHref,
} from '../../lib/auth/server.ts';

export const prerender = false;

/** The page each step's errors come back to. */
const pageOf: Record<string, AuthPageId> = {
  'sign-in': 'signIn',
  'sign-up': 'signUp',
  forgot: 'forgotPassword',
  code: 'forgotPassword',
  again: 'forgotPassword',
  update: 'resetPassword',
};

/** Supabase Auth refuses a password longer than this many characters. */
const PASSWORD_MAX = 72;

export const POST: APIRoute = async ({ request, url, cookies, locals, redirect, clientAddress: peer }) => {
  if (!enabledMethods().has('password')) return new Response(null, { status: 404 });
  const { field, secret, locale, next } = await readAuthForm(request);
  const step = field('step');
  const page = pageOf[step] ?? 'signIn';
  const back = (error?: string) => redirect(authHref(locale, page, next, { error }), 303);
  const done = () => redirect(next ?? signInHref(locale), 303);

  if (step === 'again') {
    clearPending(cookies, RECOVERY_PENDING_COOKIE);
    return back();
  }
  if (!(step in pageOf)) return back();
  if (!authConfig() || !locals.supabase) return back('unavailable');
  const auth = locals.supabase.auth;
  if (!allowAttempt(clientAddress(request, peer))) return back('rate');

  /** A new password from the form: its error, or none. */
  const newPassword = (): string | null => {
    const password = secret('password');
    if (password.length === 0) return 'password-weak';
    if (password.length > PASSWORD_MAX) return 'password-long';
    if (password !== secret('confirm')) return 'password-mismatch';
    return null;
  };

  if (step === 'sign-in') {
    const email = emailOf(field('email'));
    if (!email) return back('email');
    const password = secret('password');
    if (password.length === 0 || password.length > PASSWORD_MAX) return back('credentials');
    const { error } = await auth.signInWithPassword({ email, password });
    if (error) {
      logAuthError('the password was refused', error);
      if (error.status === 429) return back('rate');
      if (error.code === 'email_not_confirmed') return back('unconfirmed');
      return back(error.code === 'invalid_credentials' || error.status === 400 ? 'credentials' : 'failed');
    }
    return done();
  }

  if (step === 'sign-up') {
    const email = emailOf(field('email'));
    if (!email) return back('email');
    const problem = newPassword();
    if (problem) return back(problem);
    const { data, error } = await auth.signUp({
      email,
      password: secret('password'),
      options: { emailRedirectTo: callbackUrl(url.origin, locale, next), data: { locale } },
    });
    if (error) {
      logAuthError('signing up failed', error);
      if (error.status === 429) return back('rate');
      if (error.code === 'weak_password') return back('password-weak');
      if (error.code === 'user_already_exists' || error.code === 'email_exists') return back('exists');
      return back(error.code === 'signup_disabled' ? 'unavailable' : 'send');
    }
    if (data.session) return done();
    // The confirmation mail is on its way: the sign-in page asks for its code.
    setPending(cookies, url, email);
    return redirect(signInHref(locale, next), 303);
  }

  if (step === 'forgot') {
    const email = emailOf(field('email'));
    if (!email) return back('email');
    const { error } = await auth.resetPasswordForEmail(email, {
      redirectTo: callbackUrl(url.origin, locale, next, 'recovery'),
    });
    if (error) {
      logAuthError('sending the reset mail failed', error);
      return back(error.status === 429 ? 'rate' : 'send');
    }
    setPending(cookies, url, email, RECOVERY_PENDING_COOKIE);
    return back();
  }

  if (step === 'code') {
    const email = cookies.get(RECOVERY_PENDING_COOKIE)?.value;
    if (!email) return back();
    const code = field('code').replace(/\s+/g, '');
    if (!CODE.test(code)) return back('code');
    const { data, error } = await auth.verifyOtp({ email, token: code, type: 'recovery' });
    if (error || !data.session) {
      if (error) logAuthError('the reset code was refused', error);
      return back(error?.status === 429 ? 'rate' : 'code');
    }
    clearPending(cookies, RECOVERY_PENDING_COOKIE);
    if (!setResetSession(cookies, url, data.session.access_token)) return back('code');
    return redirect(authHref(locale, 'resetPassword', next), 303);
  }

  // step === 'update'
  if (!inResetSession(cookies, locals.auth)) return back();
  const problem = newPassword();
  if (problem) return back(problem);
  const { error } = await auth.updateUser({ password: secret('password') });
  if (error) {
    logAuthError('setting the new password failed', error);
    if (error.status === 429) return back('rate');
    if (error.code === 'weak_password') return back('password-weak');
    return back(error.code === 'same_password' ? 'password-same' : 'update');
  }
  clearPending(cookies, RESET_COOKIE);
  return redirect(next ?? authHref(locale, 'signIn', null, { notice: 'password' }), 303);
};
