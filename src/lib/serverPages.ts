/**
 * The localised pages the server renders on each request
 * (`export const prerender = false`), by id: the locale-neutral path, whether
 * search engines may index the page, and whether it reads the visitor's
 * session. The sitemap lists the indexed pages; the others carry `noindex`.
 * A page that reads the session is personal and never cached; one that does
 * not may be (`src/middleware.ts` reads the session only for those that do).
 * A page that needs a signed-in person sends anyone else to the sign-in page
 * and back (`signIn`); a page with `script` runs one script of its own, built
 * and hashed into the policy by Astro (the others run none).
 * Like the pages of `src/lib/pages.ts`, each has `pages.<id>` (title and
 * description) in every catalogue, and tests/server.test.ts checks every one
 * in every locale on the built server.
 */
interface ServerPage {
  readonly path: string;
  readonly index: boolean;
  readonly session: boolean;
  readonly signIn: boolean;
  readonly script: boolean;
}

const list = {
  pro: { path: 'pro/', index: true, session: false, signIn: false, script: false },
  signIn: { path: 'sign-in/', index: false, session: true, signIn: false, script: false },
  connect: { path: 'pro/connect/', index: false, session: true, signIn: true, script: true },
};

export type ServerPageId = keyof typeof list;

export const serverPages: Readonly<Record<ServerPageId, ServerPage>> = list;
