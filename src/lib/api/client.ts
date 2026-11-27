/**
 * The site's server calls the API's internal listener from the pages it
 * renders; the browser never sees the API. Its address is
 * DEVTAB_API_INTERNAL_URL, read when the server runs (in development the
 * API's internal listener on the loopback address). The bodies' types come
 * from the API's own description (`schema.gen.ts`, written by script/gen).
 */
import type { components } from './schema.gen.ts';

export type SiteCatalogue = components['schemas']['SiteCatalogue'];
export type SitePlan = components['schemas']['SitePlan'];

/** How long a page waits for the API before it gives up. */
const TIMEOUT_MS = 5000;

/** Where the server reaches the API's internal listener. */
export function apiBaseUrl(): string {
  return (process.env.DEVTAB_API_INTERNAL_URL?.trim() || 'http://127.0.0.1:8081').replace(/\/+$/, '');
}

/**
 * Calls the internal API, as the signed-in person of `locals` when there is
 * one (`Authorization: Bearer <their access token>`), else anonymously. Never
 * throws: an API that cannot be reached, is too slow or answers something
 * that is not JSON gives status 503 and no body.
 */
export async function apiFromServer<T>(
  locals: App.Locals | null,
  path: string,
  init: RequestInit = {},
): Promise<{ status: number; body: T | null }> {
  const headers = new Headers(init.headers);
  if (locals?.auth) headers.set('authorization', `Bearer ${locals.auth.accessToken}`);
  try {
    const response = await fetch(`${apiBaseUrl()}${path}`, {
      ...init,
      headers,
      signal: AbortSignal.timeout(TIMEOUT_MS),
    });
    const text = await response.text();
    return { status: response.status, body: text ? (JSON.parse(text) as T) : null };
  } catch {
    return { status: 503, body: null };
  }
}

/** The plans' catalogue in `locale`, or null when the API cannot give it. */
export async function plans(locale: string): Promise<SiteCatalogue | null> {
  const { status, body } = await apiFromServer<SiteCatalogue>(
    null,
    `/internal/plans?locale=${encodeURIComponent(locale)}`,
  );
  if (status !== 200 || !body || !Array.isArray(body.plans)) {
    console.error(`GET /internal/plans answered ${status}`);
    return null;
  }
  return body;
}
