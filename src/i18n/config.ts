/**
 * The site's locales. The first one is the default: its pages live at the
 * root (`/`), every other locale's under its own prefix (`/ru/`). Adding a
 * locale is one line here and one catalogue in `messages/` (README, "Add a
 * locale"); the routes, the language switch, the alternates and the sitemap
 * follow from this list.
 */
export const locales = ['en', 'ru'] as const;

export type Locale = (typeof locales)[number];

export const defaultLocale: Locale = locales[0];
