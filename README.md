# devtab-dev-web

The product site of **DevTab.Dev**, a Chrome extension for comparing and
reviewing code. `devtab-dev` is the project's code name; the site shows the
product's name as its wordmark.

The site is empty on purpose: a home page, a 404, `robots.txt`, a sitemap and
a language switch, in English and Russian. It is an Astro site whose pages are
prerendered into static files, with no client-side JavaScript, no analytics,
no third-party scripts and nothing loaded from another host. Astro's Node
adapter adds a server that serves those files and renders the few pages that
need it (see [The server](#the-server)): the plans page and signing in to an
account of the site by e-mail.

The project has three sibling repositories: `devtab-dev-extension` (the
extension), `devtab-dev-docs` (developer documents, private) and
`devtab-dev-server` (the companion for live review of local changes, not
started). This repository imports nothing from any of them and does not
need them checked out.

The site's backend lives here too, in `api/` (see [Backend](#backend-api)).

## Requirements

- Node 22.23.4 or newer, Astro's minimum (`.nvmrc`, which nvm and fnm read).
- pnpm 12 (`packageManager` in `package.json`).
- [gitleaks](https://github.com/gitleaks/gitleaks), which the git hooks require.

## Set up a clone

```bash
script/install-hooks                 # once per clone: the git hooks, and .git/info/privacy-patterns
git config --local user.name  "…"    # the repository's own identity, before the first commit
git config --local user.email "…"
pnpm install --frozen-lockfile
```

`.npmrc` points every install at the public npm registry
(`registry.npmjs.org`), and `pnpm-workspace.yaml` refuses any version, direct
or transitive, published less than two weeks ago.

## Commands

| Command             | What it does                                                           |
| ------------------- | ---------------------------------------------------------------------- |
| `pnpm dev`          | Development server at http://localhost:4321                            |
| `pnpm build`        | Build: static pages to `dist/client/`, the server to `dist/server/`    |
| `pnpm preview`      | Serves the build                                                       |
| `pnpm start`        | Runs the built server, `dist/server/entry.mjs` (`HOST`, `PORT`)        |
| `pnpm check`        | Everything below, in order: the one command to run before a commit     |
| `pnpm typecheck`    | `astro check`: TypeScript and the templates                            |
| `pnpm lint`         | ESLint                                                                 |
| `pnpm format:check` | Prettier (`pnpm format` writes)                                        |
| `pnpm test`         | `node --test`: the catalogues and templates, then the build in `dist/` |
| `pnpm test:server`  | The server's pages alone, on the built server (part of `pnpm test`)    |

`pnpm test` checks the build, so it runs after `pnpm build` (as `pnpm check`
does). It proves that:

- every locale has every string of the English catalogue, none empty, with
  the same `{placeholders}`;
- no `.astro` template carries user-visible text of its own;
- every page exists in every locale, with the right `<html lang>`, canonical
  URL, `hreflang` alternates and `x-default`;
- the language switch links the same page in every locale and marks the
  current one;
- every built page is listed in `src/lib/pages.ts`, and the sitemap lists
  exactly the localised URLs, each with its alternates;
- internal links and references reach a file of the build;
- the built pages load nothing from another host;
- every page carries the content security policy, and nothing on a page
  breaks it;
- every page the server renders (`src/lib/serverPages.ts`) answers in every
  locale on the built server, `dist/server/entry.mjs`, started against one
  stub (`node:http`) of the API's internal listener (the plans of
  `tests/fixtures/internal-plans.json`) and of Supabase Auth, with the same
  checks of language, canonical URL, alternates, switch and policy, and no
  script; `/pro/` shows what the catalogue says; the forms of every sign-in
  method (the mail's link and code, a password with sign-up and reset,
  Google) ask Supabase Auth what they should, keep `next` to the site, refuse
  a form from another site and count attempts per address, and a method that
  is off is neither shown nor served (`tests/server.test.ts`, with more
  servers for fewer methods);
- the site's code reads only public values (the sign-in methods among them)
  and a rate limit from its environment and never names Supabase's secret key, and neither does the
  build (`tests/no-secrets.test.ts`).

## Layout

```
astro.config.mjs          site address, URL shape, i18n routing, the Node adapter, the content security policy, Tailwind
src/i18n/config.ts        the locales; the first is the default
src/i18n/messages/*.ts    one string catalogue per locale (en.ts is the source)
src/i18n/wordmark.ts      the wordmark every locale shows
src/i18n/validate.ts      the catalogue check (the build and the tests)
src/i18n/index.ts         messages(locale), the routes of a localised page
src/lib/pages.ts          every localised page, by id and path
src/lib/serverPages.ts    every localised page the server renders, by id: path, whether indexed
src/lib/site.ts           the site's address
src/lib/price.ts          a plan's price as the page shows it
src/lib/api/client.ts     the server's calls to the API's internal listener, as the signed-in person
src/lib/auth/             sign-in's settings and methods, the Supabase server client, the pending addresses, attempts
src/middleware.ts         reads the session on the pages that need it
src/env.d.ts              what the middleware gives a page (`locals.auth`)
src/lib/api/schema.gen.ts the internal API's types, written by script/gen
src/layouts/              <html lang>, title, canonical URL, alternates
src/components/           the header, the language and theme switches; auth/: the sign-in pages' card, Google link
src/pages/[...locale]/    the localised pages; pro/ and the sign-in pages are rendered by the server
src/pages/auth/           the sign-in endpoints: sign-in (mail), password, google, callback, signout
src/pages/                404, robots.txt, sitemap.xml
src/styles/tokens.css     the design tokens (placeholders)
tests/                    node:test, from the sources and from the build (dist/client/)
```

## Localisation

- **Strings** live in `src/i18n/messages/<locale>.ts`, one catalogue per
  locale; markup only reads them (`messages(locale).pages.home.heading`).
  English is the source; every other catalogue `satisfies` its shape.
- **Routes.** English, the default, is served at the root (`/`); every other
  locale under its prefix (`/ru/`). URLs are directories with a trailing
  slash. A page is written once, in `src/pages/[...locale]/`, and
  `getStaticPaths` builds it for every locale.
- **The language switch** in the header links the same page in each locale,
  by the language's own name; the current one carries `aria-current="page"`.
  It is a plain link: no script, no cookie, no redirect by browser language.
- **Alternates.** Every page has a canonical URL and `<link rel="alternate">`
  for every locale plus `x-default` (English); the sitemap repeats them.
- **A missing translation** fails three times over: `pnpm typecheck` (the
  catalogue does not satisfy English's shape), `pnpm build` and `pnpm dev`
  (loading the catalogues throws, naming the locale and the key), and
  `pnpm test`.
- The 404 answers in English, the default locale: a static host serves one
  404 for every address. Its switch links each locale's home page.

### Texts awaiting the owner

The texts of the pages the server renders (`pages.pro`, `pages.signIn`,
`pages.signUp`, `pages.forgotPassword`, `pages.resetPassword`,
`pages.checkout`, `pages.thanks`, `pages.trial`), of the API's mails
(`mail/listmonk/`) and of Auth's mails
(`supabase/templates/sign_in.html`, `recovery.html` and `invite.html`, and their
subjects in `supabase/config.toml`) are not the owner's yet: they are the specifications' words where those give any,
otherwise the shortest factual text, and the English catalogue says so. They
stay together under their page's key, so that the owner approves or replaces
them in one place. No marketing copy and no claim about the product beyond
the specifications.

### Add a locale

Two steps; `de` as the example:

1. Copy `src/i18n/messages/en.ts` to `src/i18n/messages/de.ts`, translate the
   strings, and declare the shape as `ru.ts` does
   (`export default { … } satisfies Messages;`).
2. Add it to the list in `src/i18n/config.ts`:
   `export const locales = ['en', 'ru', 'de'] as const;`

The routes (`/de/`), the switch, the alternates and the sitemap follow.

### Add a page

Three steps; an `about` page as the example:

1. `src/pages/[...locale]/about.astro`, like `index.astro`:
   `export const getStaticPaths = localeStaticPaths;` and
   `<BaseLayout locale={locale} page="about">`.
2. `about: 'about/'` in `src/lib/pages.ts`.
3. `pages.about` (at least `title` and `description`) in every catalogue.

## The server

`astro.config.mjs` keeps `output: 'static'` and adds the Node adapter
(`@astrojs/node`, standalone): every page is prerendered as before, into
`dist/client/`, which still works as plain static files; a page that needs
the server says `export const prerender = false`. `dist/server/entry.mjs` is
the server: it serves `dist/client/` and renders those pages.

```bash
pnpm build
HOST=127.0.0.1 PORT=4321 pnpm start   # http://127.0.0.1:4321
```

The server's pages are listed in `src/lib/serverPages.ts` (path, and
whether search engines index the page: the sitemap lists the indexed ones,
the others carry `noindex`), written once in `src/pages/[...locale]/` like
the others, with `export const prerender = false`, and their locale comes
from the address (`localeOfParam`): an unknown prefix is the site's 404. They
call the API's internal listener from the server, never from the browser, at
`DEVTAB_API_INTERNAL_URL` (default `http://127.0.0.1:8081`, the API's
internal listener in development), through `src/lib/api/client.ts`, whose
bodies are typed by `src/lib/api/schema.gen.ts`. The server sends their
content security policy as a header. Only the site's own host and the
loopback addresses are believed in a request's `Host` header
(`security.allowedDomains`): the forms' origin check and the sign-in mail's
link back are built from it.

### The plans page

`/pro/` (and `/ru/pro/`) shows the cards of `GET /internal/plans` in the
page's language: name, badge, price and period, summary, price note, what a
plan includes. The plans' texts are data (`config/plans.toml`, README,
"Plans"), and a plan without texts in the page's language shows its English
ones, marked with their `lang`. A paid plan that can be bought now links to
`/pro/checkout/?plan=<id>`; one that cannot says "Soon", and while no paid
plan can be bought the page says that purchases are not open yet. Nothing on
the page depends on the visitor, so it carries `Cache-Control: public,
max-age=300`; when the API cannot be reached it answers 503 with
`no-store`. With the repository's catalogue no plan is available, so every
paid plan says "Soon"; the fake provider's XTS prices show only in `dev`.

### The free trial

Where the deployment offers a trial (`trialDays` of `GET /internal/plans`;
the API's `DEVTAB_TRIAL_DAYS`), `/pro/` says how many days it lasts and
links to `/pro/trial/`. Signed out, that page links to signing in and back
(a trial belongs to an account, one per inbox); signed in, one form starts
it, posting to the page itself (origin checked, counted with the sign-in
attempts), and the answer shows its end, its key once and the step to
connect the extension. Without a trial the page says none is offered.

### Buying

`/pro/checkout/?plan=<id>` buys a plan that is on sale, with or without an
account. Signed in, one form buys for the account. Signed out, where the
deployment sells without an account (`DEVTAB_GUEST_CHECKOUT=true`, on the
site and on the API), a form buys without one, next to a link to sign in
first and come back; without the setting, only that link (and the guest form
answers 404). A team plan asks for its size (from its smallest team to
1000; the provider's page lets the buyer change it). The form posts to the
page itself (Astro's origin check and the `Origin` again, counted with the
sign-in attempts of the client address, `DEVTAB_RATE_SIGN_IN_PER_IP`), and
the API starts the checkout ("The purchase", below). The answer is the same
page saying "on to the payment", which moves on to the provider's page by a
`<meta http-equiv="refresh">` and a link: a form's redirect to another host
would need that host in the policy's `form-action`, which names no other
host. The site shows only the provider's hosted page; the embedded form
would need Stripe.js.

A guest checkout's answer carries a secret, which the site keeps in an
`HttpOnly`, `SameSite=Lax` cookie of that browser (`devtab-checkout`, a day,
with the checkout's id), never in an address or a page.

`/pro/thanks/?ref=<checkout>` is where the provider returns the buyer. It
asks the API as the signed-in person when the checkout is theirs, otherwise
as the browser holding the guest checkout's secret, and shows: "being
confirmed" until the provider's event has been processed (reloading asks
again); the key the first time anyone asks, this browser or the account,
then its last four characters; for the account's owner, the step to connect
the extension (`/pro/connect/`); for a guest, either "check your mail to
open your account" (a new account was invited) or "sign in with the address
you paid with" (the address had one). Both pages are personal: never cached,
never indexed.

### Signing in

`/sign-in/` (and `/ru/sign-in/`) signs a person in to an account of the site,
which Supabase Auth keeps. The methods are the owner's choice; which of
them a deployment offers is its setting, `DEVTAB_SIGN_IN_METHODS`
(comma-separated; unset: `email`):

| Method     | What the person does                                                      | Pages and endpoints                                                                       |
| ---------- | ------------------------------------------------------------------------- | ----------------------------------------------------------------------------------------- |
| `email`    | gets a mail with a sign-in link and a one-time code                       | `/sign-in/`; `POST /auth/sign-in/`                                                        |
| `password` | signs in with an address and a password; signs up; resets a lost password | `/sign-in/`, `/sign-up/`, `/forgot-password/`, `/reset-password/`; `POST /auth/password/` |
| `google`   | continues with a Google account                                           | a link on `/sign-in/` and `/sign-up/`; `GET /auth/google/`                                |

A method left out is not shown, and its pages and endpoints answer 404; with
none that the site knows, the page says that sign-in is not available. No
captcha. Everything works without client-side JavaScript: plain forms post
to the endpoints, which call Supabase Auth from the server and answer 303,
an error coming back as `?error=<code>`, which the page words.

1. The e-mail form: Supabase Auth sends one mail
   (`supabase/templates/sign_in.html`) with a link and a one-time code. The
   address waits for the code in an `HttpOnly` cookie of the site
   (`devtab-sign-in`, an hour).
2. The link works in the browser that asked (its PKCE verifier is a cookie of
   that browser) and comes back to `/auth/callback/`, which exchanges its code
   for the session; opened elsewhere it comes back with "enter the code
   instead". The code works anywhere: the code form checks it.
3. A password: signing up asks Supabase Auth for the account, which sends
   the same mail to confirm the address (its link back to `/auth/callback/`,
   its code on the sign-in page, as above); until then the password does not
   sign in. A forgotten password: `/forgot-password/` sends a reset mail
   (`supabase/templates/recovery.html`) with a link back to
   `/auth/callback/?via=recovery` and a code that page takes (its address
   waits in `devtab-reset-address`). Either one starts a session and marks
   it, by its id, in `devtab-reset` (an hour): only that session may set the
   new password on `/reset-password/`; any other sees "ask for a new
   e-mail". Passwords are checked by Supabase Auth (its length and
   character rules); the site asks for the new one twice and never logs it.
4. The invite of a purchase made without an account
   (`supabase/templates/invite.html`, "Buying" above, "The purchase" below)
   links to `/auth/callback/?token_hash=…&type=invite` on the project's Site
   URL. The callback verifies the hash (`verifyOtp`, no PKCE: the API asked
   for the invite, not this browser), which confirms the address and starts
   the session, and then decides where to go from the account's metadata:
   the purchase's `/pro/thanks/`, in the purchase's language. The link names
   no landing. The invited person has no password: where the deployment
   offers passwords, the thanks page offers setting one, in the session the
   invite started (the `devtab-reset` mark, as after a reset mail), and
   nothing asks for it. A used or expired link comes back to the sign-in
   page with `?error=invite`.
5. Google: a link (not a form: the policy's `form-action 'self'` would stop a
   form's redirect to Google) to `/auth/google/`, which asks Supabase Auth
   for the provider's address with a PKCE verifier in a cookie and answers
   303 there; Google comes back through Supabase Auth to
   `/auth/callback/?via=google`, with the page's language and `next`. It
   starts only from the site's own pages (`Sec-Fetch-Site`). No script, font
   or image comes from Google: the button is text and an outline of the
   letter.
6. The session lives in cookies that `@supabase/ssr` manages, `HttpOnly` and
   `SameSite=Lax` (no script reads them). `src/middleware.ts` reads it only
   on the pages that need it (those of `serverPages` marked `session`, and
   `/auth/*`), verifying the token with `getClaims()` against the project's
   keys and refreshing it; the cacheable pages never touch it, so no
   session cookie is set on a response a cache may keep. A page reads the
   person as `locals.auth`, and `src/lib/api/client.ts` passes their access
   token to the API as `Authorization: Bearer …`.
7. Signed in, the page shows who and offers signing out
   (`POST /auth/signout/`). `?next=<path of this site>` sends a signed-in
   person there, and the forms and links carry it through every method;
   anything but a path of this site is dropped. The account's own page comes
   later.

Astro's origin check refuses a form posted from another site. Each client
address may try 10 times an hour, all methods together
(`DEVTAB_RATE_SIGN_IN_PER_IP`, counted in the server's memory, before
Supabase Auth's own limits). The pages are never cached and never indexed.
The site holds no secret: only the project's address and its publishable
key, which anyone may see.

#### What a deployment sets in Supabase

The site only chooses among the methods; Supabase Auth must allow the ones
it offers. Locally `supabase/config.toml` does; a hosted project sets the
same in its dashboard (Authentication), and nothing of it is in the site's
environment:

- **Redirect URLs**: the site's `https://<site>/auth/callback/**`, so that
  the mails' links and Google come back to it.
- **E-mail** (`email`, `password`): sign-ups on, "Confirm email" on (a
  password signs in only once its address is confirmed), the code's length
  6, and the four mail templates of `supabase/templates/` (magic link and
  confirmation: `sign_in.html`; reset password: `recovery.html`; invite user:
  `invite.html`) with their subjects. The Site URL is the site's address: the
  invite mail's button is built on it.
- **Buying without an account** (`DEVTAB_GUEST_CHECKOUT=true`): the API holds
  the project's secret key (`sb_secret_…`, or the legacy `service_role` key)
  in a file (`DEVTAB_SUPABASE_SECRET_KEY_FILE`), with which it invites the
  buyers who have no account. The site never holds it.
- **Sender**: Auth's mails go out through the project's SMTP server, which
  says who sends them, its address and name: locally `[auth.email.smtp]`,
  off (every mail goes to the mail catcher) unless
  `SUPABASE_AUTH_EMAIL_SMTP_ENABLED=true` and the server's
  `SUPABASE_AUTH_EMAIL_SMTP_HOST`, `_PORT`, `_USER`, `_PASS`, the sender's
  address `_ADMIN_EMAIL` (for example `no-reply@example.invalid`) and name
  `_SENDER_NAME` are in the environment; hosted, Authentication, SMTP.
- **Google** (`google`): the Google provider on, with the client id and
  secret of an OAuth client of a Google Cloud project, whose authorised
  redirect URI is the project's `<Supabase URL>/auth/v1/callback`. Locally
  `[auth.external.google]`, off unless
  `SUPABASE_AUTH_EXTERNAL_GOOGLE_ENABLED=true`,
  `SUPABASE_AUTH_EXTERNAL_GOOGLE_CLIENT_ID` and
  `SUPABASE_AUTH_EXTERNAL_GOOGLE_SECRET` are in the environment of
  `script/auth-up`; hosted, Authentication, Providers. The client id and
  secret are never in this repository or the site's environment.

### Run it locally

The site's server against the API, without sign-in:

```bash
script/db-up && script/api-dev      # the API, its internal listener on 127.0.0.1:8081
pnpm build && script/site-server    # http://127.0.0.1:4321
```

With sign-in, the full local stack: `script/auth-up` starts Postgres, Auth,
the gateway (http://127.0.0.1:54332) and the mail catcher, Mailpit
(http://127.0.0.1:54334), where every sign-in mail lands; nothing leaves the
machine. `script/site-server` then reads the stack's publishable key from
`supabase status` and passes it on; nothing writes it into a file.

```bash
script/auth-up                      # the stack's containers; the first start pulls two images
script/api-dev                      # checks access tokens against the stack's JWKS
pnpm build && script/site-server    # http://127.0.0.1:4321/sign-in/; mails at http://127.0.0.1:54334
node script/e2e-sign-in.ts          # sign in by the code, the link and a password, end to end
supabase --workdir . stop           # stop the stack when done: its ports are on every interface
```

`script/site-server` offers the e-mail and passwords
(`DEVTAB_SIGN_IN_METHODS=email,password` unless set), which the local stack
serves on its own; Google needs a client (above).
`script/e2e-sign-in.ts` starts and stops nothing: it posts the forms, reads
the mail from Mailpit, signs in by its code, checks that the API accepts the
session's access token (and refuses none or a forged one), signs out, then
signs in by the link in the same browser and shows that the link fails in
another. When the site offers passwords it then signs up with one, confirms
the address by the mail's code, signs in with the password, and resets it
by the code and by the link of a reset mail. It prints tokens and codes
masked and leaves its three `…@example.invalid` accounts in the local
database.

### The site's configuration

The server reads it when it runs; every value is public.

| Variable                          | Default                 | Note                                                           |
| --------------------------------- | ----------------------- | -------------------------------------------------------------- |
| `HOST`, `PORT`                    | `localhost`, `4321`     | where the server listens (`@astrojs/node`)                     |
| `DEVTAB_API_INTERNAL_URL`         | `http://127.0.0.1:8081` | the API's internal listener                                    |
| `DEVTAB_SUPABASE_URL`             | none                    | the Supabase project, as the browser reaches it; with the key  |
| `DEVTAB_SUPABASE_PUBLISHABLE_KEY` | none                    | its publishable key; without both, sign-in is off              |
| `DEVTAB_RATE_SIGN_IN_PER_IP`      | `10`                    | sign-in attempts an hour, by client address                    |
| `DEVTAB_SIGN_IN_METHODS`          | `email`                 | the sign-in methods offered: `email`, `password`, `google`     |
| `DEVTAB_EXTENSION_IDS`            | none                    | the extension builds `/pro/connect/` talks to, comma-separated |
| `DEVTAB_GUEST_CHECKOUT`           | none (off)              | `true`: anyone may buy without an account (the API's too)      |

## Content security policy

Every page carries a content security policy (`security.csp` in
`astro.config.mjs`), which Astro writes into a `<meta http-equiv>` tag:
`default-src 'self'`, `form-action 'self'`, `object-src 'none'`,
`base-uri 'self'`, and for scripts and styles `'self'` plus the hashes of
Astro's own. Nothing loads from another host and forms post only to the site.
A `style` attribute or a page's own script in its markup would be blocked:
style with Tailwind classes. `pnpm test` checks every page's policy.

## Styling

Tailwind CSS v4 through its Vite plugin (`src/styles/global.css`). Colours,
radius and type faces come from one file, `src/styles/tokens.css`, under
shadcn/ui's names, with neutral placeholder values. The extension's style
package is meant to replace that file later; components name tokens through
utilities (`bg-background`, `text-muted-foreground`) and never a colour
value. Type is the platform's own: no web fonts.

## Site address

`https://devtab.dev`, in `src/lib/site.ts`: canonical URLs, alternates,
`robots.txt` and the sitemap use it. `SITE_URL=https://… pnpm build` builds
for another address; run the tests with the same `SITE_URL`.

## Backend (`api/`)

`devtab-api` is the site's backend: one Rust binary (axum, tokio, sqlx) with
two listeners. The public one (`:8080`) answers `/api/healthz`,
`/api/readyz` and the extension's endpoints: the licence's `POST /v1/activate`,
`/v1/refresh` and `/v1/deactivate`, and the plans' catalogue, `GET /v1/plans`;
the internal one (`127.0.0.1:8081` in development) answers `/internal/healthz`,
`/internal/plans` and `/internal/me` and is meant only for the site's own
pages, never published. It reads its configuration from `DEVTAB_*` variables
and keeps its data in Postgres 17, schema `app`, as the role `devtab_api`.
The site's server calls the internal listener for the pages it renders
(`/pro/`), as the signed-in person when there is one.

The backend stands on [Supabase](https://supabase.com): its Postgres holds the
API's data, and Supabase Auth holds the users, so both are managed the same
way. Locally that is the Supabase CLI's stack in Docker
(`supabase/config.toml`). The API connects to Postgres directly, as
`devtab_api`; schema `app` is not exposed through Supabase's Data API, and
its roles (`anon`, `authenticated`, `service_role`) have no privilege in it.
No hosted Supabase project is linked to this repository yet.

Every error answers
`{"error":"<kebab-case code>","message":"<English text>"}`.

### Licences

A user is a user of Supabase Auth: a trigger on `auth.users` gives each one a
row in `app.users`. A licence belongs to a user (or, issued free of charge, to
nobody), has a plan, a number of seats and a key; the extension activates an
installation with the key and keeps it going with the installation's secret
and the signed token the server returns.

- **Kinds**: `subscription` (from a purchase), `complimentary` (`admin
licence comp`), `trial` ("The trial"), `one_time` (not sold yet).
- **Key**: `DTD-XXXX-XXXX-XXXX-XXXX-XXXX`, 100 random bits in Crockford base32,
  shown once and stored only as the SHA-256 of its canonical form (upper
  case, no spaces or hyphens, `O` read as `0`, `I` and `L` as `1`). A licence
  id is `lic_` and 26 characters of 16 random bytes; an installation's secret
  is 32 random bytes in base64url, stored as its SHA-256.
- **Token**: a compact JWS signed with ES256 (deterministic ECDSA), header
  `{"alg":"ES256","kid":…,"typ":"devtab-licence"}`. Its dates follow one rule:
  with `end` the end of what was paid (plus 14 days for a subscription's
  payment retries), `exp = min(iat + 30 d, end)`, `chk = min(iat + 7 d, exp)`,
  `warn = max(exp − 7 d, chk)`; past `end` the token says `ended`.
- **Seats**: activating takes a seat while the licence has one free and fewer
  than 3 × seats new activations in 30 days; activating the same installation
  again only replaces its secret. An installation not seen for 60 days loses
  its seat (the scheduler, hourly, under a Postgres advisory lock) and gets
  it back at its next check if one is free.
- **Signing keys**: the public keys are `config/signing-keys.json`, built into
  the binary. The private key is a file on the server
  (`DEVTAB_SIGNING_KEY_FILE`, id `DEVTAB_SIGNING_KID`), never in the
  repository, the environment, the database or CI; `serve` refuses to start
  unless it is the trusted key of that id and the key's `sign_from` has come.
  No private key is in the repository, not even a test one, and the
  published list stays empty until production keys exist. In `dev` the API
  also trusts a developer's own test keys, listed in the file
  `DEVTAB_DEV_SIGNING_KEYS_FILE`: `script/api-dev` makes one on first use
  (`admin keys generate --test`) in the ignored `.state/signing/`, the
  private key with mode 600. `staging` and `prod` refuse that setting and
  any `test-` key. Tests make their keys in memory.

### Plans

The plans' catalogue is data: `config/plans.toml` holds the plans (id, kind,
billing period, seats, order, whether available, the end of a limited offer),
the texts of each plan's card (English required, Russian optional) and each
plan's prices as a provider and its price id or lookup key. `devtab-api admin plans sync`
makes `app.plans`, `app.plan_texts` and `app.plan_prices` what the file says
for the plans it lists, and writes the catalogue's version (`<date>.<n>`) into
`app.settings`:

- It holds every text to the extension's rules, so that the server never
  sends what the extension would skip: lengths in Unicode code points after
  trimming (name 1–32, summary 140, 0–8 lines of what a plan includes of
  1–100 each, price note 60, badge 24), no control or text-direction
  characters, at most six available plans (counting those not in the file),
  English texts for every plan, and an answer within 32 KB.
- Amounts are never typed in: the sync reads each price's id, amount and
  currency from its provider (`PaymentProvider::resolve`, `api/src/payments/`)
  and stores them; a recurring price must bill as often as its plan. `fake`,
  for development and tests, has its prices stated in the file next to their
  ids; Stripe's are named by lookup key (or id) and read from Stripe
  ("Payments (Stripe)" below). The endpoints only read the stored rows.
- A real price uses a currency code of ISO 4217 that browsers know
  (`api/src/plans/currency.rs`); XTS, the code ISO 4217 reserves for tests,
  belongs to the fake provider alone and is served only in `dev`, as only the
  extension's test and development builds accept it. `DEVTAB_PAYMENTS=fake`
  is refused in `prod`; in `staging` its prices are not served, so no paid plan
  is offered.
- It prints the changes and asks first; texts of a language the file no longer
  has are deleted, prices it no longer has are deactivated (kept for the
  purchases that will refer to them). One `app.admin_audit` row per applied
  sync.

`GET /v1/plans` answers the catalogue in the format the extension reads (its
plan model, format `v` 1): the version, `maxAge` (`DEVTAB_PLANS_MAX_AGE`), the
site's manage address, and at most six plans in the order of their cards —
those that are available, whose offer has not ended and that can be sold now
(a paid plan needs an active price of the provider `DEVTAB_PAYMENTS` names, in
a currency this environment serves). English texts only; nothing about the
person asking, no seats. Any origin may read it (`Access-Control-Allow-Origin:
*`, never credentials, errors included; a preflight gets 204); it carries
`Cache-Control: no-cache` and an `ETag`, and answers 304 to a matching
`If-None-Match`. It answers only on `DEVTAB_LICENCE_HOST`, closes for
maintenance like the other `/v1/*`, and allows 60 requests a minute per client
address (`DEVTAB_RATE_PLANS_PER_IP`). With nothing available, its list is
empty: the extension reads that as no usable answer (`empty`) and keeps the
list it had.

`GET /internal/plans?locale=en|ru` answers the same for the site's pages, with
the texts of that language (English where a plan has none, as each plan's
`locale` says), every plan whose offer has not ended, and whether each can be
bought now (`available`).

What `config/plans.toml` holds now: Free, Monthly and Annually (`free`,
`pro-monthly`, `pro-annual`, three seats each) with the placeholder texts of
the extension's plans specification, the fake provider's prices XTS 11.11 and
XTS 111.10, and no plan available: their final wording and prices are not
decided, so `GET /v1/plans` answers an empty list.

### Signed-in people

The API keeps no session: a route of the internal listener that needs a
signed-in person takes the `CurrentUser` extractor (`api/src/auth/`), which
reads `Authorization: Bearer <Supabase access token>` and checks it there,
against the project's JWKS (`<DEVTAB_SUPABASE_URL>/auth/v1/.well-known/jwks.json`):

- a key of the set signed it, with ES256 or RS256 and the key's own
  algorithm; a shared HS256 secret is never accepted, so the project signs
  with an asymmetric key (locally an ES256 key of this machine, below);
- `iss` is the project's (`<DEVTAB_SUPABASE_URL>/auth/v1`, or
  `DEVTAB_JWT_ISSUER`), `aud` is `authenticated`, `exp` has not passed by the
  API's clock (30 seconds of leeway), `sub` is a UUID, the session is not
  anonymous;
- the set is kept for five minutes; a token with a `kid` the set lacks fetches
  it again, at most every 30 seconds; when it cannot be fetched the keys
  already kept still work, and with none that matches the answer is 503, not
  401;
- the person is the token's `sub` and nothing else (not an id from the body,
  the query or a header, not a role from the token), and must have an
  `app.users` row (the trigger on `auth.users` makes it), read again every
  minute: none is 401, `blocked` is 403.

No header, or a token that does not pass: 401 `unauthorized` with
`WWW-Authenticate: Bearer`. Without `DEVTAB_SUPABASE_URL` (allowed in `dev`
only) these routes answer 503. So far one route needs a person:
`GET /internal/me`, `{"user": "<uuid>", "email": "<the account's address>"}`,
never cached; the account pages of a later stage add to it.

### Payments (Stripe)

The payment provider is chosen when deploying: `DEVTAB_PAYMENTS=stripe` with
the settings under "Configuration" (`deploy/api.env.example` lists them all).
Nothing about an account is in the code: no key, price, product, webhook
address or host of anyone. What differs between deployments is a setting, and
each secret is a file the setting names.

- **Mode.** `DEVTAB_PAYMENTS_ENV` is `sandbox` or `live` (prod: live only);
  the key must be of that mode, and a webhook event of the other mode is
  refused.
- **Seller.** With Managed Payments (`DEVTAB_STRIPE_MANAGED_PAYMENTS=true`,
  the default) Stripe sells as merchant of record and handles the tax; the
  checkout passes `managed_payments[enabled]` and nothing about tax. With
  `false` the account sells, and `DEVTAB_STRIPE_AUTOMATIC_TAX=true` turns on
  Stripe Tax for the checkout.
- **Checkout.** A subscription Checkout Session for one price of the stored
  catalogue, tied to our checkout by `client_reference_id` and metadata on the
  session and its subscription, with an idempotency key. `hosted` (the
  default) sends the buyer to Stripe's page; `embedded` answers the session's
  client secret, the publishable key (`DEVTAB_STRIPE_PUBLISHABLE_KEY`) and the
  API version for Stripe.js, so the site needs no Stripe setting of its own.
  The purchase pages will have to widen the content security policy for
  Stripe: `form-action` for its checkout and portal hosts when a form posts
  there, and Stripe.js and its frames for the embedded form.
- **Webhooks.** `POST /api/webhooks/stripe` on the public listener (the
  site's host routes `/api/*` there; bodies up to 256 KB). The signature is
  checked on the raw body (`api/src/payments/stripe/webhook.rs`: `t` and every
  `v1`, HMAC-SHA256 with the destination's secret, the tolerance, the mode); a
  mismatch is 400 `signature`. A valid event is stored once in
  `app.provider_events` (a repeated delivery is answered 200 at once), a
  `process_event` job is queued in the same transaction, and the answer is 200
  before any processing.
- **API.** REST over rustls with the Mozilla roots built in, the version
  pinned by `DEVTAB_STRIPE_API_VERSION` (default `2026-11-18.endive`, nothing
  older than `2026-09-30.endive`), retries on 409, 429 and 5xx as Stripe
  advises, at most 1 MB read per answer.

What the operator creates at Stripe, once per mode (the sandbox, then live):

1. For Managed Payments: the application in the Dashboard (Settings › Managed
   Payments) and the product's eligible tax code.
2. A product with two recurring prices, monthly and yearly, each with a lookup
   key; the plans file names those keys (`config/plans.stripe.example.toml`).
3. The customer portal's default configuration: switching between the two
   prices, cancelling at the period's end, updating the payment method,
   invoice history.
4. A webhook destination at `<DEVTAB_SITE_URL>/api/webhooks/stripe` with
   snapshot events of the API version above: `checkout.session.completed`,
   `checkout.session.async_payment_succeeded`,
   `checkout.session.async_payment_failed`, `customer.subscription.created`,
   `.updated`, `.deleted`, `.paused`, `.resumed`, `invoice.paid`,
   `invoice.payment_failed`, `invoice.payment_action_required`,
   `charge.refunded`, `charge.dispute.created`, `charge.dispute.closed`. Its
   signing secret goes into the file `DEVTAB_PAYMENTS_WEBHOOK_SECRET_FILE`
   names.
5. A restricted key with write on Checkout Sessions, Customer portal,
   Subscriptions and Subscription schedules, and read on Prices, Invoices,
   Invoice payments, Charges, Disputes and Events; nothing else. It goes into
   the file `DEVTAB_PAYMENTS_API_KEY_FILE` names.

Then `devtab-api admin stripe check --file <plans file>` checks the settings
and both secrets as `serve` does and finds every Stripe price of the file;
`admin plans sync --file <plans file>` stores them. `serve` does not start
with a missing secret, a secret key, or a key of the other mode.

### The purchase

A purchase is made signed in or, where the deployment allows it, without an
account (D35 of the specifications). A signed-in person starts a checkout of
a plan on sale (`POST /internal/checkouts {"plan", "locale", "members"}`: a
row in `app.checkouts`, then the provider's session, naming the account's
customer if it has one; the answer is the provider's page or its embedded
form; 404 `plan-unknown`, 409 `plan-unavailable`, 503
`payments-unavailable`). The provider returns the buyer to
`<DEVTAB_SITE_URL>[/<locale>]/pro/thanks/?ref=<checkout>`, which reads
`GET /internal/checkouts/{id}`: `open`, `expired`, or `completed` with the
key the first time it is asked for within a day, and only to the person who
started it (anyone else gets 404); `key_hint` after that.

**Without an account** (a guest checkout; `DEVTAB_GUEST_CHECKOUT=true`):
`POST /internal/checkouts/guest` takes the same body and starts the same
checkout with no user and no customer, so Stripe asks the buyer for the
address and makes a customer (a subscription checkout always does). Its
answer adds `claim`, a secret for the browser that started it, of which the
checkout keeps only the SHA-256; `POST /internal/checkouts/{id}/claim
{"claim"}` answers the checkout as above, plus `account`: `invited` or
`existing` once paid. With the setting off the route answers 403
`guest-checkout-off`; on, but without the Supabase secret key, 503
`guest-checkout-unavailable`; a signed-in purchase works either way. The
site limits how often a client starts one.

When a guest checkout's subscription is paid, the worker gives it a buyer
before it issues the licence, inside the subscription's lock and the job's
transaction:

1. the address the buyer typed at Stripe, read from the Checkout Session
   (`customer_details.email`; the restricted key's write permission on
   Checkout Sessions reads it, no permission on Customers is needed);
2. the account with that exact address (trimmed, lower case), through
   `app.user_id_by_email`, the security definer function the API role
   already has; else the first account of the same canonical inbox
   (`app.users.email_canonical`, below);
3. only if there is none, an invite: `POST <Supabase Auth>/invite` with the
   project's secret key (`DEVTAB_SUPABASE_SECRET_KEY_FILE`; Auth's address is
   `<DEVTAB_SUPABASE_URL>/auth/v1` unless `DEVTAB_SUPABASE_AUTH_URL` names
   another), the address as typed, user metadata `{locale, app: "devtab",
source: "purchase", checkout, team}` and `redirect_to` the site's
   callback. Auth creates the user (the trigger makes its `app.users` row)
   and sends `supabase/templates/invite.html`.

Supabase Auth invites an unconfirmed address again each time it is asked, so
the lookups always come first, under an advisory lock on the inbox: a job
tried again after a failure (Auth down, an answer lost) finds the user the
first attempt made and invites nobody twice. The licence, the checkout
(`user_id`, `buyer`) and the customer then belong to that user, and so does
a team. A buyer who had an account gets no invite but a `licence-added`
mail; every other buyer gets the `welcome` mail ("Mail", below). A guest purchase cannot name the
account's customer, so a person may be the provider's customer more than
once. Without the secret key, a paid guest checkout waits (its job retries).

**One inbox, many spellings.** The trial without a card is once per inbox
("The trial", below), so the API knows each user's canonical inbox:
`app.canonical_email` (and `accounts::canonical_email` in the API, held to it
by a test) trims and lower-cases the address, drops a `+tag` on every domain,
and on gmail.com and googlemail.com drops the dots and makes the domain
gmail.com. A trigger on `auth.users` keeps `app.users.email_canonical`; the
account's own address stays as the person typed it, since they sign in with
it and mail goes to it.

Nothing changes a licence but a verified event. The queue's worker
(`api/src/jobs/worker.rs`) runs `process_event` (`api/src/payments/process.rs`):
for each subscription the event names, it reads the subscription's state at
the provider, never the event's body, and `reconcile`
(`api/src/payments/reconcile.rs`) decides:

- a licence is issued only from one of our checkouts, when the subscription is
  active and its current period paid, to the person who started the checkout:
  the provider's customer is tied to that account (a customer of another
  account, or a second one of the same, stops it), the key is kept in
  `app.key_reveals` for a day to be shown once, the checkout is completed;
- `paid_until` moves to the period's end only while the period's invoice is
  paid; the first overdue state sets `past_due_since`, a paid period clears
  it; cancelling at the period's end keeps the licence active; an ended or
  paused subscription ends it, and resumed and paid makes it active again;
- a full refund or a lost dispute revokes it (`refund`); a partial refund or
  an open dispute changes nothing; a revoked licence stays revoked;
- a price no plan has, or a paid subscription with no checkout of ours, stops
  the event for someone to look at.

Each subscription is handled under a transaction-scoped advisory lock, so two
events of one purchase processed at once issue one licence. A failed job is
tried again after 1, 5 and 30 minutes, 2, 6 and 24 hours, then given up and
logged as an error with its event marked `failed`; an error that retrying
cannot help is given up at once. A purchase ends the buyer's running trial.
Still to come: the daily reconciliations, changing plans and the customer
portal.

### The trial

A trial without a card (D36): where `DEVTAB_TRIAL_DAYS` is set (7 to 14;
`serve` refuses any other value), a signed-in person starts one with
`POST /internal/trials {"locale"}` and gets its key once; `GET
/internal/plans` gives `trialDays` (null without a trial), and the site's
`/pro/trial/` starts it. Signed in only, so the address is confirmed.

- It is an ordinary licence of kind `trial`: the seats of
  `DEVTAB_TRIAL_PLAN`, a key, and its end in `paid_until`. The token reads it
  like a complimentary licence with an end (`paid` is the end, no `term`, so
  no payment grace; `renews: false`), so the extension activates, refreshes
  and expires it as any licence: past its end the token says `ended`,
  activation answers `licence-inactive`, connecting from the account skips
  it.
- Once per canonical inbox: `app.trial_inboxes` keeps the SHA-256 of
  `app.users.email_canonical` (never the address) and outlives the account,
  so another spelling of the same inbox, or a new account after deleting the
  old one, gets 409 `trial-used`. Someone who ever had a paid licence gets 409
  `trial-not-eligible`. Off: 403 `trial-off`.
- A purchase ends a running trial (its installations' next check says
  `ended`, and connecting from the account picks the paid licence).
- Its mails: `trial-started` at once, `trial-ending` the days of
  `DEVTAB_MAIL_TRIAL_REMIND_DAYS` before its end (3 and 1), `trial-ended` at
  its end; each is skipped if the trial is over or replaced by then.

### Mail

The API's own mails go through one interface (`api/src/mail/`, D37): a mail
of a kind, to an address, in a language, with the data its template needs.
Supabase Auth's mails (sign-in, password reset, the invite) stay Supabase's.

- **Queued, never sent inline.** What happens queues a `mail` job
  (`mail::queue`): the kind, the user or team member it goes to by id (never
  an address), the language and record ids, due when it should go, with a
  key that makes it once only (one mail per licence, kind and period, however
  many events ask).
- **Checked when due.** The worker reads the state again (`mail::compose`):
  a mail whose reason has gone is skipped (no trial reminder after a
  purchase, no "payment failed" once paid, no renewal reminder for a moved
  period or a cancelled subscription), the address is read now, and the data
  are built (`site_url`, `plans_url`, `connect_url`, `sign_in_url`,
  `plan_name` in the mail's language, `key_hint`, `members`, `ends_on`,
  `days_left`). A provider's refusal gives the mail up at once; an outage
  retries it on the queue's schedule.
- **The kinds.** `trial-started`, `trial-ending`, `trial-ended` ("The
  trial"); `welcome` after a paid licence and `licence-added` for a guest
  buyer whose address had an account; `renewal-reminder`
  `DEVTAB_MAIL_RENEWAL_REMIND_DAYS` (7) before a yearly renewal;
  `payment-failed`, `subscription-cancelled` and `subscription-ended` when a
  subscription becomes overdue, stops renewing or ends; `team-invite` when an
  owner invites an address.
- **Providers**, chosen by `DEVTAB_MAIL`: unset or `none`, mail jobs wait in
  the queue and nothing else changes; `log` logs them (kind and language,
  never the address; refused in `prod`); `listmonk` sends them through
  listmonk's transactional API (`POST /api/tx`) with the template id of each
  kind and language (`DEVTAB_MAIL_LISTMONK_TEMPLATES`), HTTP basic auth with
  an API user and its token (a file), and the sender `DEVTAB_MAIL_FROM`. A
  kind without a template is refused when it is sent, and `serve` warns of
  the missing ones at start.
- **Who receives a listmonk mail.** listmonk's default mode sends only to
  its subscribers. With `DEVTAB_MAIL_LISTMONK_LIST_ID` the recipient first
  joins that (private) list, preconfirmed, and an existing subscriber is left
  as it is; without it the mail goes in `external` mode, which needs no
  subscriber but a listmonk that knows the mode (v6.2.0 does; v5.1.0 ignores
  it and refuses the address). Templates read only `{{ .Tx.Data.* }}`.
- **Templates.** `mail/listmonk/` holds a skeleton of every kind in English
  and Russian (`<kind>.<locale>.html`, subjects in `templates.toml`), whose
  texts await the owner, built into the binary. `devtab-api admin mail
templates` says what it would create or update in the instance;
  `--apply` creates or updates them by name (`DevTab.Dev: <kind>
(<locale>)`) and prints `DEVTAB_MAIL_LISTMONK_TEMPLATES=…` for the deploy
  configuration.

### Connecting the extension

The licence key stays the way in that always works; the usual way is the
extension's "Connect with your account" button. It opens `/pro/connect/`
(signed out: the sign-in page first, and back). The page's script asks the
extension, through `externally_connectable` (only the site the extension's
build names may talk to it, and no install warning), for its installation id;
`POST /auth/connect/` (the site's pages only, a signed-in person only) asks
`POST /internal/licences/connect` for an activation of the person's licence
for that installation: the same seat, secret and token as `POST /v1/activate`
with a key, the licence found by the account: the one this installation is
already on, else the longest-running usable one with a free seat (the person
never chooses; `no-licence`, or `seat-limit` when all are full). The page
hands the answer to the extension, which keeps it only when the token is
signed, active, for its own installation and the licence named. The
installation id comes from the extension, never from the address, so no other
site can make a page take a seat for an installation of its choosing.
`DEVTAB_EXTENSION_IDS` lists the extension builds the page talks to.

### Teams

A team plan (`min_members` in the plans file) is sold per member, on the site
only: `GET /v1/plans` leaves it out, so the extension's cards and contract stay
as they are; `GET /internal/plans` gives its `min_members`. The checkout takes
`members` (at least `min_members`, up to 1000) and lets the buyer change the
number on Stripe's page; later the owner changes it in Stripe's portal. The
licence keeps the number (`members`) and gets the plan's seats for each
member, and every subscription event brings both in line. A team plan's price
may be volume-tiered at Stripe (every member pays the step the whole team
reaches): `plans sync` keeps the steps, `GET /internal/plans` gives them as
`tiers`, and a tiered price on a personal plan is refused.

The buyer owns the team and is its first member. On
`/internal/licences/{lid}/members` they list, invite (`POST {"email"}`, up to
`members` − 1 addresses; `team-full` beyond, `not-a-team` for a personal
licence) and remove (`DELETE …/{email}`) members. An invited person signs in
with the address and connects from their account, as the owner does; the
browsers a member connected so are disconnected when they are removed. The
team's key works too, from the same seats.

### The internal API's types

The internal listener's routes and their bodies are described with `utoipa`
in the Rust code. `script/gen` writes that description,
`contract/internal-openapi.json` (`devtab-api openapi`), and the site's types
from it, `src/lib/api/schema.gen.ts` (`openapi-typescript`, a development
dependency of the site); both are committed and written by nothing else.
`script/gen --check`, in `script/check-all` and CI, fails when either is not
what the code gives. The site's API client (`src/lib/api/client.ts`) takes
its types from it; Prettier, ESLint and Tailwind skip the file.

### The contract with the extension

`contract/licence-v1/` is the machine-readable contract of the licence
endpoints: JSON Schemas of the request and answer bodies and of the token's
header and claims, `errors.json` (codes, HTTP statuses, which endpoint answers
which), `production-keys.json` (empty until production keys exist) and
`server-vectors.json`: tokens this server signs, one per case of the issuing
rules and per revoke reason, plus broken ones, each with what the extension's
`verifyToken` and `evaluate` must say. They are signed by a test key made for
one run, whose public half is in the file and whose private half was never
stored. `cargo test --test contract` checks that every token verifies with
that public key and that its claims are what the code issues for its case;
after a change to the issuing rules,
`DEVTAB_WRITE_VECTORS=1 cargo test -p devtab-api --test contract server_vectors`
writes the file again with a new key.

`contract/licence-v1/verifier/` and `extension-vectors.json` are byte copies
of the extension's side: its token check and state function
(`packages/licence/lib/{verify,state,base64url,types}.ts`), its test vectors,
and the contract's rules as code (`scripts/mock-backend/contract.mjs`).
`script/contract-sync <path to a clone of the extension>` (or
`DEVTAB_EXTENSION_DIR=<path>`) refreshes them and records their checksums in
`SOURCES.sha256`; nothing is imported from that repository otherwise.
`pnpm test:contract` runs the backend's tokens and the extension's own
vectors through the copied check and state function, holds the issued claims
to the contract's issuing rules case by case, and the copies to their
checksums (`script/contract-sync --check` checks only the last). The
extension's modules import each other without file extensions; the test
resolves those with Node's own resolve hook (`node:module`), so the copies
stay as they are and no dependency is added. `cargo test --test contract`
runs the extension's vectors through the server's check. The token's `term`
and `renews` (a subscription's billing period and whether it renews) are among
the issued claims the vectors cover.

`contract/plans-v1/` is the contract of `GET /v1/plans`: the JSON Schema of
its answer, `server-catalogs.json` (answers this server builds from the
fixture catalogue `api/tests/fixtures/plans/three-plans.toml` and from
`config/plans.toml`, each with what the extension must make of it), and in
`reader/` byte copies of the extension's catalogue reader
(`extension/src/plans/{model,price}.ts`, `extension/src/shared/plansRecord.ts`),
which `script/contract-sync` copies with their checksums.
`pnpm test:contract` runs every answer through the extension's own
`parseCatalog`: a test or development build keeps every plan and drops none,
a production build drops the plans priced in XTS; the contract's
`catalogCheck` finds nothing the server should not have sent, and each answer
is within 32 KB. The reader uses TypeScript that Node's type stripping alone
does not take; the test's hooks (`node:module`) turn those copies into
JavaScript with Node's own `stripTypeScriptTypes`. After a change to the catalogue's format,
`DEVTAB_WRITE_VECTORS=1 cargo test -p devtab-api --test plans server_catalogs`
writes `server-catalogs.json` again.

### Requirements

- [rustup](https://rustup.rs); `rust-toolchain.toml` pins Rust 1.94.1 with
  rustfmt and clippy, and rustup installs it on first use.
- Docker, for the local Supabase stack.
- The [Supabase CLI](https://supabase.com/docs/guides/local-development/cli/getting-started)
  2.54.11, the release CI pins (a package manager or a release binary, as
  its documentation says). The scripts run it with `SUPABASE_TELEMETRY_DISABLED=1`
  and `DO_NOT_TRACK=1`; this release collects no usage data, later ones read
  those variables.
- `psql` (from any Postgres 15 or newer client), with which `script/db-up`
  gives the API role its local password.
- cargo-deny: `cargo install cargo-deny --locked`.
- sqlx-cli 0.9.0, the release of the sqlx crate: `script/sqlx-prepare`
  installs it into the ignored `.state/tools/` on first use and leaves any
  other installed release alone.

### Commands

| Command                       | What it does                                                                                                            |
| ----------------------------- | ----------------------------------------------------------------------------------------------------------------------- |
| `script/db-up`                | Starts the local Supabase Postgres alone (port 54330, database `postgres`) and prints the URLs                          |
| `script/db-migrate`           | Applies the migrations in `supabase/migrations/` that the local database does not have yet                              |
| `script/db-reset`             | Recreates the local database and applies every migration from zero                                                      |
| `script/api-dev`              | Runs the API against the local database as `devtab_api`, signing with a local test key                                  |
| `script/auth-up`              | Starts the local stack for sign-in: Postgres, Auth, the gateway on 54332 and the mail catcher on 54334                  |
| `script/site-server`          | Runs the built site's server on 127.0.0.1:4321 with the API and, when the stack runs, sign-in                           |
| `node script/e2e-sign-in.ts`  | Signs in end to end against the running stack, API and site: the code and the link of a real local mail                 |
| `script/sqlx-prepare`         | Writes the queries' offline data, `.sqlx/`, from the local database (`--check` compares)                                |
| `script/contract-sync <path>` | Copies the extension's side of the contracts into `contract/*-v1/` (`--check` compares)                                 |
| `script/gen`                  | Writes `contract/internal-openapi.json` and `src/lib/api/schema.gen.ts` (`--check` compares)                            |
| `pnpm test:contract`          | The contract tests on the extension's side: tokens and catalogues through the copied modules                            |
| `script/api-check`            | fmt, clippy with `-D warnings`, the tests (with the database), the `.sqlx/` check, `cargo deny`                         |
| `script/check-migrations`     | Fails when a migration of `origin/main` (or another base) was edited, renamed or deleted                                |
| `script/check-all`            | `pnpm check`, `script/gen --check`, `pnpm test:contract`, `script/check-migrations`, `script/api-check`: what CI checks |

```bash
script/db-up
script/api-dev                       # in another terminal:
curl -fsS localhost:8080/api/readyz  # {"status":"ok"}
```

On its first start the CLI creates the database and applies every migration;
`script/db-up` then gives `devtab_api` a password generated on this machine
into the ignored `.state/devtab_api.password`, and `script/api-dev` reads it
from there. Nothing secret is in the repository: the scripts read the
database's URL from `supabase status -o env`. `script/db-migrate` and
`script/db-reset` set the password again, since a reset creates the role
anew.

`DEVTAB_PUBLIC_ADDR`, `DEVTAB_INTERNAL_ADDR` move the listeners (for example
when 8080 or 8081 is taken); `devtab-api healthcheck` reads the same
`DEVTAB_PUBLIC_ADDR`. The database tests (`api/tests/db.rs`, `v1.rs`,
`admin.rs`, `jobs.rs`, `plans.rs`, and those of `auth.rs` that need people)
run when `DEVTAB_TEST_DATABASE_URL` names a database
with the migrations applied; `script/api-check` points it at the local one,
plain `cargo test` skips them with a note. They act as the API role and
remove what they create.

Stop the stack with `supabase stop`; its data stays in Docker volumes
(`supabase stop --no-backup` deletes them). The local stack uses ports
54330-54337, away from the CLI's defaults, so it runs next to another
Supabase project's stack. The CLI publishes them on every interface of the
machine, not only the loopback address, and the owner's password is the
CLI's well-known default: keep the machine's firewall on.

`script/db-up` starts Postgres alone, which is all the API's checks need.
`script/auth-up` adds what sign-in needs: Auth, the gateway and the mail
catcher (not Studio, the Data API or the other services). Auth allows
e-mail sign-in by link or one-time code and by password (a new address
confirmed by its mail first), with its mails `supabase/templates/sign_in.html`
and `recovery.html`; Google is configured but off unless the environment
turns it on (above, "What a deployment sets in Supabase"); no captcha. It signs access tokens with an ES256 key of this machine:
`supabase/config.toml` names `.state/supabase/signing-keys.json`, which
`script/lib/supabase.sh` makes on first use (`supabase gen signing-key`,
mode 600; the CLI refuses to start without the file) and which is never
committed; Auth publishes its public half in the JWKS. Run the CLI through
the scripts, or make that file first, on a fresh clone.

Nobody runs `supabase login`, `supabase link`, `supabase db push` or any
other command that talks to a hosted project without the owner's word.

Dependencies come from crates.io only (`deny.toml`), at versions published at
least two weeks before they are locked, transitive ones included: after
`cargo update`, check the new versions' dates and step back a patch where
needed (`cargo update -p <crate> --precise <version>`).

### Administration

`devtab-api admin …` on the server (in development: `cargo run -p devtab-api --
admin …` with `DEVTAB_DATABASE_URL` set, or the built binary). Every change is
recorded in `app.admin_audit` with the machine's user name, in the same
transaction; keys are printed once and never stored.

| Command                                                                        | What it does                                                                                                           |
| ------------------------------------------------------------------------------ | ---------------------------------------------------------------------------------------------------------------------- |
| `licence comp --plan <id> [--user <e-mail>] [--until <date>] [--note "…"]`     | Issues a licence free of charge (to an account or to nobody); prints its key                                           |
| `licence show <lid \| e-mail>`                                                 | A licence, or an account's licences: installations, recent admin actions                                               |
| `licence block <lid> --reason "…"`                                             | Blocks the key; installations get `revoked: key-blocked` at their next check                                           |
| `licence revoke <lid> --refund`                                                | Records a refund the provider did not report: `revoked: refund`                                                        |
| `licence rotate-key <lid>`                                                     | A new key, printed once; installations activated with the old one keep working                                         |
| `user block <e-mail>`, `user unblock <e-mail>`                                 | `app.users.status`; licences are not touched                                                                           |
| `installation release <id>`                                                    | Frees a seat; that installation's next check gets `revoked: deactivated`                                               |
| `plans sync [--file config/plans.toml] [--yes]`                                | Makes the catalogue the file's: plans, texts, prices read from the provider; prints the changes, asks first            |
| `keys generate --kid <p1, p2, …> --out <file>`, `keys generate --test --out …` | An ES256 key pair: the private key into a new file, the public entry printed (`--list-out <file>`: also as a key list) |
| `keys check`                                                                   | The server's key against the trusted keys, as `serve` checks it                                                        |
| `stripe check [--file config/plans.toml]`                                      | Stripe's settings and secrets as `serve` checks them; every Stripe price of the file found at Stripe; changes nothing  |
| `mail templates [--apply]`                                                     | The listmonk templates of every mail: what it would do; `--apply` creates or updates them and prints their ids         |

`devtab-api openapi` prints the internal listener's OpenAPI document (what
`script/gen` writes); it needs neither the database nor any setting.

`serve --emergency-key` signs with a key whose `sign_from` has not come yet,
for the day the signing key leaks and the waiting key takes over; it is
logged and recorded in `app.admin_audit`.

### Queries and `.sqlx/`

Queries are `sqlx::query!`, checked against the schema when they compile.
Builds use the committed offline data in `.sqlx/` (`SQLX_OFFLINE=true` in
`script/api-check`, CI and later the image); with a database at hand,
`DATABASE_URL` makes the macros ask it instead. After changing a query or a
migration: `script/db-migrate`, then `script/sqlx-prepare`, and commit
`.sqlx/` with the change. `script/api-check` and CI fail when `.sqlx/` is out
of date.

### Migrations

`supabase/migrations/<timestamp>_<name>.sql`, applied in order by the
Supabase CLI and recorded in the database; `supabase migration new <name>`
creates the next file. They only go forward: an applied migration is never
edited or deleted. The CLI records a migration by its version alone and would
not notice an edited one, so `script/check-migrations [<base>]` (in
`script/check-all` and CI) fails when a file that the base (`origin/main`;
in CI the pull request's target or the commit before a push to `main`)
already has was edited, renamed or deleted, or when a new file does not sort
after the base's last one; uncommitted files count. Every object a migration
creates in `app` gets row level security, and nothing in `app` is granted to
the Data API's roles. So far:
the schema and the API role; `app.users` with its trigger on `auth.users` and
the two lookups by e-mail address; `app.plans`; licences, installations,
customers, the job queue and the admin record; the plans' texts and prices and
`app.settings` (the catalogue's version); purchases, provider events and keys
to reveal; team plans and members; guest checkouts (no user until paid, the
browser's secret's hash, the buyer) and each user's canonical inbox; the
mail jobs' keys, licences' languages and team members' ids; trials and the
inboxes that had one.

### Configuration

| Variable                                  | Default                                              | Note                                                                  |
| ----------------------------------------- | ---------------------------------------------------- | --------------------------------------------------------------------- |
| `DEVTAB_ENV`                              | `dev`                                                | `dev`, `staging` or `prod`                                            |
| `DEVTAB_PUBLIC_ADDR`                      | `0.0.0.0:8080`                                       |                                                                       |
| `DEVTAB_INTERNAL_ADDR`                    | `127.0.0.1:8081` in `dev`, `0.0.0.0:8081` otherwise  |                                                                       |
| `DEVTAB_DATABASE_URL`                     | none: required (`script/api-dev` sets the local one) | `postgres://` or `postgresql://`                                      |
| `DEVTAB_DB_MAX_CONNECTIONS`               | `10`                                                 | greater than 0                                                        |
| `DEVTAB_LOG`                              | `info`                                               | `tracing` directives                                                  |
| `DEVTAB_SIGNING_KEY_FILE`                 | none: `serve` needs it (`script/api-dev`: test key)  | the private key, PKCS#8 PEM                                           |
| `DEVTAB_SIGNING_KID`                      | none: `serve` needs it (`script/api-dev`: test key)  | its id among the trusted keys                                         |
| `DEVTAB_DEV_SIGNING_KEYS_FILE`            | none (`script/api-dev`: `.state/signing/`)           | `dev` only: a developer's test public keys; refused otherwise         |
| `DEVTAB_LICENCE_HOST`                     | none: any host in `dev`; required otherwise          | `/v1/*` answers 404 on any other `Host`                               |
| `DEVTAB_EXTENSION_ORIGINS`                | none in `dev`; required otherwise                    | `chrome-extension://<id>`, comma-separated: CORS of `/v1/*`           |
| `DEVTAB_MAINTENANCE_UNTIL`                | none                                                 | RFC 3339; until then `/v1/*` answers 503 with `Retry-After`           |
| `DEVTAB_TOKEN_CHECK_DAYS`                 | `7`                                                  | `chk`                                                                 |
| `DEVTAB_TOKEN_WINDOW_DAYS`                | `30`                                                 | `exp`                                                                 |
| `DEVTAB_TOKEN_WARN_DAYS`                  | `7`                                                  | `warn` before `exp`                                                   |
| `DEVTAB_TOKEN_DUNNING_DAYS`               | `14`                                                 | a subscription's grace after its paid period                          |
| `DEVTAB_RELEASE_AFTER_DAYS`               | `60`                                                 | an installation not seen for this long loses its seat                 |
| `DEVTAB_ACTIVATION_CHURN`                 | `3`                                                  | new activations in 30 days, per seat                                  |
| `DEVTAB_RATE_ACTIVATE_PER_IP`             | `30`                                                 | requests an hour, by client address                                   |
| `DEVTAB_RATE_ACTIVATE_PER_KEY`            | `10`                                                 | requests an hour, by key                                              |
| `DEVTAB_RATE_REFRESH_PER_IP`              | `60`                                                 | requests an hour, by client address                                   |
| `DEVTAB_RATE_REFRESH_PER_INSTALLATION`    | `6`                                                  | requests an hour, by licence and installation                         |
| `DEVTAB_RATE_DEACTIVATE_PER_INSTALLATION` | `10`                                                 | requests an hour, by licence and installation                         |
| `DEVTAB_RATE_PLANS_PER_IP`                | `60`                                                 | `GET /v1/plans`: requests a minute, by client address                 |
| `DEVTAB_SITE_URL`                         | `https://web.example.invalid` in `dev`               | required outside `dev`: `https://<host>`, checkout, manage            |
| `DEVTAB_PAYMENTS`                         | `fake` in `dev`; none otherwise                      | `fake` or `stripe`: whose prices are served; `fake` refused in `prod` |
| `DEVTAB_PAYMENTS_ENV`                     | none: required with `stripe`                         | `sandbox` or `live`; `prod` takes `live` only                         |
| `DEVTAB_PAYMENTS_API_KEY_FILE`            | none: required with `stripe`                         | the file of a restricted key of that mode, `rk_test_…`/`rk_live_…`    |
| `DEVTAB_PAYMENTS_WEBHOOK_SECRET_FILE`     | none: `serve` needs it with `stripe`                 | the file of the webhook destination's secret, `whsec_…`               |
| `DEVTAB_STRIPE_MANAGED_PAYMENTS`          | `true`                                               | Stripe sells as merchant of record                                    |
| `DEVTAB_STRIPE_AUTOMATIC_TAX`             | `false`                                              | Stripe Tax; only with Managed Payments off                            |
| `DEVTAB_STRIPE_CHECKOUT_UI`               | `hosted`                                             | `hosted` (Stripe's page) or `embedded` (Stripe's form on the site)    |
| `DEVTAB_STRIPE_PUBLISHABLE_KEY`           | none: required with `embedded`                       | `pk_test_…`/`pk_live_…` of the mode; public                           |
| `DEVTAB_STRIPE_API_VERSION`               | `2026-11-18.endive`                                  | not older than `2026-09-30.endive`; the webhook destination's too     |
| `DEVTAB_STRIPE_API_URL`                   | `https://api.stripe.com`                             | `https://<host>[:port]`; `http` to the loopback address in `dev` only |
| `DEVTAB_STRIPE_TIMEOUT`                   | `8`                                                  | seconds per attempt of a request, 1 to 60                             |
| `DEVTAB_STRIPE_WEBHOOK_TOLERANCE`         | `300`                                                | seconds a webhook's signature stays valid, 60 to 600                  |
| `DEVTAB_PLANS_MAX_AGE`                    | `86400`                                              | `maxAge` of `GET /v1/plans`, 3600 to 604800 seconds                   |
| `DEVTAB_SUPABASE_URL`                     | none in `dev`; required otherwise                    | the project (`script/api-dev`: http://127.0.0.1:54332): JWKS          |
| `DEVTAB_JWT_ISSUER`                       | `<DEVTAB_SUPABASE_URL>/auth/v1`                      | the access tokens' `iss`                                              |
| `DEVTAB_GUEST_CHECKOUT`                   | `false`                                              | `true`: a checkout may start without an account                       |
| `DEVTAB_SUPABASE_SECRET_KEY_FILE`         | none: guest checkouts refused without it             | the file of the project's secret key (`sb_secret_…`): invites buyers  |
| `DEVTAB_SUPABASE_AUTH_URL`                | `<DEVTAB_SUPABASE_URL>/auth/v1`                      | Supabase Auth as the API reaches it, ending in `/auth/v1`             |
| `DEVTAB_TRIAL_DAYS`                       | none: no trial                                       | 7 to 14: the trial without a card; any other value is refused         |
| `DEVTAB_TRIAL_PLAN`                       | none: required with the days                         | the plan whose seats a trial gives                                    |
| `DEVTAB_MAIL`                             | `none`: mail jobs wait                               | `none`, `log` (not in `prod`) or `listmonk`                           |
| `DEVTAB_MAIL_FROM`                        | the provider's default                               | the sender, `Name <address>`                                          |
| `DEVTAB_MAIL_LISTMONK_URL`                | none: required with `listmonk`                       | `https://<host>[:port]`; `http` to the loopback address in `dev` only |
| `DEVTAB_MAIL_LISTMONK_API_USER`           | none: required with `listmonk`                       | the API user                                                          |
| `DEVTAB_MAIL_LISTMONK_API_TOKEN_FILE`     | none: required with `listmonk`                       | the file of its token                                                 |
| `DEVTAB_MAIL_LISTMONK_TEMPLATES`          | none: every mail refused                             | `<kind>.<locale>=<id>,…`, as `admin mail templates --apply` prints    |
| `DEVTAB_MAIL_LISTMONK_LIST_ID`            | none: `external` mode                                | the list recipients join first                                        |
| `DEVTAB_MAIL_TRIAL_REMIND_DAYS`           | `3,1`                                                | days before a trial's end, each fewer than the trial's days           |
| `DEVTAB_MAIL_RENEWAL_REMIND_DAYS`         | `7`                                                  | days before a yearly renewal, 1 to 60                                 |

Logs are compact lines in `dev` and JSON lines in `staging` and `prod`. They
carry a licence id and the first four characters of an installation id;
never a key, a secret, a token, an e-mail address or a client address. The
client address (the last `X-Forwarded-For` entry, else the peer) lives only
in the rate limiters' memory.

### Layout

```
Cargo.toml, Cargo.lock    the Rust workspace, its lints and the release profile
rust-toolchain.toml       the pinned Rust toolchain
clippy.toml, deny.toml    clippy thresholds; licences, advisories and sources of dependencies
api/src/main.rs           the binary: serve, admin, healthcheck, version
api/src/config.rs         DEVTAB_* configuration, every problem reported at once
api/src/clock.rs          the time decisions depend on, movable in tests
api/src/http/             routers, middleware, the error shape, /v1/*, the plans, the internal listener and its
                          OpenAPI description, CORS, rate limits, health checks
api/src/licence/          keys, secrets, the token and its signing, activation, refresh, seats
api/src/plans/            the plans file, its sync, currencies, the catalogue the endpoints answer
api/src/auth/             signed-in people: access tokens against the JWKS, the CurrentUser extractor
api/src/accounts.rs       Supabase Auth's admin API (inviting a guest purchase's buyer), the canonical inbox
api/src/trial.rs          the trial without a card
api/src/mail/             the mails: kinds, the queue, composing a due mail, the providers (listmonk, log), templates
api/src/payments/         the payment provider's interface, the fake provider and Stripe (settings' secrets, REST
                          client, prices, checkout, portal, webhook signatures)
api/src/store/            the Postgres pool and the queries, a module for each of the tables
api/src/jobs/             the queue's worker (process_event, retries) and the scheduler with its task under an
                          advisory lock
api/src/admin.rs          the admin commands
api/tests/                HTTP, database, /v1/*, plans, sign-in, admin, scheduler, start-up and contract tests;
                          fixtures/plans/ (a test catalogue)
.sqlx/                    the queries' offline data (script/sqlx-prepare)
config/                   signing-keys.json (published public keys), plans.toml (the plans' catalogue),
                          plans.stripe.example.toml (the shape of a deployment's plans file with Stripe prices)
deploy/                   api.env.example and web.env.example: every setting, the defaults commented out
contract/licence-v1/      the licence contract: schemas, errors, vectors, the copied verifier, its test
contract/plans-v1/        the plans contract: the answer's schema, server catalogues, the copied reader, its test
contract/internal-openapi.json  the internal listener's OpenAPI document (script/gen)
supabase/config.toml      the local Supabase stack: ports, the Data API, Auth
mail/listmonk/            the API's mail templates for listmonk, a skeleton of every kind in English and Russian
supabase/templates/       Auth's mails: sign-in or confirmation, password reset (link and code), the invite of a guest
                          purchase (English and Russian)
supabase/migrations/      SQL migrations, forward only
script/                   db-up, db-migrate, db-reset, auth-up, api-dev, site-server, api-check, check-all,
                          check-migrations, sqlx-prepare, contract-sync, gen, e2e-sign-in.ts; lib/supabase.sh
.github/workflows/ci.yml  CI: secret scan, the site and the contract tests, the migrations check, the API and the
                          generated files, sign-in end to end
```

## Licence

The source is available under the
[Fair Core License, Version 1.0, ALv2 Future License](LICENSE.md)
(`FCL-1.0-ALv2`), the same licence as the extension. It is not an open source
licence. In short, and the licence itself is what counts:

- You may read, build, use and change the code for any purpose except a
  competing one: you may not offer it, or a product made from it, to others as
  a commercial product or service that replaces this one or does substantially
  the same.
- You may not remove, change or get around a licence key check that protects
  paid features.
- Each version becomes available under the Apache License, Version 2.0 two
  years after it is published.

The name DevTab.Dev, its icons and the site's texts and images are not
licensed.
