import { defaultLocale, locales, type Locale } from './config.ts';
import type { Messages } from './messages/en.ts';
import { catalogueProblems } from './validate.ts';

export { defaultLocale, locales, type Locale, type Messages };

// Every catalogue in messages/, by file name: adding a locale adds no import.
const modules = import.meta.glob<{ default: Messages }>('./messages/*.ts', { eager: true });
const catalogues = Object.fromEntries(
  Object.entries(modules).map(([file, module]) => [file.replace(/^.*\/|\.ts$/g, ''), module.default]),
) as Record<string, Messages>;

const problems = catalogueProblems(catalogues, locales, defaultLocale);
if (problems.length > 0) {
  throw new Error(`The translation catalogues are incomplete:\n  ${problems.join('\n  ')}`);
}

/** The strings of one locale. */
export function messages(locale: Locale): Messages {
  return catalogues[locale];
}

/** `getStaticPaths` of a localised page: the default locale at the root, the others under their prefix. */
export function localeStaticPaths() {
  return locales.map(locale => ({
    params: { locale: locale === defaultLocale ? undefined : locale },
    props: { locale },
  }));
}

/**
 * The locale of a page the server renders, from its `[...locale]` parameter:
 * none is the default locale, a prefix is its locale; anything else (the
 * default's own code included, which is never a prefix) is no page: null.
 */
export function localeOfParam(param: string | undefined): Locale | null {
  if (param === undefined) return defaultLocale;
  return locales.find(locale => locale === param && locale !== defaultLocale) ?? null;
}

/** A catalogue string with its `{placeholders}` filled in. */
export function fill(text: string, values: Record<string, string>): string {
  return text.replace(/\{(\w+)\}/g, (whole, name: string) => values[name] ?? whole);
}
