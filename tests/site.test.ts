/**
 * The built site's static part (dist/client/, after `pnpm build`): every page
 * in every locale with the right language, canonical URL and alternates, a
 * sitemap of exactly those pages and the server's indexed ones, internal
 * links that reach a file or a page of the server, nothing loaded from
 * another host, and the content security policy on every page. The server's
 * pages themselves are tested on the running server, in server.test.ts.
 */
import assert from 'node:assert/strict';
import { existsSync, readdirSync, readFileSync, statSync } from 'node:fs';
import { join, relative } from 'node:path';
import test from 'node:test';
import { defaultLocale, locales } from '../src/i18n/config.ts';
import { pages } from '../src/lib/pages.ts';
import { serverPages } from '../src/lib/serverPages.ts';
import { siteUrl } from '../src/lib/site.ts';
import { cspProblems } from './csp.ts';
import { decode, tags } from './html.ts';

const dist = new URL('../dist/client/', import.meta.url).pathname;
const origin = new URL(siteUrl).origin;

/** The address a page is served at: the default locale at the root, the others under their prefix. */
const pathOf = (locale: string, path: string) =>
  `${locale === defaultLocale ? '' : `/${locale}`}/${path}`.replace(/\/?$/, '/');
const urlOf = (locale: string, path: string) => new URL(pathOf(locale, path), origin).href;
const fileOf = (locale: string, path: string) => join(dist, pathOf(locale, path), 'index.html');

function files(directory: string, extension: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return files(path, extension);
    return entry.name.endsWith(extension) ? [path] : [];
  });
}

/** The paths of the server's pages, in every locale. */
const serverPaths = new Set(
  Object.values(serverPages).flatMap(page => locales.map(locale => pathOf(locale, page.path))),
);

