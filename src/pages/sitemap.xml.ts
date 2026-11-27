/**
 * `/sitemap.xml`: every page of `src/lib/pages.ts` and every indexed page of
 * `src/lib/serverPages.ts` in every locale, each with its alternates in all
 * locales and `x-default` (the default locale). The tests check it against
 * the pages the build actually wrote and the server's pages.
 */
import type { APIRoute } from 'astro';
import { getAbsoluteLocaleUrl } from 'astro:i18n';
import { defaultLocale, locales } from '../i18n';
import { pages } from '../lib/pages';
import { serverPages } from '../lib/serverPages';

const escapeXml = (value: string) =>
  value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');

export const GET: APIRoute = () => {
  const paths = [
    ...Object.values(pages),
    ...Object.values(serverPages)
      .filter(page => page.index)
      .map(page => page.path),
  ];
  const entries = paths.flatMap(path => {
    const alternates = [
      ...locales.map(locale => [locale, getAbsoluteLocaleUrl(locale, path)]),
      ['x-default', getAbsoluteLocaleUrl(defaultLocale, path)],
    ]
      .map(([hreflang, href]) => `    <xhtml:link rel="alternate" hreflang="${hreflang}" href="${escapeXml(href)}"/>`)
      .join('\n');
    return locales.map(locale =>
      ['  <url>', `    <loc>${escapeXml(getAbsoluteLocaleUrl(locale, path))}</loc>`, alternates, '  </url>'].join('\n'),
    );
  });
  const xml = [
    '<?xml version="1.0" encoding="UTF-8"?>',
    '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">',
    ...entries,
    '</urlset>',
    '',
  ].join('\n');
  return new Response(xml, { headers: { 'Content-Type': 'application/xml; charset=utf-8' } });
};
