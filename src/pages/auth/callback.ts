/**
 * `GET /auth/callback/`: where the link of a sign-in mail comes back, through
 * Supabase Auth, with a PKCE `code`. Exchanging it (with the verifier cookie
 * of the browser that asked) sets the session's cookies; then on to `next`,
 * or the sign-in page, which shows who is signed in. A link opened in another
 * browser, used twice or expired comes back to the sign-in page with
 * `?error=link`: the code of the same mail still works there.
 */
import type { APIRoute } from 'astro';
import { defaultLocale, locales, type Locale } from '../../i18n/config.ts';
import { safeNext } from '../../lib/auth/config.ts';
import { clearPending, signInHref } from '../../lib/auth/server.ts';

export const prerender = false;

export const GET: APIRoute = async ({ url, locals, cookies, redirect }) => {
  const locale: Locale = locales.find(l => l === url.searchParams.get('locale')) ?? defaultLocale;
  const nextRaw = url.searchParams.get('next');
  const next = nextRaw ? safeNext(nextRaw, '') || null : null;
  const code = url.searchParams.get('code');
  if (code && locals.supabase) {
    const { error } = await locals.supabase.auth.exchangeCodeForSession(code);
    if (!error) {
      clearPending(cookies);
      return redirect(next ?? signInHref(locale), 303);
    }
    console.error(`sign-in: the link's code was refused (${error.status ?? '-'} ${error.code ?? '-'})`);
  }
  return redirect(signInHref(locale, next, 'link'), 303);
};
