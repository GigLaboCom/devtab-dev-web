/**
 * The server side of sign-in: the session lives in cookies that
 * `@supabase/ssr` manages, read and refreshed on the pages that need it (see
 * `src/middleware.ts`); forms post to the endpoints under `/auth/`, which call
 * Supabase Auth from the server and answer 303. No script runs in the
 * browser, so the cookies are `HttpOnly`.
 */
import { createServerClient, parseCookieHeader } from '@supabase/ssr';
import type { AstroCookies } from 'astro';
import { getRelativeLocaleUrl } from 'astro:i18n';
import type { Locale } from '../../i18n/config.ts';
import { serverPages } from '../serverPages.ts';
import type { AuthConfig } from './config.ts';
import { signInLimit } from './config.ts';

export function serverClient(cfg: AuthConfig, request: Request, cookies: AstroCookies, responseHeaders: Headers) {
  const secure = new URL(request.url).protocol === 'https:';
  return createServerClient(cfg.url, cfg.key, {
    cookieOptions: { httpOnly: true, secure, sameSite: 'lax', path: '/' },
    cookies: {
      getAll() {
        return parseCookieHeader(request.headers.get('cookie') ?? '').map(({ name, value }) => ({
          name,
          value: value ?? '',
        }));
      },
      setAll(toSet, headers) {
        for (const { name, value, options } of toSet) cookies.set(name, value, options);
        // A response that sets the session's cookies is never cached.
        for (const [key, value] of Object.entries(headers)) responseHeaders.set(key, value);
      },
    },
  });
}

/** The sign-in page in `locale`, coming back to `next` afterwards, with an error or none. */
export function signInHref(locale: Locale, next?: string | null, error?: string): string {
  const query = new URLSearchParams();
  if (next) query.set('next', next);
  if (error) query.set('error', error);
  const page = getRelativeLocaleUrl(locale, serverPages.signIn.path);
  return query.size > 0 ? `${page}?${query}` : page;
}

/**
 * The address a code was sent to, between the two forms of a sign-in: in a
 * cookie only this site reads, for as long as the code lives, never in an
 * address or a page's markup beyond the sign-in page itself.
 */
export const PENDING_COOKIE = 'devtab-sign-in';
export const PENDING_MAX_AGE_S = 3600;

export function setPending(cookies: AstroCookies, url: URL, email: string): void {
  cookies.set(PENDING_COOKIE, email, {
    httpOnly: true,
    secure: url.protocol === 'https:',
    sameSite: 'lax',
    path: '/',
    maxAge: PENDING_MAX_AGE_S,
  });
}

export function clearPending(cookies: AstroCookies): void {
  cookies.delete(PENDING_COOKIE, { path: '/' });
}

/** The client's address: the last `X-Forwarded-For` entry (the site's own proxy sets it), else the peer. */
export function clientAddress(request: Request, peer: string | undefined): string {
  const forwarded = request.headers.get('x-forwarded-for');
  const last = forwarded?.split(',').at(-1)?.trim();
  return last || peer || 'unknown';
}

const attempts = new Map<string, number[]>();
const HOUR_MS = 3_600_000;

/**
 * Counts one sign-in attempt by `address` and says whether it is within the
 * limit for the last hour (`DEVTAB_RATE_SIGN_IN_PER_IP`). The count lives in
 * this process's memory: one server, as the site runs.
 */
export function allowAttempt(address: string, now = Date.now()): boolean {
  const recent = (attempts.get(address) ?? []).filter(at => now - at < HOUR_MS);
  if (attempts.size > 10_000) {
    for (const [key, times] of attempts) if (times.every(at => now - at >= HOUR_MS)) attempts.delete(key);
  }
  const allowed = recent.length < signInLimit();
  if (allowed) recent.push(now);
  attempts.set(address, recent);
  return allowed;
}
