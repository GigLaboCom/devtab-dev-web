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
  // The pages of the `password` sign-in method (src/lib/auth/config.ts): 404 when it is off.
  signUp: { path: 'sign-up/', index: false, session: true, signIn: false, script: false },
  forgotPassword: { path: 'forgot-password/', index: false, session: true, signIn: false, script: false },
  resetPassword: { path: 'reset-password/', index: false, session: true, signIn: false, script: false },
  connect: { path: 'pro/connect/', index: false, session: true, signIn: true, script: true },
  // Buying, with or without an account, and the return from the payment page.
  checkout: { path: 'pro/checkout/', index: false, session: true, signIn: false, script: false },
  thanks: { path: 'pro/thanks/', index: false, session: true, signIn: false, script: false },
  // The trial without a card: a signed-in person starts it.
  trial: { path: 'pro/trial/', index: false, session: true, signIn: false, script: false },
};

export type ServerPageId = keyof typeof list;

export const serverPages: Readonly<Record<ServerPageId, ServerPage>> = list;