/** The file a site path is served from, if the build has one, or the server's page at that path. */
function served(path: string): string | undefined {
  const clean = decodeURIComponent(path.replace(/[?#].*$/, ''));
  if (serverPaths.has(clean)) return clean;
  return [join(dist, clean), join(dist, clean, 'index.html')].find(file => existsSync(file) && statSync(file).isFile());
}

test('the build exists', () => {
  assert.ok(existsSync(join(dist, 'index.html')), 'run pnpm build first');
});

test('every page exists in every locale, with its language, canonical URL and alternates', () => {
  for (const path of Object.values(pages)) {
    for (const locale of locales) {
      const file = fileOf(locale, path);
      assert.ok(existsSync(file), `${relative(dist, file)} is missing`);
      const html = readFileSync(file, 'utf8');
      const where = relative(dist, file);

      assert.equal(tags(html, 'html')[0]?.get('lang'), locale, `${where}: <html lang>`);
      assert.match(html, /<title>[^<]+<\/title>/, `${where}: title`);
      assert.equal((html.match(/<h1[\s>]/g) ?? []).length, 1, `${where}: one h1`);

      const links = tags(html, 'link');
      const canonical = links.filter(link => link.get('rel') === 'canonical').map(link => link.get('href'));
      assert.deepEqual(canonical, [urlOf(locale, path)], `${where}: canonical`);
      const alternates = links
        .filter(link => link.get('rel') === 'alternate' && link.has('hreflang'))
        .map(link => [link.get('hreflang'), link.get('href')]);
      assert.deepEqual(
        alternates,
        [...locales.map(other => [other, urlOf(other, path)]), ['x-default', urlOf(defaultLocale, path)]],
        `${where}: hreflang alternates`,
      );
    }
  }
});

test('the language switch links the same page in every locale and marks the current one', () => {
  for (const path of Object.values(pages)) {
    for (const locale of locales) {
      const html = readFileSync(fileOf(locale, path), 'utf8');
      const links = tags(html, 'a').filter(link => link.has('hreflang'));
      assert.deepEqual(
        links.map(link => [link.get('hreflang'), link.get('href'), link.get('aria-current')]),
        locales.map(other => [other, pathOf(other, path), other === locale ? 'page' : undefined]),
        `${relative(dist, fileOf(locale, path))}: language switch`,
      );
    }
  }
});

test('every built page is a listed page, apart from the 404', () => {
  const expected = new Set(Object.values(pages).flatMap(path => locales.map(locale => fileOf(locale, path))));
  const unlisted = files(dist, '.html').filter(file => !expected.has(file) && relative(dist, file) !== '404.html');
  assert.deepEqual(
    unlisted.map(file => relative(dist, file)),
    [],
    'add these pages to src/lib/pages.ts',
  );
});

test('the sitemap lists every localised URL once, with all its alternates', () => {
  const xml = readFileSync(join(dist, 'sitemap.xml'), 'utf8');
  const entries = [...xml.matchAll(/<url>([\s\S]*?)<\/url>/g)].map(([, body]) => ({
    loc: decode(body.match(/<loc>([^<]*)<\/loc>/)?.[1] ?? ''),
    alternates: [...body.matchAll(/hreflang="([^"]*)" href="([^"]*)"/g)].map(([, lang, href]) => [lang, decode(href)]),
  }));
  const indexed = Object.values(serverPages)
    .filter(page => page.index)
    .map(page => page.path);
  const expected = [...Object.values(pages), ...indexed].flatMap(path =>
    locales.map(locale => ({
      loc: urlOf(locale, path),
      alternates: [...locales.map(other => [other, urlOf(other, path)]), ['x-default', urlOf(defaultLocale, path)]],
    })),
  );
  assert.deepEqual(entries, expected);
});

test('robots.txt points at the sitemap', () => {
  assert.match(readFileSync(join(dist, 'robots.txt'), 'utf8'), new RegExp(`^Sitemap: ${origin}/sitemap\\.xml$`, 'm'));
});

test('the 404 page is not indexed and answers in the default locale', () => {
  const html = readFileSync(join(dist, '404.html'), 'utf8');
  assert.equal(tags(html, 'html')[0]?.get('lang'), defaultLocale);
  assert.ok(
    tags(html, 'meta').some(meta => meta.get('name') === 'robots' && /noindex/.test(meta.get('content') ?? '')),
  );
  assert.ok(!tags(html, 'link').some(link => link.get('rel') === 'canonical'), 'no canonical URL');
});

test('internal links and references reach a file of the build', () => {
  const broken: string[] = [];
  for (const file of files(dist, '.html')) {
    const html = readFileSync(file, 'utf8');
    const ids = new Set(tags(html, '[a-z][a-z0-9-]*').flatMap(tag => (tag.has('id') ? [tag.get('id')] : [])));
    for (const [name, attribute] of [
      ['a', 'href'],
      ['link', 'href'],
      ['script', 'src'],
      ['img', 'src'],
      ['source', 'src'],
    ]) {
      for (const tag of tags(html, name)) {
        const value = tag.get(attribute);
        if (!value) continue;
        const url = new URL(value, new URL(relative(dist, file), `${origin}/`));
        if (url.origin !== origin) continue; // another host: the next test
        if (value.startsWith('#')) {
          if (!ids.has(value.slice(1))) broken.push(`${relative(dist, file)} → ${value}`);
        } else if (!served(url.pathname)) {
          // Canonical and alternate URLs are absolute (the site's own host): their page must be built too.
          broken.push(`${relative(dist, file)} → ${value}`);
        }
      }
    }
  }
  assert.deepEqual(broken, []);
});

test('the built pages load nothing from another host', () => {
  const foreign: string[] = [];
  const absolute = /^(?:[a-z][a-z0-9+.-]*:)?\/\//i;
  for (const file of files(dist, '.html')) {
    const html = readFileSync(file, 'utf8');
    const where = relative(dist, file);
    for (const [, name, attributes] of html.matchAll(/<([a-z][a-z0-9-]*)\b([^>]*)>/gi)) {
      for (const [, key, raw] of attributes.matchAll(/([\w:-]+)="([^"]*)"/g)) {
        const value = decode(raw);
        if (!['src', 'href', 'srcset', 'action', 'formaction', 'poster', 'data', 'content'].includes(key)) continue;
        for (const candidate of value.split(/,\s*/).map(part => part.trim().split(/\s+/)[0])) {
          if (!absolute.test(candidate)) continue;
          if (new URL(candidate, origin).origin !== origin) foreign.push(`${where}: <${name} ${key}="${value}">`);
        }
      }
    }
    for (const [, url] of html.matchAll(/url\(\s*['"]?([^'")]+)/g)) {
      if (absolute.test(url)) foreign.push(`${where}: url(${url})`);
    }
  }
  for (const file of [...files(dist, '.css'), ...files(dist, '.js')]) {
    const text = readFileSync(file, 'utf8').replace(/\/\*[\s\S]*?\*\//g, '');
    for (const [reference] of text.matchAll(
      /(?:url\(\s*['"]?|@import\s+['"]|import\(\s*['"`]|from\s*['"`]|fetch\(\s*['"`])(?:[a-z][a-z0-9+.-]*:)?\/\/[^'"`)\s]+/gi,
    )) {
      foreign.push(`${relative(dist, file)}: ${reference}`);
    }
  }
  assert.deepEqual(foreign, []);
});

test('every page carries the content security policy, and nothing on it breaks the policy', () => {
  const problems = files(dist, '.html').flatMap(file =>
    cspProblems(readFileSync(file, 'utf8')).map(problem => `${relative(dist, file)}: ${problem}`),
  );
  assert.deepEqual(problems, []);
});

test('the policy check catches a missing directive, another host, a style attribute and an unhashed script', () => {
  const page = (policy: string, body = '') =>
    `<html><head><meta http-equiv="content-security-policy" content="${policy}"></head><body>${body}</body></html>`;
  const good = "default-src 'self';form-action 'self';object-src 'none';base-uri 'self'; style-src 'self'";
  assert.deepEqual(cspProblems(page(good, '<p>x</p>')), []);
  assert.equal(cspProblems('<html></html>').length, 1);
  assert.equal(cspProblems(page(good.replace("form-action 'self';", ''))).length, 1);
  assert.equal(cspProblems(page(`${good}; img-src https://cdn.example.invalid`)).length, 1);
  assert.equal(cspProblems(page(good, '<p style="color: red">x</p>')).length, 1);
  assert.equal(cspProblems(page(good, '<script>alert(1)</script>')).length, 1);
});
