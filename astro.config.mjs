// @ts-check
import tailwindcss from '@tailwindcss/vite';
import { defineConfig } from 'astro/config';
import { defaultLocale, locales } from './src/i18n/config.ts';
import { siteUrl } from './src/lib/site.ts';

export default defineConfig({
  site: siteUrl,
  output: 'static',
  // Directory URLs end with a slash (/ru/), as the files are written (ru/index.html).
  trailingSlash: 'always',
  build: { format: 'directory' },

  // The default locale at the root, every other one under its prefix (/ru/).
  i18n: {
    defaultLocale,
    locales: [...locales],
    routing: { prefixDefaultLocale: false },
  },

  vite: {
    plugins: [tailwindcss()],
  },
});
