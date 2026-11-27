/**
 * `GET /auth/callback/`: where Supabase Auth sends the browser back with a
 * PKCE `code`: from the link of a sign-in or sign-up mail, or of a password
 * reset mail (`via=recovery`). Exchanging it
 * (with the verifier cookie of the browser that asked) sets the session's
 * cookies; then on to `next`, or the sign-in page, which shows who is signed
 * in. A password reset goes on to /reset-password/ instead, marking this
 * session as the one that may set the new password.
 *
 * A link opened in another browser, used twice or expired comes back with
 * `?error=link` to the sign-in page (or the password reset page): the code
 * of the same mail still works there.
 */
import type { APIRoute } from 'astro';
import { defaultLocale, locales, type Locale } from '../../i18n/config.ts';
import { safeNext } from '../../lib/auth/config.ts';
import {
  RECOVERY_PENDING_COOKIE,
  authHref,
  clearPending,
  logAuthError,
  setResetSession,
  signInHref,
} from '../../lib/auth/server.ts';

export const prerender = false;

export const GET: APIRoute = async ({ url, locals, cookies, redirect }) => {
  const locale: Locale = locales.find(l => l === url.searchParams.get('locale')) ?? defaultLocale;
  const nextRaw = url.searchParams.get('next');
  const next = nextRaw ? safeNext(nextRaw, '') || null : null;
  const via = url.searchParams.get('via');
  const code = url.searchParams.get('code');
  if (code && locals.supabase) {
    const { data, error } = await locals.supabase.auth.exchangeCodeForSession(code);
    if (!error && data.session) {
      // supabase-js reads the flow's kind from the verifier it stored: a reset's is `recovery`.
      if ((data as { redirectType?: string | null }).redirectType === 'recovery') {
        clearPending(cookies, RECOVERY_PENDING_COOKIE);
        if (setResetSession(cookies, url, data.session.access_token)) {
          return redirect(authHref(locale, 'resetPassword', next), 303);
        }
      } else {
        clearPending(cookies);
      }
      return redirect(next ?? signInHref(locale), 303);
    }
    if (error) logAuthError("the link's code was refused", error);
  }
  if (via === 'recovery') return redirect(authHref(locale, 'forgotPassword', next, { error: 'link' }), 303);
  return redirect(signInHref(locale, next, 'link'), 303);
};
