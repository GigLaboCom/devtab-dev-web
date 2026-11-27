/**
 * The localised pages the server renders on each request
 * (`export const prerender = false`), by id: the locale-neutral path, whether
 * search engines may index the page, and whether it reads the visitor's
 * session. The sitemap lists the indexed pages; the others carry `noindex`.
 * A page that reads the session is personal and never cached; one that does
 * not may be (`src/middleware.ts` reads the session only for those that do).
 * Like the pages of `src/lib/pages.ts`, each has `pages.<id>` (title and
 * description) in every catalogue, and tests/server.test.ts checks every one
 * in every locale on the built server.
 */
interface ServerPage {
  readonly path: string;
  readonly index: boolean;
  readonly session: boolean;
}

const list = {
  pro: { path: 'pro/', index: true, session: false },
  signIn: { path: 'sign-in/', index: false, session: true },
};

export type ServerPageId = keyof typeof list;

export const serverPages: Readonly<Record<ServerPageId, ServerPage>> = list;
