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
  `agents`. Not conventional prefixes (`feat:`, `fix:`).
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

## Localisation

- No user-visible string outside the catalogues. Text, `alt`, `title` and
  `aria-*` labels come from `src/i18n/messages/<locale>.ts` through
  `messages(locale)`; `pnpm test` fails on literal text in a template.
- Every string goes into every catalogue, in the same change. A missing key
  fails the type check, the build and the tests. Never fall back to English
  at run time.
- A page is written once, in `src/pages/[...locale]/`, and listed in
  `src/lib/pages.ts`; never fork a page per locale.
- The wordmark is `src/i18n/wordmark.ts`. Do not invent a product name, a
  tagline or a logo; the product has no display name yet.

## The site

- Static output, no client-side JavaScript unless a page needs it.
- No analytics, no third-party scripts, no fonts or assets from other hosts;
  `pnpm test` checks the build for references to other hosts.
- Colours, radius and type faces come only from `src/styles/tokens.css`,
  through Tailwind utilities; no colour values in components.
- No push, no remotes, no deploy unless the owner asks.
