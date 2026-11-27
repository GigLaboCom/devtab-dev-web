// @ts-check
import node from '@astrojs/node';
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'astro/config';
import { defaultLocale, locales } from './src/i18n/config.ts';
import { createHash } from 'node:crypto';
import { siteUrl } from './src/lib/site.ts';
import { themeScript } from './src/lib/themeScript.ts';

export default defineConfig({
  site: siteUrl,
  // Pages are prerendered, as before; a page that needs the server says so
  // (`export const prerender = false`) and the Node server renders it.
  output: 'static',
  adapter: node({ mode: 'standalone' }),
  // Directory URLs end with a slash (/ru/), as the files are written (ru/index.html).
  trailingSlash: 'always',
  build: { format: 'directory' },

  // The default locale at the root, every other one under its prefix (/ru/).
  i18n: {
    defaultLocale,
    locales: [...locales],
    routing: { prefixDefaultLocale: false },
  },

  security: {
    // The hosts whose `Host` header the server believes when it builds a
    // request's address (the forms' origin check, the sign-in mail's link back):
    // the site's own and the loopback addresses of a local run. Any other host
    // is read as localhost.
    allowedDomains: [
      { hostname: new URL(siteUrl).hostname, protocol: 'https' },
      { hostname: '127.0.0.1', protocol: 'http' },
      { hostname: 'localhost', protocol: 'http' },
    ],
    // Astro hashes its own scripts and styles into the policy; nothing else
    // runs, nothing loads from another host, and forms post only to the site.
    csp: {
      directives: ["default-src 'self'", "form-action 'self'", "object-src 'none'", "base-uri 'self'"],
      // The theme's script in <head> is inline so that it runs before the first paint; it is allowed by its hash.
      scriptDirective: {
        hashes: [`sha256-${createHash('sha256').update(themeScript).digest('base64')}`],
      },
    },
  },

  vite: {
    plugins: [tailwindcss()],
  },
});
