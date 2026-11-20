# devtab-dev-site

The product site of **devtab.dev**, a Chrome extension for comparing and
reviewing code. `devtab-dev` is the project's code name; the product has no
display name yet, so the site shows the domain as its wordmark.

The site is empty on purpose: a home page, a 404, `robots.txt`, a sitemap and
a language switch, in English and Russian. It is a static Astro site with no
client-side JavaScript, no analytics, no third-party scripts and nothing
loaded from another host.

The project has two sibling repositories: `devtab-dev-extension` (the
extension) and `devtab-dev-docs` (developer documents, private). This
repository imports nothing from either and does not need them checked out.

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

## Licence

None chosen yet: the repository has no licence file.
