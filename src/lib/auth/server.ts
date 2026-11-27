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
import { defaultLocale, locales, type Locale } from '../../i18n/config.ts';
import { serverPages } from '../serverPages.ts';
import type { AuthConfig } from './config.ts';
import { safeNext, signInLimit } from './config.ts';

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

/** The pages of signing in, by their id in `serverPages`. */
export type AuthPageId = 'signIn' | 'signUp' | 'forgotPassword' | 'resetPassword';

/**
 * One of the pages of signing in, in `locale`, coming back to `next`
 * afterwards, with an error or a notice (`?error=<code>`, `?notice=<code>`),
 * which the page words.
 */
export function authHref(
  locale: Locale,
  page: AuthPageId,
  next?: string | null,
  message?: { error?: string; notice?: string },
): string {
  const query = new URLSearchParams();
  if (next) query.set('next', next);
  if (message?.error) query.set('error', message.error);
  if (message?.notice) query.set('notice', message.notice);
  const path = getRelativeLocaleUrl(locale, serverPages[page].path);
  return query.size > 0 ? `${path}?${query}` : path;
}

/** The sign-in page in `locale`, coming back to `next` afterwards, with an error or none. */
export function signInHref(locale: Locale, next?: string | null, error?: string): string {
  return authHref(locale, 'signIn', next, { error });
}

/**
 * Where Supabase Auth sends the browser back, from a mail's link or from
 * Google: `/auth/callback/` with the page's language, `next`, and `via` for
 * what started it (`recovery`, `google`), so that a failure is worded for it.
 */
export function callbackUrl(origin: string, locale: Locale, next: string | null, via?: 'recovery' | 'google'): string {
  const callback = new URL('/auth/callback/', origin);
  callback.searchParams.set('locale', locale);
  if (next) callback.searchParams.set('next', next);
  if (via) callback.searchParams.set('via', via);
  return callback.href;
}

/**
 * A form posted to an endpoint under `/auth/`: its fields, trimmed, the
 * page's locale and `next`, kept to a path of this site.
 */
export async function readAuthForm(request: Request) {
  const form = await request.formData().catch(() => new FormData());
  const field = (name: string) => form.get(name)?.toString().trim() ?? '';
  const locale: Locale = locales.find(l => l === field('locale')) ?? defaultLocale;
  const nextRaw = field('next');
  const next = nextRaw ? safeNext(nextRaw, '') || null : null;
  /** A password as typed: never trimmed. */
  const secret = (name: string) => form.get(name)?.toString() ?? '';
  return { field, secret, locale, next };
}

export const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/;
export const CODE = /^\d{6,10}$/;
/** An address as the forms take it: lower case, or null when it is not one. */
export function emailOf(raw: string): string | null {
  const email = raw.toLowerCase();
  return email.length <= 254 && EMAIL.test(email) ? email : null;
}

/** Supabase Auth's error, for the log: its status and code, never its message (it may name the address). */
export function logAuthError(what: string, error: { status?: number; code?: string }): void {
  console.error(`sign-in: ${what} (${error.status ?? '-'} ${error.code ?? '-'})`);
}

/**
 * The address a code was sent to, between the two forms of a sign-in (or of
 * a sign-up's confirmation): in a cookie only this site reads, for as long
 * as the code lives, never in an address or a page's markup beyond the page
 * that asks for the code. `RECOVERY_PENDING_COOKIE` is the same for the code
 * of a password reset mail.
 */
export const PENDING_COOKIE = 'devtab-sign-in';
export const RECOVERY_PENDING_COOKIE = 'devtab-reset-address';
export const PENDING_MAX_AGE_S = 3600;

/**
 * The session a password reset mail started (by its link or its code),
 * named by its id: only that session may set a new password, on
 * /reset-password/, and for an hour at most.
 */
export const RESET_COOKIE = 'devtab-reset';

export function setPending(cookies: AstroCookies, url: URL, value: string, name = PENDING_COOKIE): void {
  cookies.set(name, value, {
    httpOnly: true,
    secure: url.protocol === 'https:',
    sameSite: 'lax',
    path: '/',
    maxAge: PENDING_MAX_AGE_S,
  });
}

export function clearPending(cookies: AstroCookies, name = PENDING_COOKIE): void {
  cookies.delete(name, { path: '/' });
}

/** The `session_id` claim of an access token Supabase Auth has just handed this server. */
export function sessionIdOf(accessToken: string): string | null {
  try {
    const claims = JSON.parse(Buffer.from(accessToken.split('.')[1] ?? '', 'base64url').toString('utf8')) as {
      session_id?: unknown;
    };
    return typeof claims.session_id === 'string' && claims.session_id ? claims.session_id : null;
  } catch {
    return null;
  }
}

/** Marks the session a password reset mail started as the one that may set a new password. */
export function setResetSession(cookies: AstroCookies, url: URL, accessToken: string): boolean {
  const id = sessionIdOf(accessToken);
  if (id) setPending(cookies, url, id, RESET_COOKIE);
  return id !== null;
}

/** Whether the signed-in person's session is the one a password reset mail started. */
export function inResetSession(cookies: AstroCookies, auth: App.Locals['auth']): boolean {
  const marked = cookies.get(RESET_COOKIE)?.value;
  return Boolean(auth?.sessionId && marked && marked === auth.sessionId);
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
 * Counts one attempt by `address` at any of the forms of signing in, and
 * says whether it is within the limit for the last hour
 * (`DEVTAB_RATE_SIGN_IN_PER_IP`). The count lives in this process's memory:
 * one server, as the site runs.
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
