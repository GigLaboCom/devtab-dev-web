/**
 * The catalogues and the templates, from the sources (no build needed):
 * every locale has every string, and no template carries text of its own.
 */
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { defaultLocale, locales } from '../src/i18n/config.ts';
import { catalogueProblems } from '../src/i18n/validate.ts';
import { literalText } from './astro-text.ts';

const src = new URL('../src/', import.meta.url).pathname;
const messagesDir = join(src, 'i18n/messages');

async function loadCatalogues(): Promise<Record<string, object>> {
  const catalogues: Record<string, object> = {};
  for (const file of readdirSync(messagesDir).filter(name => name.endsWith('.ts'))) {
    const module = (await import(join(messagesDir, file))) as { default: object };
    catalogues[file.slice(0, -3)] = module.default;
  }
  return catalogues;
}

function astroFiles(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return astroFiles(path);
    return entry.name.endsWith('.astro') ? [path] : [];
  });
}

test('every locale has a catalogue with every string of the default locale', async () => {
  assert.deepEqual(catalogueProblems(await loadCatalogues(), locales, defaultLocale), []);
});

test('the catalogue check reports a missing, an extra, an empty and a mismatched string', () => {
  const en = { a: 'A', b: { c: 'Hello, {name}', d: 'D' } };
  const xx = { a: ' ', b: { c: 'Hallo', e: 'E' } };
  assert.deepEqual(catalogueProblems({ en, xx }, ['en', 'xx', 'yy'], 'en'), [
    'yy: no catalogue (src/i18n/messages/yy.ts)',
    'xx: empty a',
    'xx: b.c has placeholders {}, en has {name}',
    'xx: missing b.d',
    'xx: b.e is not in en',
  ]);
  assert.deepEqual(catalogueProblems({ en, zz: en }, ['en'], 'en'), [
    'zz: a catalogue for a locale not in src/i18n/config.ts',
  ]);
});

test('templates carry no user-visible text of their own', () => {
  const offenders = astroFiles(src).flatMap(file =>
    literalText(readFileSync(file, 'utf8')).map(text => `${file.slice(src.length)}: ${text}`),
  );
  assert.deepEqual(offenders, [], 'move these strings into src/i18n/messages/');
});

test('the template scan finds text between tags, in markup inside expressions and in visible attributes', () => {
  const source = [
    '---',
    "const items = ['a'];",
    '---',
    '<nav aria-label="Main">',
    '  {items.map(item => (<li title={item}>Item {item}</li>))}',
    '  {items.length > 0 && <p>{items[0]}</p>}',
    '  <img alt="Logo" src="/x.svg" />',
    '</nav>',
    '<p>Hello</p>',
    '<style>p { content: "x"; }</style>',
  ].join('\n');
  assert.deepEqual(literalText(source), ['aria-label="Main"', 'Item', 'alt="Logo"', 'Hello']);
});
