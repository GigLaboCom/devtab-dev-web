/**
 * The catalogue check, shared by the build (`index.ts` throws on a problem,
 * so a missing translation fails `astro build` and `astro dev`) and by
 * `tests/i18n.test.ts`. Plain TypeScript without Astro or Vite, so Node runs
 * it directly.
 */

type Tree = { [key: string]: string | Tree };

/** Every leaf of a catalogue as `dotted.key → string`. */
export function flatten(tree: Tree, prefix = ''): Map<string, string> {
  const leaves = new Map<string, string>();
  for (const [key, value] of Object.entries(tree)) {
    const path = prefix ? `${prefix}.${key}` : key;
    if (typeof value === 'string') leaves.set(path, value);
    else for (const [leaf, text] of flatten(value, path)) leaves.set(leaf, text);
  }
  return leaves;
}

const placeholders = (text: string) => [...text.matchAll(/\{(\w+)\}/g)].map(match => match[1]).sort();

/**
 * What is wrong with the catalogues, one line per problem: a locale without
 * a catalogue or a catalogue without a locale, a key the source has and a
 * translation lacks (or the other way round), an empty string, and a
 * `{placeholder}` that differs from the source's.
 */
export function catalogueProblems(
  catalogues: Record<string, object>,
  locales: readonly string[],
  source: string,
): string[] {
  const problems: string[] = [];
  for (const locale of locales) {
    if (!catalogues[locale]) problems.push(`${locale}: no catalogue (src/i18n/messages/${locale}.ts)`);
  }
  for (const locale of Object.keys(catalogues)) {
    if (!locales.includes(locale)) problems.push(`${locale}: a catalogue for a locale not in src/i18n/config.ts`);
  }
  const reference = flatten(catalogues[source] as Tree);
  for (const locale of locales) {
    if (!catalogues[locale]) continue;
    const leaves = flatten(catalogues[locale] as Tree);
    for (const [key, text] of reference) {
      const translated = leaves.get(key);
      if (translated === undefined) problems.push(`${locale}: missing ${key}`);
      else if (!translated.trim()) problems.push(`${locale}: empty ${key}`);
      else if (placeholders(translated).join() !== placeholders(text).join()) {
        problems.push(
          `${locale}: ${key} has placeholders {${placeholders(translated)}}, ${source} has {${placeholders(text)}}`,
        );
      }
    }
    for (const key of leaves.keys()) {
      if (!reference.has(key)) problems.push(`${locale}: ${key} is not in ${source}`);
    }
  }
  return problems;
}
