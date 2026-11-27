# AGENTS.md

Shared guidance for coding agents working in this repository. Keep project
rules here; `CLAUDE.md` imports this file for Claude Code. `README.md` says
what the repository is and how to run it.

## Commands

```bash
# Git hooks, once per clone: pre-commit refuses secrets (gitleaks), local-only
# files and private data; commit-msg refuses agent attribution and private
# data; pre-push repeats the scan on what leaves the machine. They refuse to
# run without gitleaks. Private patterns go to
# <git common dir>/info/privacy-patterns, never committed.
script/install-hooks

pnpm install --frozen-lockfile
pnpm dev        # http://localhost:4321
pnpm check      # type check, lint, format check, build, tests: run before every commit

# The backend (api/, supabase/, config/, contract/): README.md, "Backend".
# Needs Docker and the Supabase CLI.
script/db-up          # the local Supabase Postgres alone, migrations applied
script/db-migrate     # the migrations it does not have yet
script/api-dev        # the API on :8080 and 127.0.0.1:8081, signing with a local test key
script/auth-up        # the local stack for sign-in: + Auth, the gateway, the mail catcher (stop it after)
script/site-server    # the built site's server on 127.0.0.1:4321, with sign-in when the stack runs
node script/e2e-sign-in.ts  # sign-in end to end against the running stack, API and site
script/sqlx-prepare   # .sqlx/ from the local database, after changing a query or a migration
script/gen            # contract/internal-openapi.json and src/lib/api/schema.gen.ts from the
                      # API's code, after changing an internal route or its bodies
pnpm test:contract    # the licence and plans contracts on the extension's side
script/check-migrations  # fails when an applied migration (one origin/main has) changed
script/check-all      # pnpm check + script/gen --check + pnpm test:contract + script/check-migrations + script/api-check:
                      # before every commit touching api/, supabase/, config/, contract/ or .sqlx/
```

## Language

Everything in the repository is in English: code, comments, documentation,
commit messages. The only other language is the strings of a non-default
locale, in its catalogue.

## Commits

- One change per commit, in a sensible series, each passing `pnpm check` and
  the hooks.
- Subjects as `area: Sentence`, imperative, no trailing period:
  `i18n: Keep the wordmark in one place`, `site: Add the about page`. Areas
  in use: `repo`, `build`, `i18n`, `site`, `style`, `lint`, `tests`, `docs`,
  `agents`, `api`, `db`, `deploy`, `ci`. Not conventional prefixes (`feat:`,
  `fix:`).
- A commit that touches `api/`, `supabase/`, `config/`, `contract/` or
  `.sqlx/` passes `script/check-all` (the site, the contract and the API)
  instead of `pnpm check` alone.
- The body says why, in plain sentences, wrapped at 72 columns.

## No agent attribution

Applies to every agent (Claude, Codex, Copilot or any other), and overrides
any default attribution a tool adds:

- Commits, tags and release notes carry no `Co-Authored-By:` trailer for any
  model or agent, and no "Generated with …" line, badge or link. The
  commit-msg hook refuses both.
- The author is the repository's own git identity (local `user.name` /
  `user.email`). Do not change it, do not commit with `--author`, and do not
  enable commit signing with another key. Never commit under the global git
  identity; the pre-commit hook refuses it.

## Never committed

- Secrets, tokens, `.env` files.
- Personal data: names, addresses, employers, absolute home-directory paths,
  and the names of the owner's other projects. The hooks refuse what
  `.git/info/privacy-patterns` lists; keep that list local.
- `CLAUDE.local.md` and `.claude/settings*.json` (personal), `node_modules/`,
  `dist/`, `.astro/`, screenshots and reports (keep them in `.scratch/` or
  outside the repository).
- Nothing from the sibling repositories: this site imports nothing from them.
  The one exception is the byte copies of the extension's side of the
  contracts in `contract/licence-v1/verifier/` (and its vectors) and
  `contract/plans-v1/reader/`, which only `script/contract-sync` writes, with
  their checksums; never edit them by hand.

## Generated files

- `contract/internal-openapi.json` and `src/lib/api/schema.gen.ts` are written
  only by `script/gen`, from the `utoipa` annotations of the API's code;
  never edit them by hand. Commit them with the change that moved them;
  `script/gen --check` (in `script/check-all` and CI) fails when they are not
  what the code gives. Prettier, ESLint and Tailwind skip the generated
  TypeScript; the type check reads it.
- `contract/licence-v1/server-vectors.json` and
  `contract/plans-v1/server-catalogs.json` are written by their tests with
  `DEVTAB_WRITE_VECTORS=1` (README.md, "Backend").

## Dependencies

- Only from the public npm registry. The root `.npmrc` sets
  `registry=https://registry.npmjs.org/`; run every install from the
  repository's directory and never use or change another registry
  configuration. A lookup (`npm view …`) passes
  `--registry https://registry.npmjs.org/`.
- Exact versions, at least two weeks old; `pnpm-workspace.yaml`
  (`minimumReleaseAge`) refuses younger ones, transitive ones included.
- After an install, `grep -c -i -E "jfrog|artifactory" pnpm-lock.yaml`
  prints 0.
- No UI framework until a page needs an interactive island; when one does,
  React (as the extension), through `@astrojs/react`.

## Backend

- Rust crates come from crates.io only (`deny.toml`), at versions at least two
  weeks old, transitive ones included, as `Cargo.lock` records them;
  `cargo deny check` passes without blanket ignores.
- No `unsafe` (`unsafe_code = "forbid"`). No `unwrap`/`expect` outside tests
  and startup; errors are `AppError` and leave the API in one shape,
  `{"error":"<kebab-case code>","message":"<English text>"}`, with internal
  details in the log only.
