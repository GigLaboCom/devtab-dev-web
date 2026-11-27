/**
 * The site's server holds no secret: of the environment it reads only public
 * values (the API's internal address, the Supabase project's address and
 * publishable key, the site's address) and a rate limit, and nothing of it
 * names Supabase's secret key, which only the API may ever hold.
 */
import assert from 'node:assert/strict';
import { readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import test from 'node:test';
import { authConfig } from '../src/lib/auth/config.ts';

const root = new URL('../', import.meta.url).pathname;

/** What the site may read from its environment. */
const allowed = new Set([
  'DEVTAB_API_INTERNAL_URL',
  'DEVTAB_SUPABASE_URL',
  'DEVTAB_SUPABASE_PUBLISHABLE_KEY',
  'DEVTAB_RATE_SIGN_IN_PER_IP',
  'DEVTAB_EXTENSION_IDS',
  'SITE_URL',
  // Astro's own, set at build time.
  'DEV',
  'PROD',
  'BASE_URL',
]);

// Made at run time, so that the secret scan does not take the canary for a key.
const canary = ['sb', 'secret', 'canary', 'not', 'for', 'the', 'site'].join('_');
const secretNames = /SUPABASE_SECRET|SERVICE_ROLE|service_role|sb_secret_/;

function sources(directory: string): string[] {
  return readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) return sources(path);
    return /\.(ts|astro|mjs)$/.test(entry.name) && !entry.name.endsWith('.gen.ts') ? [path] : [];
  });
}

const siteFiles = [...sources(join(root, 'src')), join(root, 'astro.config.mjs')];

test('the site reads only public values and a rate limit from its environment', () => {
  const read = new Set(
    siteFiles.flatMap(file =>
      [...readFileSync(file, 'utf8').matchAll(/(?:process\.env|import\.meta\.env)\.([A-Za-z_]\w*)/g)].map(
        ([, name]) => name,
      ),
    ),
  );
  assert.deepEqual(
    [...read].filter(name => !allowed.has(name)),
    [],
  );
  assert.ok(read.has('DEVTAB_SUPABASE_PUBLISHABLE_KEY'), 'the scan finds what the site reads');
});

test("no code of the site names Supabase's secret key", () => {
  assert.deepEqual(
    siteFiles.filter(file => secretNames.test(readFileSync(file, 'utf8'))).map(file => file.slice(root.length)),
    [],
  );
});

test('sign-in takes the publishable key, never a secret one that the process happens to have', () => {
  const saved = { ...process.env };
  try {
    process.env.DEVTAB_SUPABASE_URL = 'http://127.0.0.1:54332/';
    process.env.DEVTAB_SUPABASE_PUBLISHABLE_KEY = 'sb_publishable_example';
    process.env.DEVTAB_SUPABASE_SECRET_KEY = canary;
    process.env.SUPABASE_SERVICE_ROLE_KEY = canary;
    assert.deepEqual(authConfig(), { url: 'http://127.0.0.1:54332', key: 'sb_publishable_example' });
    delete process.env.DEVTAB_SUPABASE_PUBLISHABLE_KEY;
    assert.equal(authConfig(), null, 'without the publishable key sign-in is off');
  } finally {
    process.env = saved;
  }
});

test("the built site holds no secret key's name or value", () => {
  const built = (directory: string): string[] =>
    readdirSync(directory, { withFileTypes: true }).flatMap(entry => {
      const path = join(directory, entry.name);
      return entry.isDirectory() ? built(path) : /\.(mjs|js|html|css)$/.test(entry.name) ? [path] : [];
    });
  const files = built(join(root, 'dist'));
  assert.ok(
    files.some(file => file.endsWith('entry.mjs')),
    'run pnpm build first',
  );
  assert.deepEqual(
    files
      .filter(file => /sb_secret_|service_role/.test(readFileSync(file, 'utf8')))
      .map(file => file.slice(root.length)),
    [],
  );
});
