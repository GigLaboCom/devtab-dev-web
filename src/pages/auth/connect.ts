/**
 * POST /auth/connect/: the connect page's script asks, for the signed-in
 * person, for an activation of their licence for the installation the
 * extension named, and gets what the extension stores (`lid`, `token`,
 * `secret`), to hand it on. Only from the site's own pages (the `Origin`
 * must be the site's), only for a signed-in person, never cached.
 */
import type { APIRoute } from 'astro';
import { apiFromServer } from '../../lib/api/client';

export const prerender = false;

const json = (status: number, body: unknown): Response =>
  new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json', 'cache-control': 'no-store' },
  });

export const POST: APIRoute = async ({ request, locals, url }) => {
  if (request.headers.get('origin') !== url.origin) return json(403, { error: 'forbidden' });
  if (!locals.auth) return json(401, { error: 'unauthorized' });
  let body: { inst?: unknown; lid?: unknown };
  try {
    body = (await request.json()) as { inst?: unknown; lid?: unknown };
  } catch {
    return json(400, { error: 'bad-request' });
  }
  if (typeof body.inst !== 'string' || (body.lid !== undefined && typeof body.lid !== 'string')) {
    return json(400, { error: 'bad-request' });
  }
  const { status, body: answer } = await apiFromServer<unknown>(locals, '/internal/licences/connect', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ inst: body.inst, lid: body.lid }),
  });
  return json(status, answer ?? { error: 'unavailable' });
};
