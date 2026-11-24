// The plans contract from the extension's side, run on the site: the answers of GET /v1/plans this server builds
// (server-catalogs.json, written by `cargo test --test plans server_catalogs`) through the copy of the extension's own
// catalogue reader (reader/model.ts, price.ts, plansRecord.ts, copied byte for byte by script/contract-sync), and
// through the contract's catalogue rules (../licence-v1/verifier/contract.mjs).
//
//   pnpm test:contract
//
// The extension's modules import each other without file extensions ('./model') and use TypeScript that Node's
// type stripping alone does not take (a parameter property). Node's own hooks (node:module registerHooks) add `.ts`
// to those specifiers inside reader/ and turn its files into JavaScript with Node's stripTypeScriptTypes in transform
// mode; no dependency and no change to the copies.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
import { registerHooks, stripTypeScriptTypes } from 'node:module';
import { join } from 'node:path';
import { test } from 'node:test';
import { fileURLToPath } from 'node:url';

import { catalogCheck, ENDPOINT_ERRORS, LIMITS, PATTERNS, testCurrencies } from '../licence-v1/verifier/contract.mjs';

const READER = '/plans-v1/reader/';
registerHooks({
  resolve(specifier, context, nextResolve) {
    const inReader = context.parentURL?.includes(READER) ?? false;
    if (inReader && /^\.\.?\//.test(specifier) && !/\.[a-z]+$/.test(specifier)) {
      return nextResolve(`${specifier}.ts`, context);
    }
    return nextResolve(specifier, context);
  },
  load(url, context, nextLoad) {
    if (url.includes(READER) && url.endsWith('.ts')) {
      const source = readFileSync(fileURLToPath(url), 'utf8');
      return { format: 'module', source: stripTypeScriptTypes(source, { mode: 'transform', sourceUrl: url }), shortCircuit: true };
    }
    return nextLoad(url, context);
  },
});
const { parseCatalog, plansAt, intlCurrencies, MAX_PLANS, MAX_INCLUDES, MAX_AMOUNT, TEXT_LIMITS } = await import('./reader/model.ts');
const { formatPrice, periodText } = await import('./reader/price.ts');
const { readPlansRecord, freshness } = await import('./reader/plansRecord.ts');

const here = import.meta.dirname;
const read = (name: string) => readFileSync(join(here, name));
const json = (name: string) => JSON.parse(read(name).toString('utf8'));

interface Expect {
  ok: boolean;
  why?: string;
  kept?: string[];
  dropped?: { index: number; why: string }[];
}
interface Case {
  name: string;
  env: string;
  bytes: number;
  body: { plans: { id: string; price: { amount: number; currency: string } | null; term: string | null }[] } & Record<string, unknown>;
  expect: { test: Expect; production: Expect };
}
const vectors: { payHosts: string[]; now: number; cases: Case[] } = json('server-catalogs.json');
const testRules = { payHosts: vectors.payHosts, currencies: new Set([...intlCurrencies(), 'XTS']) };
const productionRules = { payHosts: vectors.payHosts, currencies: intlCurrencies() };

/** What parseCatalog made of an answer, in the vectors' terms. */
function reading(result: ReturnType<typeof parseCatalog>): Expect {
  if (!result.ok) return { ok: false, why: result.why };
  return {
    ok: true,
    kept: result.catalog.plans.map((plan: { id: string }) => plan.id),
    dropped: result.dropped.map((d: { index: number; why: string }) => ({ index: d.index, why: d.why })),
  };
}

test('the copies match SOURCES.sha256, and every copy is recorded', () => {
  const recorded = new Map(
    read('SOURCES.sha256')
      .toString('utf8')
      .trim()
      .split('\n')
      .map(line => {
        const [sum, file] = line.split(/\s+/);
        return [file, sum] as const;
      }),
  );
  const copies = readdirSync(join(here, 'reader')).map(name => `reader/${name}`);
  assert.deepEqual([...recorded.keys()].sort(), copies.sort());
  for (const [file, sum] of recorded) {
    assert.equal(createHash('sha256').update(read(file)).digest('hex'), sum, `${file}: run script/contract-sync`);
  }
});

test("a test or development build reads every answer of the server whole: nothing dropped", () => {
  assert.equal(vectors.cases.length, 4);
  for (const c of vectors.cases) {
    const result = parseCatalog(c.body, testRules);
    assert.deepEqual(reading(result), c.expect.test, c.name);
    if (result.ok) {
      assert.deepEqual(result.dropped, [], `${c.name}: nothing dropped`);
      assert.equal(result.catalog.plans.length, c.body.plans.length, `${c.name}: every plan kept`);
    }
  }
});

