/**
 * Reads the Supabase session on the server's pages that need it (the sign-in
 * page and the endpoints under `/auth/`); prerendered pages and the public,
 * cacheable server pages (`/pro/`) are left alone, so that no session cookie
 * is ever set on a response a cache may keep. `locals.auth` is the signed-in
 * person with a verified token, or null; `locals.supabase` is the request's
 * server client of Supabase Auth.
 */
import { defineMiddleware } from 'astro:middleware';
import { localeOfParam } from './i18n';
import { authConfig } from './lib/auth/config';
import { serverClient } from './lib/auth/server';
import { serverPages } from './lib/serverPages';

/** The paths of the server's pages that read the session, in every locale. */
function readsSession(path: string): boolean {
  if (path.startsWith('/auth/')) return true;
  const [, first = '', ...rest] = path.split('/');
  const prefixed = localeOfParam(first) !== null && first !== '';
  const neutral = prefixed ? rest.join('/') : path.slice(1);
  return Object.values(serverPages).some(page => page.session && page.path === neutral);
}

export const onRequest = defineMiddleware(async (context, next) => {
  context.locals.auth = null;
  context.locals.supabase = null;
  const cfg = context.isPrerendered || !readsSession(context.url.pathname) ? null : authConfig();
  if (!cfg) return next();

  const headers = new Headers();
  const supabase = serverClient(cfg, context.request, context.cookies, headers);
  context.locals.supabase = supabase;
  // getClaims checks the token against the project's keys and refreshes an
  // expired session; the session's own fields are not trusted before that.
  const { data } = await supabase.auth.getClaims();
  if (data?.claims) {
    const { data: session } = await supabase.auth.getSession();
    if (session.session) {
      context.locals.auth = {
        userId: data.claims.sub,
        email: typeof data.claims.email === 'string' ? data.claims.email : null,
        accessToken: session.session.access_token,
      };
    }
  }

  const response = await next();
  headers.forEach((value, key) => response.headers.set(key, value));
  return response;
});
