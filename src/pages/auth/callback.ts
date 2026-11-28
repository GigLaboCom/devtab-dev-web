/**
 * `GET /auth/callback/`: where Supabase Auth sends the browser back with a
 * PKCE `code`: from the link of a sign-in or sign-up mail, of a password
 * reset mail (`via=recovery`), or from Google (`via=google`). Exchanging it
 * (with the verifier cookie of the browser that asked) sets the session's
 * cookies; then on to `next`, or the sign-in page, which shows who is signed
 * in. A password reset goes on to /reset-password/ instead, marking this
 * session as the one that may set the new password.
 *
 * A link opened in another browser, used twice or expired comes back with
 * `?error=link` to the sign-in page (or the password reset page): the code
 * of the same mail still works there. A refused or abandoned Google sign-in
 * comes back with `?error=google`.
 *
 * The invite mail of a purchase made without an account
 * (supabase/templates/invite.html) links here with `token_hash` and
 * `type=invite` instead: no PKCE (the API asked for the invite, not this
 * browser), so the hash is verified here (`verifyOtp`), which confirms the
 * address and starts the session. Where to go is decided here, never by the
 * link: the purchase's page, from the account's metadata (`source`
 * "purchase", `checkout`, `locale`, which the API set), else the sign-in
 * page. The invited person has no password; where the deployment offers
 * passwords, this session may set one on /reset-password/, as after a reset
 * mail, but nothing asks for it. A link that fails comes back to the sign-in
 * page with `?error=invite`.
 */
import type { APIRoute } from 'astro';
import { getRelativeLocaleUrl } from 'astro:i18n';
import { defaultLocale, locales, type Locale } from '../../i18n/config.ts';
import { enabledMethods, safeNext } from '../../lib/auth/config.ts';
import {
  RECOVERY_PENDING_COOKIE,
  authHref,
  clearPending,
  logAuthError,
  setResetSession,
  signInHref,
} from '../../lib/auth/server.ts';
import { checkoutRef } from '../../lib/checkout.ts';

export const prerender = false;

/** An invited account's landing, from the metadata the API gave it: the purchase's page, else the sign-in page. */
function landingOf(metadata: Record<string, unknown> | undefined, fallback: Locale): string {
  const locale = locales.find(l => l === metadata?.locale) ?? fallback;
  const checkout = typeof metadata?.checkout === 'string' ? checkoutRef(metadata.checkout) : null;
  if (metadata?.source === 'purchase' && checkout) {
    return `${getRelativeLocaleUrl(locale, 'pro/thanks/')}?ref=${checkout}`;
  }
  return signInHref(locale);
}

/** A token hash as Supabase Auth makes them. */
const TOKEN_HASH = /^[A-Za-z0-9_-]{16,200}$/;

export const GET: APIRoute = async ({ url, locals, cookies, redirect }) => {
  const locale: Locale = locales.find(l => l === url.searchParams.get('locale')) ?? defaultLocale;
  const nextRaw = url.searchParams.get('next');
  const next = nextRaw ? safeNext(nextRaw, '') || null : null;
  const via = url.searchParams.get('via');
  const tokenHash = url.searchParams.get('token_hash');
  if (tokenHash !== null && url.searchParams.get('type') === 'invite') {
    if (locals.supabase && TOKEN_HASH.test(tokenHash)) {
      const { data, error } = await locals.supabase.auth.verifyOtp({ token_hash: tokenHash, type: 'invite' });
      if (!error && data.session) {
        clearPending(cookies);
        if (enabledMethods().has('password')) setResetSession(cookies, url, data.session.access_token);
        return redirect(landingOf(data.user?.user_metadata, locale), 303);
      }
      if (error) logAuthError("the invite's link was refused", error);
    }
    return redirect(signInHref(locale, null, 'invite'), 303);
  }
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
  return redirect(signInHref(locale, next, via === 'google' ? 'google' : 'link'), 303);
};