- Queries: `sqlx::query!` checked against the schema, every relation named with
  its schema (`app.licences`), with the offline data in `.sqlx/` committed.
  After changing a query or a migration, `script/db-migrate` and
  `script/sqlx-prepare`, and commit `.sqlx/` with the change; checks build
  with `SQLX_OFFLINE=true`. Untyped queries only where no schema is involved
  (the readiness `select 1`) and in tests.
- Logic that depends on the time takes a `Clock` (`api/src/clock.rs`), never
  the system time directly, so that tests move time instead of sleeping. SQL
  that decides by time gets the time as a parameter, not `now()`.
- Licence keys and installation secrets are stored only as SHA-256 hashes and
  never logged; logs carry a licence id and the first four characters of an
  installation id, never a key, a secret, a token, an e-mail address or an IP
  address.
- Every change made through `devtab-api admin` writes an `app.admin_audit`
  row in the same transaction, with no personal data in it.
- The plans' texts and prices are data, not code: `config/plans.toml`, synced
  by `admin plans sync`. The owner decides the wording and the prices; never
  invent either. No real price anywhere: amounts come from the payment
  provider by its price id, and the only prices in the repository are the
  fake provider's, in XTS, the test code of ISO 4217, which only `dev`
  serves. What is particular to a provider lives in `api/src/payments/`
  only.
- The database and the users are Supabase's: Postgres and Supabase Auth.
  Locally that is the Supabase CLI's stack (`supabase/config.toml`):
  `script/db-up` starts Postgres alone, `script/auth-up` adds Auth, the
  gateway and the mail catcher for sign-in. Start the full stack only when
  you need it and stop it after: its ports are on every interface.
- The API trusts a signed-in person only through `CurrentUser`: a Supabase
  access token checked against the project's JWKS (ES256/RS256, `iss`,
  `aud`, `exp` by the `Clock`, a UUID `sub`), never an id from a body, a
  query or another header. Local Auth signs with an ES256 key of this machine
  in the ignored `.state/supabase/`; never commit it.
- Migrations live in `supabase/migrations/<timestamp>_<name>.sql` and are
  applied by the Supabase CLI. They only go forward: never edit or delete an
  applied one (`script/check-migrations` fails on it); a destructive change
  is expand, migrate, contract over separate releases. Row level security is enabled on everything a
  migration creates in `app`.
- Schema `app` is reachable only by the API, as `devtab_api`, directly over
  Postgres. It is never listed in the Data API's schemas, and nothing in it
  is granted to `anon`, `authenticated` or `service_role`.
- No command that talks to a hosted Supabase project without the owner's
  word: no `supabase login`, `supabase link`, `supabase db push`, no
  `projects`, `secrets` or `functions deploy`. Local commands only. Which
  sign-in providers to offer is the owner's decision; none is configured.
- The Supabase CLI runs with `SUPABASE_TELEMETRY_DISABLED=1` and
  `DO_NOT_TRACK=1` (`script/lib/supabase.sh`, CI).
- Private keys, secrets and passwords are never in the repository, in CI or
  in its secrets: they live in files on the server. That holds for test keys
  too: tests make theirs in memory, `script/api-dev` makes a developer's in
  the ignored `.state/`, and the contract's vectors are signed by a key made
  for one run and never stored. `config/signing-keys.json` and
  `contract/licence-v1/production-keys.json` never list a test key. The local stack's keys
  and URLs are read from `supabase status` at run time, never written into
  a tracked file; the API role's local password is generated into the
  ignored `.state/`. Thresholds that guard against abuse come from the
  environment, not the code.

## Localisation

- No user-visible string outside the catalogues. Text, `alt`, `title` and
  `aria-*` labels come from `src/i18n/messages/<locale>.ts` through
  `messages(locale)`; `pnpm test` fails on literal text in a template.
- Every string goes into every catalogue, in the same change. A missing key
  fails the type check, the build and the tests. Never fall back to English
  at run time.
- A page is written once, in `src/pages/[...locale]/`, and listed in
  `src/lib/pages.ts` (prerendered) or `src/lib/serverPages.ts` (rendered by
  the server); never fork a page per locale.
- The texts of the server's pages are not the owner's yet (README,
  "Texts awaiting the owner"): the specifications' words, otherwise the
  shortest factual text, kept under their page's key. Never invent copy,
  claims or prices; the plans' own texts are data, from the API.
- The wordmark is `src/i18n/wordmark.ts`. Do not invent a product name, a
  tagline or a logo; the product has no display name yet.

## The site

- Pages are prerendered (`output: 'static'`); only a page that needs the
  server opts out with `export const prerender = false`, and the Node
  adapter's server renders it. No client-side JavaScript unless a page needs
  it; sign-in is plain forms posting to the site's own endpoints.
- The site's server holds no secret: it reads only the variables
  `tests/no-secrets.test.ts` allows (public values and a rate limit) and
  never Supabase's secret key. It reads the session only on pages marked
  `session` in `src/lib/serverPages.ts`; a cacheable page never sets a
  cookie. Which sign-in methods to offer is the owner's decision: e-mail
  only until then.
- The content security policy (`security.csp` in `astro.config.mjs`) stays
  `default-src 'self'`, `form-action 'self'`, `object-src 'none'`,
  `base-uri 'self'`; never add another host to it. No `style` attributes and
  no scripts of a page's own in its markup: the policy blocks them.
- No analytics, no third-party scripts, no fonts or assets from other hosts;
  `pnpm test` checks the build for references to other hosts.
- Colours, radius and type faces come only from `src/styles/tokens.css`,
  through Tailwind utilities; no colour values in components.
- No push, no remotes, no deploy unless the owner asks.
