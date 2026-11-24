// The licence contract from the extension's side, run on the site: the tokens this server signed (server-vectors.json,
// written by `cargo test --test contract`) through the copy of the extension's own check and state function (verifier/
// verify.ts and state.ts, copied byte for byte by script/contract-sync), the issued claims against the contract's
// issuing rules (verifier/contract.mjs), and the contract files against the copied rules.
//
//   pnpm test:contract
//
// The extension's modules import each other without file extensions ('./base64url'). Node's own resolve hook
// (node:module registerHooks) adds `.ts` to those specifiers inside verifier/, and Node strips the types; no
// dependency and no change to the copies.
import assert from 'node:assert/strict';
import { createHash } from 'node:crypto';
import { readdirSync, readFileSync } from 'node:fs';
import { registerHooks } from 'node:module';
import { join } from 'node:path';
import { test } from 'node:test';

import { claimsCheck, ENDPOINT_ERRORS, ERRORS, issueClaims, issueProblems, PATTERNS } from './verifier/contract.mjs';

registerHooks({
  resolve(specifier, context, nextResolve) {
    const inVerifier = context.parentURL?.includes('/licence-v1/verifier/') ?? false;
    if (inVerifier && /^\.\.?\//.test(specifier) && !/\.[a-z]+$/.test(specifier)) {
      return nextResolve(`${specifier}.ts`, context);
    }
    return nextResolve(specifier, context);
  },
});
const { verifyToken } = await import('./verifier/verify.ts');
const { evaluate } = await import('./verifier/state.ts');

const here = import.meta.dirname;
const read = (name: string) => readFileSync(join(here, name));
const json = (name: string) => JSON.parse(read(name).toString('utf8'));

interface Vector {
  name: string;
  token: string;
  inst: string;
  now: number;
  seen: number;
  verify: string;
  state: Record<string, unknown>;
  licence?: Record<string, unknown>;
}
interface VectorFile {
  iat?: number;
  keys: { kid: string; jwk: Record<string, string> }[];
  vectors: Vector[];
}

/** The payload of a compact token as signed (seconds on the wire). */
const payloadOf = (token: string) => JSON.parse(Buffer.from(token.split('.')[1], 'base64url').toString('utf8'));

/** What the extension makes of a vector: verifyToken's outcome and evaluate's state (`toMatchObject`, as its test does). */
async function extensionReads(vector: Vector, keys: VectorFile['keys']) {
  const verified = await verifyToken(vector.token, keys);
  assert.equal(verified.ok ? 'ok' : verified.reason, vector.verify, `${vector.name}: verifyToken`);
  const stored = verified.ok
    ? { kind: 'token', claims: verified.claims, inst: vector.inst }
    : { kind: 'rejected', reason: verified.reason };
  const state = evaluate(stored, vector.now, vector.seen);
  for (const [field, value] of Object.entries(vector.state)) {
    assert.deepEqual(state[field], value, `${vector.name}: evaluate ${field}`);
  }
  return verified;
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
  const copies = [...readdirSync(join(here, 'verifier')).map(name => `verifier/${name}`), 'extension-vectors.json'];
  assert.deepEqual([...recorded.keys()].sort(), copies.sort());
  for (const [file, sum] of recorded) {
    assert.equal(createHash('sha256').update(read(file)).digest('hex'), sum, `${file}: run script/contract-sync`);
  }
});

test("every server vector gets its stated outcome and state from the extension's own check", async () => {
  const file: VectorFile = json('server-vectors.json');
  assert.equal(file.vectors.length, 26);
  for (const vector of file.vectors) await extensionReads(vector, file.keys);
});

test("the extension's own vectors read the same through the copies (the copies load and work)", async () => {
  const file: VectorFile = json('extension-vectors.json');
  assert.ok(file.vectors.length >= 8);
  for (const vector of file.vectors) await extensionReads(vector, file.keys);
});

test('the server issues exactly what the contract issuing rules give, case by case', () => {
  const file: VectorFile = json('server-vectors.json');
  const cases = file.vectors.filter(v => v.licence);
  assert.equal(cases.length, 20, 'cases 1–15 and case 16 with each of the five reasons');
  for (const vector of cases) {
    const claims = payloadOf(vector.token);
    assert.equal(claims.iat, file.iat);
    assert.deepEqual(claims, issueClaims({ lid: claims.lid, inst: claims.inst, iat: claims.iat, licence: vector.licence }), vector.name);
    // What a server must sign: no field the extension would ignore, and the issuing rules.
    assert.deepEqual(claimsCheck(claims, { strict: true }).problems, [], vector.name);
    assert.deepEqual(issueProblems(claims), [], vector.name);
  }
});

test('the dates of every active token obey exp = min(iat + 30 d, end), chk = min(iat + 7 d, exp), warn = max(exp − 7 d, chk)', () => {
  const day = 86_400;
  const file: VectorFile = json('server-vectors.json');
  for (const vector of file.vectors.filter(v => v.licence?.status === 'active')) {
    const c = payloadOf(vector.token);
    if (c.status !== 'active') continue;
    const licence = vector.licence as { paid: number | null; term?: string };
    const end = licence.paid === null ? null : licence.paid + (licence.term ? 14 * day : 0);
    const exp = end === null ? c.iat + 30 * day : Math.min(c.iat + 30 * day, end);
    const chk = Math.min(c.iat + 7 * day, exp);
    const warn = end === null ? c.iat + 23 * day : Math.max(exp - 7 * day, chk);
    assert.deepEqual([c.chk, c.warn, c.exp], [chk, warn, exp], vector.name);
    assert.ok(c.iat < c.chk && c.chk <= c.warn && c.warn <= c.exp, vector.name);
  }
});

test('errors.json is the reference error table for the licence and plans endpoints', () => {
  const errors = json('errors.json');
  assert.deepEqual(errors.codes, ERRORS);
  assert.deepEqual(Object.keys(errors.endpoints).sort(), Object.keys(ENDPOINT_ERRORS).sort());
  for (const endpoint of ['activate', 'refresh', 'deactivate', 'plans']) {
    assert.deepEqual(errors.endpoints[endpoint], ENDPOINT_ERRORS[endpoint], endpoint);
  }
});

test('the schemas use the reference field rules', () => {
  const patterns = PATTERNS as Record<string, RegExp>;
  const activate = json('activate.request.schema.json');
  assert.equal(activate.properties.key.pattern, patterns.key.source);
  assert.equal(activate.properties.inst.pattern, patterns.inst.source);
  for (const name of ['refresh.request.schema.json', 'deactivate.request.schema.json']) {
    const schema = json(name);
    for (const field of ['lid', 'inst', 'secret']) {
      assert.equal(schema.properties[field].pattern, patterns[field].source, `${name} ${field}`);
    }
  }
  assert.equal(json('activate.response.schema.json').properties.secret.pattern, patterns.secret.source);
  assert.equal(json('token.header.schema.json').properties.kid.pattern, patterns.kid.source);
  const claims = json('token.claims.schema.json');
  assert.equal(claims.properties.lid.pattern, patterns.lid.source);
  assert.equal(claims.properties.inst.pattern, patterns.inst.source);
});

test('test keys only in the vectors, and never in production-keys.json', () => {
  for (const entry of json('production-keys.json').keys) assert.ok(!entry.kid.startsWith('test-'), entry.kid);
  for (const name of ['server-vectors.json', 'extension-vectors.json']) {
    for (const entry of json(name).keys) assert.ok(entry.kid.startsWith('test-'), `${name}: ${entry.kid}`);
  }
});