test('a production build refuses the test currency: the server never sends it outside development', () => {
  for (const c of vectors.cases) {
    assert.deepEqual(reading(parseCatalog(c.body, productionRules)), c.expect.production, c.name);
  }
  // What staging and production send carries no test currency at all.
  for (const c of vectors.cases.filter(c => c.env !== 'dev')) {
    for (const plan of c.body.plans) assert.notEqual(plan.price?.currency, 'XTS', `${c.name}: ${plan.id}`);
  }
});

test("the contract's catalogue rules agree: nothing the server should not have sent", () => {
  for (const c of vectors.cases) {
    const check = catalogCheck(c.body, { payHosts: vectors.payHosts, currencies: testCurrencies(), now: vectors.now });
    if (c.expect.test.ok) {
      assert.equal(check.ok, true, c.name);
      assert.deepEqual(check.problems, [], c.name);
      assert.deepEqual(check.kept, c.expect.test.kept, c.name);
    } else {
      assert.deepEqual([check.ok, check.why], [false, c.expect.test.why], c.name);
    }
  }
});

test('every answer is within the 32 KB the extension reads, and its size is the bytes the server sent', () => {
  for (const c of vectors.cases) {
    assert.ok(c.bytes <= LIMITS.plansAnswerBytes, c.name);
    assert.equal(Buffer.byteLength(JSON.stringify(c.body)), c.bytes, c.name);
  }
});

test('the cards show the placeholder prices as XTS 11.11 and XTS 111.10', () => {
  const three = vectors.cases.find(c => c.name === 'three-plans')!;
  const parsed = parseCatalog(three.body, testRules);
  assert.ok(parsed.ok);
  const shown = parsed.catalog.plans
    .filter((plan: { price: unknown }) => plan.price !== null)
    .map((plan: { price: { amount: number; currency: string }; term: 'month' | 'year' }) => `${formatPrice(plan.price)} ${periodText(plan.term)}`);
  assert.deepEqual(
    shown.map((text: string) => text.replace(/\s/gu, ' ')),
    ['XTS 11.11 per month', 'XTS 111.10 per year'],
  );
});

test('the record the worker keeps holds the answer, fresh for maxAge', () => {
  const three = vectors.cases.find(c => c.name === 'three-plans')!;
  const fetchedAt = vectors.now * 1000;
  const record = readPlansRecord({ v: 1, fetchedAt, body: three.body, attempts: { failures: 0 } });
  assert.ok(typeof record === 'object');
  const parsed = parseCatalog(record.body, testRules);
  assert.ok(parsed.ok);
  assert.equal(parsed.catalog.maxAgeMs, 86_400_000);
  assert.equal(freshness(fetchedAt, parsed.catalog.maxAgeMs, fetchedAt + 1_000), 'fresh');
  assert.equal(freshness(fetchedAt, parsed.catalog.maxAgeMs, fetchedAt + 86_400_000), 'stale');
});

test('a limited offer shows until its end', () => {
  const offer = vectors.cases.find(c => c.name === 'limited-offer')!;
  const parsed = parseCatalog(offer.body, testRules);
  assert.ok(parsed.ok);
  const annual = parsed.catalog.plans.find((plan: { id: string }) => plan.id === 'pro-annual');
  assert.ok(annual.until > vectors.now * 1000);
  assert.equal(plansAt(parsed.catalog, vectors.now * 1000).length, 3);
  assert.equal(plansAt(parsed.catalog, annual.until).length, 2);
});

test('the schema of the answer uses the reader limits and the reference field rules', () => {
  const schema = json('plans.response.schema.json');
  const plan = schema.properties.plans.items.properties;
  assert.equal(schema.properties.catalog.pattern, PATTERNS.catalog.source);
  assert.equal(plan.id.pattern, PATTERNS.planId.source);
  assert.equal(plan.price.anyOf[1].properties.currency.pattern, PATTERNS.currency.source);
  assert.equal(schema.properties.plans.maxItems, MAX_PLANS);
  assert.equal(plan.includes.maxItems, MAX_INCLUDES);
  assert.equal(plan.price.anyOf[1].properties.amount.maximum, MAX_AMOUNT);
  assert.equal(plan.name.maxLength, TEXT_LIMITS.name);
  assert.equal(plan.summary.maxLength, TEXT_LIMITS.summary);
  assert.equal(plan.includes.items.maxLength, TEXT_LIMITS.include);
  assert.equal(plan.priceNote.maxLength, TEXT_LIMITS.priceNote);
  assert.equal(plan.badge.maxLength, TEXT_LIMITS.badge);
  assert.equal(schema.properties.maxAge.minimum, LIMITS.maxAgeMin);
  assert.equal(schema.properties.maxAge.maximum, LIMITS.maxAgeMax);
});

test('errors.json lists the codes GET /v1/plans may answer', () => {
  const errors = JSON.parse(readFileSync(join(here, '../licence-v1/errors.json'), 'utf8'));
  assert.deepEqual(errors.endpoints.plans, ENDPOINT_ERRORS.plans);
});
