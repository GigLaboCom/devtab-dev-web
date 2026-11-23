# devtab-dev-web

The product site of **DevTab.Dev**, a Chrome extension for comparing and
reviewing code. `devtab-dev` is the project's code name; the site shows the
product's name as its wordmark.

The site is empty on purpose: a home page, a 404, `robots.txt`, a sitemap and
a language switch, in English and Russian. It is a static Astro site with no
client-side JavaScript, no analytics, no third-party scripts and nothing
loaded from another host.

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
| `pnpm build`        | Static build to `dist/`                                                |
| `pnpm preview`      | Serves `dist/`                                                         |
| `pnpm check`        | Everything below, in order: the one command to run before a commit     |
| `pnpm typecheck`    | `astro check`: TypeScript and the templates                            |
| `pnpm lint`         | ESLint                                                                 |
| `pnpm format:check` | Prettier (`pnpm format` writes)                                        |
| `pnpm test`         | `node --test`: the catalogues and templates, then the build in `dist/` |

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
- the built pages load nothing from another host.

## Layout

```
astro.config.mjs          site address, URL shape, i18n routing, Tailwind
src/i18n/config.ts        the locales; the first is the default
src/i18n/messages/*.ts    one string catalogue per locale (en.ts is the source)
src/i18n/wordmark.ts      the wordmark every locale shows
src/i18n/validate.ts      the catalogue check (the build and the tests)
src/i18n/index.ts         messages(locale), the routes of a localised page
src/lib/pages.ts          every localised page, by id and path
src/lib/site.ts           the site's address
src/layouts/              <html lang>, title, canonical URL, alternates
src/components/           the header and the language switch
src/pages/[...locale]/    the localised pages
src/pages/                404, robots.txt, sitemap.xml
src/styles/tokens.css     the design tokens (placeholders)
tests/                    node:test, from the sources and from dist/
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
two listeners. The public one (`:8080`) answers `/api/healthz` and
`/api/readyz`; the internal one (`127.0.0.1:8081` in development) answers
`/internal/healthz` and is meant only for the site's own pages, never
published. It reads its configuration from `DEVTAB_*` variables and keeps its
data in Postgres 17, schema `app`, as the role `devtab_api`. So far it is a
scaffold: health checks, the error shape, the middleware and the first
migration. The site does not call it yet, and the site's build does not
change because of it.

The backend stands on [Supabase](https://supabase.com): its Postgres holds the
API's data, and Supabase Auth will hold the users, so both are managed the
same way. Locally that is the Supabase CLI's stack in Docker
(`supabase/config.toml`). The API connects to Postgres directly, as
`devtab_api`; schema `app` is not exposed through Supabase's Data API, and
its roles (`anon`, `authenticated`, `service_role`) have no privilege in it.
No hosted Supabase project is linked to this repository yet.

Every error answers
`{"error":"<kebab-case code>","message":"<English text>"}`.

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

### Commands

| Command             | What it does                                                                                    |
| ------------------- | ----------------------------------------------------------------------------------------------- |
| `script/db-up`      | Starts the local Supabase Postgres alone (port 54330, database `postgres`) and prints the URLs  |
| `script/db-migrate` | Applies the migrations in `supabase/migrations/` that the local database does not have yet      |
| `script/db-reset`   | Recreates the local database and applies every migration from zero                              |
| `script/api-dev`    | Runs the API against the local database as `devtab_api`                                         |
| `script/api-check`  | `cargo fmt --check`, clippy with `-D warnings`, the tests (with the database), `cargo deny`     |
| `script/check-all`  | `pnpm check`, then `script/api-check`: what CI checks, before a commit to `api/` or `supabase/` |

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
`DEVTAB_PUBLIC_ADDR`. The database tests (`api/tests/db.rs`) run when
`DEVTAB_TEST_DATABASE_URL` names a database with the migrations applied;
`script/api-check` points it at the local one, plain `cargo test` skips them.

Stop the stack with `supabase stop`; its data stays in Docker volumes
(`supabase stop --no-backup` deletes them). The local stack uses ports
54330-54337, away from the CLI's defaults, so it runs next to another
Supabase project's stack. The CLI publishes them on every interface of the
machine, not only the loopback address, and the owner's password is the
CLI's well-known default: keep the machine's firewall on.

So far the scripts start Postgres alone. Auth, the Data API, Studio and the
mail catcher are a dozen more containers, needed once sign-in is built:
`supabase start` starts the whole stack (`supabase status` then lists its
URLs; Studio on http://127.0.0.1:54333). Auth allows e-mail sign-in only; no
external provider is configured.

Nobody runs `supabase login`, `supabase link`, `supabase db push` or any
other command that talks to a hosted project without the owner's word.

Dependencies come from crates.io only (`deny.toml`), at versions published at
least two weeks before they are locked, transitive ones included: after
`cargo update`, check the new versions' dates and step back a patch where
needed (`cargo update -p <crate> --precise <version>`).

### Migrations

`supabase/migrations/<timestamp>_<name>.sql`, applied in order by the
Supabase CLI and recorded in the database; `supabase migration new <name>`
creates the next file. They only go forward: an applied migration is never
edited or deleted. Every object a migration creates in `app` gets row level
security, and nothing in `app` is granted to the Data API's roles.

### Configuration

| Variable                    | Default                                              | Note                             |
| --------------------------- | ---------------------------------------------------- | -------------------------------- |
| `DEVTAB_ENV`                | `dev`                                                | `dev`, `staging` or `prod`       |
| `DEVTAB_PUBLIC_ADDR`        | `0.0.0.0:8080`                                       |                                  |
| `DEVTAB_INTERNAL_ADDR`      | `127.0.0.1:8081` in `dev`, `0.0.0.0:8081` otherwise  |                                  |
| `DEVTAB_DATABASE_URL`       | none: required (`script/api-dev` sets the local one) | `postgres://` or `postgresql://` |
| `DEVTAB_DB_MAX_CONNECTIONS` | `10`                                                 | greater than 0                   |
| `DEVTAB_LOG`                | `info`                                               | `tracing` directives             |

Logs are compact lines in `dev` and JSON lines in `staging` and `prod`.

### Layout

```
Cargo.toml, Cargo.lock    the Rust workspace, its lints and the release profile
rust-toolchain.toml       the pinned Rust toolchain
clippy.toml, deny.toml    clippy thresholds; licences, advisories and sources of dependencies
api/src/main.rs           the binary: serve, healthcheck, version
api/src/config.rs         DEVTAB_* configuration, every problem reported at once
api/src/http/             routers, middleware, the error shape, the JSON extractor, health checks
api/src/store/            the Postgres pool
api/tests/                HTTP tests without a network; database tests
supabase/config.toml      the local Supabase stack: ports, the Data API, Auth
supabase/migrations/      SQL migrations, forward only
script/                   db-up, db-migrate, db-reset, api-dev, api-check, check-all; lib/supabase.sh
.github/workflows/ci.yml  CI: secret scan, the site, the API
```

## Licence

None chosen yet: the repository has no licence file.
