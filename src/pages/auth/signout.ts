/**
 * `POST /auth/signout/`: signs this browser out (Supabase Auth revokes the
 * session's refresh token and the cookies are cleared), then back to `next`
 * or the sign-in page. A plain form; Astro's origin check guards it.
 */
import type { APIRoute } from 'astro';
import { defaultLocale, locales, type Locale } from '../../i18n/config.ts';
import { safeNext } from '../../lib/auth/config.ts';
import { signInHref } from '../../lib/auth/server.ts';

export const prerender = false;

export const POST: APIRoute = async ({ request, locals, redirect }) => {
  const form = await request.formData().catch(() => new FormData());
  const locale: Locale = locales.find(l => l === form.get('locale')?.toString()) ?? defaultLocale;
  const next = safeNext(form.get('next')?.toString(), signInHref(locale));
  await locals.supabase?.auth.signOut({ scope: 'local' });
  return redirect(next, 303);
};
