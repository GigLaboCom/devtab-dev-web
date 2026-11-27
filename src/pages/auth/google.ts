/**
 * `GET /auth/google/?locale=<locale>&next=<path>`: signing in with Google
 * (the `google` method), through Supabase Auth's Google provider. The server
 * asks Supabase Auth for the provider's address (`signInWithOAuth`, PKCE: the
 * verifier is a cookie of this response, as for a mail's link) and answers
 * 303 there; Google comes back through Supabase Auth to `/auth/callback/`
 * (`via=google`), with `locale` and `next`.
 *
 * A link, not a form: the content security policy's `form-action 'self'`
 * would stop a form's redirect to another host. Only from the site's own
 * pages (`Sec-Fetch-Site`, when the browser sends it, is `same-origin`, or
 * `none` for an address typed in); anything else goes back to the sign-in
 * page without starting. 404 when the method is off. Counted with the other
 * forms of signing in (DEVTAB_RATE_SIGN_IN_PER_IP).
 */
import type { APIRoute } from 'astro';
import { defaultLocale, locales, type Locale } from '../../i18n/config.ts';
import { authConfig, enabledMethods, safeNext } from '../../lib/auth/config.ts';
import { allowAttempt, callbackUrl, clientAddress, logAuthError, signInHref } from '../../lib/auth/server.ts';

export const prerender = false;

export const GET: APIRoute = async ({ request, url, locals, redirect, clientAddress: peer }) => {
  if (!enabledMethods().has('google')) return new Response(null, { status: 404 });
  const locale: Locale = locales.find(l => l === url.searchParams.get('locale')) ?? defaultLocale;
  const nextRaw = url.searchParams.get('next');
  const next = nextRaw ? safeNext(nextRaw, '') || null : null;
  const back = (error?: string) => redirect(signInHref(locale, next, error), 303);

  const site = request.headers.get('sec-fetch-site');
  if (site !== null && site !== 'same-origin' && site !== 'none') return back();
  if (!authConfig() || !locals.supabase) return back('unavailable');
  if (!allowAttempt(clientAddress(request, peer))) return back('rate');

  const { data, error } = await locals.supabase.auth.signInWithOAuth({
    provider: 'google',
    options: { redirectTo: callbackUrl(url.origin, locale, next, 'google') },
  });
  if (error || !data.url) {
    if (error) logAuthError('starting Google failed', error);
    return back('google');
  }
  return redirect(data.url, 303);
};
