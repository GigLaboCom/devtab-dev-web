// DEVTAB_MOCK_BACKEND — development only; nothing of this folder is imported by the extension or lands in a build.
//
// The contract between the extension and the licence and plans server, written once as data and checks: the paths,
// the request and answer shapes, the error codes, the token's header and fields, the issuing rules, the plans catalog
// and CORS. The mock server builds its answers with this module and its tests check them with it; the extension's
// future client and the real backend can run their requests and answers through the same checks.
//
// Source (developer documents, reconciled 2026-11-23): the token and the licence endpoints —
// pro-entitlement-implementation.md §3 and §5; the plans endpoint and the catalog — pro-plans-and-payment.md §3 and
// §4; what only the server knows — backend.md §6. dev-mock-backend.md §8 records where each rule was settled.

/** One day in seconds: times on the wire are whole Unix seconds. */
export const DAY = 86_400;

/** The token's spans (issuing rules, pro-entitlement-implementation.md §5.1; values: assumption 5 / Д4). */
export const SPANS = Object.freeze({ check: 7 * DAY, window: 30 * DAY, warnBefore: 7 * DAY, dunning: 14 * DAY });

/** New activations of one licence allowed in 30 days, per seat (assumption Д5; §5.2). */
export const ACTIVATION_CHURN = 3;
export const CHURN_PERIOD = 30 * DAY;

export const LIMITS = Object.freeze({
  /** A token longer than this is `malformed` (§3.1). */
  tokenLength: 4096,
  /** The largest request body of /v1/* (backend.md §6.1); more is 413 `payload-too-large`. */
  requestBytes: 4 * 1024,
  /** The largest answer the extension reads: licence endpoints and the plans list (§5.1; plans §4.3). */
  licenceAnswerBytes: 16 * 1024,
  plansAnswerBytes: 32 * 1024,
  /** The extension's request timeout, ms (§5.1). */
  clientTimeoutMs: 10_000,
  /** Retry-After the extension honours, seconds (§5.1). */
  retryAfterMin: 60,
  retryAfterMax: 86_400,
  /** maxAge the extension keeps, seconds (plans §3.2, §4.6). */
  maxAgeMin: 3_600,
  maxAgeMax: 604_800,
});

export const ENDPOINTS = Object.freeze({
  plans: Object.freeze({ method: 'GET', path: '/v1/plans', fields: Object.freeze([]) }),
  activate: Object.freeze({ method: 'POST', path: '/v1/activate', fields: Object.freeze(['key', 'inst']) }),
  refresh: Object.freeze({ method: 'POST', path: '/v1/refresh', fields: Object.freeze(['lid', 'inst', 'secret']) }),
  deactivate: Object.freeze({
    method: 'POST',
    path: '/v1/deactivate',
    fields: Object.freeze(['lid', 'inst', 'secret']),
  }),
});

/** The endpoint at `path`, or undefined. */
export const endpointAt = path => Object.entries(ENDPOINTS).find(([, e]) => e.path === path)?.[0];

/** Error codes and their HTTP status (the table of §5.1). */
export const ERRORS = Object.freeze({
  'bad-request': 400,
  'key-blocked': 403,
  'licence-inactive': 403,
  'key-unknown': 404,
  'seat-limit': 409,
  'payload-too-large': 413,
  'rate-limited': 429,
  unavailable: 503,
});

/** Which codes each endpoint may answer besides its success (§5.1–§5.4; plans §4.3). */
export const ENDPOINT_ERRORS = Object.freeze({
  activate: Object.freeze(Object.keys(ERRORS)),
  refresh: Object.freeze(['bad-request', 'payload-too-large', 'rate-limited', 'unavailable']),
  deactivate: Object.freeze(['bad-request', 'payload-too-large', 'rate-limited', 'unavailable']),
  plans: Object.freeze(['rate-limited', 'unavailable']),
});

/** English texts for the `message` field of an error body (§5.1); the extension reads only `error`. */
export const ERROR_MESSAGES = Object.freeze({
  'bad-request': 'The request does not follow the contract.',
  'key-blocked': 'This licence key is blocked.',
  'licence-inactive': 'This licence is not active: the subscription ended or the purchase was refunded.',
  'key-unknown': 'This licence key is not known.',
  'seat-limit': 'This licence is active in as many browsers as it allows.',
  'payload-too-large': 'The request body is too large.',
  'rate-limited': 'Too many requests; try again later.',
  unavailable: 'The server could not answer in time.',
});

export const STATUSES = Object.freeze(['active', 'ended', 'revoked']);
export const REVOKE_REASONS = Object.freeze([
  'refund',
  'deactivated',
  'key-blocked',
  'seat-limit',
  'unknown-installation',
]);
export const TERMS = Object.freeze(['month', 'year']);

export const PATTERNS = Object.freeze({
  /** The key as the extension sends it: 16–128 printable ASCII characters without spaces (§5.2, §6.5). */
  key: /^[\x21-\x7e]{16,128}$/,
  /** The canonical key the server looks up (backend.md §5.2). */
  canonicalKey: /^DTD[0-9A-HJKMNP-TV-Z]{20}$/,
  inst: /^[A-Za-z0-9_-]{22}$/,
  lid: /^[A-Za-z0-9_-]{8,64}$/,
  secret: /^[A-Za-z0-9_-]{32,128}$/,
  kid: /^[a-z0-9-]{1,32}$/,
  catalog: /^[A-Za-z0-9._-]{1,40}$/,
  planId: /^[a-z0-9-]{1,32}$/,
  currency: /^[A-Z]{3}$/,
});

/** The server's canonical form of a key (backend.md §5.2), or undefined when it is not a key of that format. */
export function canonicalKey(raw) {
  if (typeof raw !== 'string') return undefined;
  const k = raw.toUpperCase().replace(/[\s-]/g, '').replace(/O/g, '0').replace(/[IL]/g, '1');
  return PATTERNS.canonicalKey.test(k) ? k : undefined;
}

// ---------------------------------------------------------------------------------------------------------------------
// Requests

const isObject = v => typeof v === 'object' && v !== null && !Array.isArray(v);

/**
 * A request body of `endpoint` (an object already parsed from JSON): exactly the named fields, each by its rule.
 * Returns the problems; empty means the server must accept it as well-formed.
 */
export function requestProblems(endpoint, body) {
  const spec = ENDPOINTS[endpoint];
  if (!spec) return [`unknown endpoint ${endpoint}`];
  if (!isObject(body)) return ['the body is not a JSON object'];
  const problems = [];
  for (const name of Object.keys(body)) if (!spec.fields.includes(name)) problems.push(`unexpected field ${name}`);
  for (const name of spec.fields) {
    const value = body[name];
    if (typeof value !== 'string') problems.push(`${name} is missing or not a string`);
    else if (!PATTERNS[name].test(value)) problems.push(`${name} breaks its rule`);
  }
  return problems;
}

// ---------------------------------------------------------------------------------------------------------------------
// Answers

/** The headers every answer of the licence endpoints carries, besides CORS (§5.1); the plans' are below. */
export const ANSWER_HEADERS = Object.freeze({ 'content-type': 'application/json', 'cache-control': 'no-store' });

/** An error body: `{ error, message? }` with a code this endpoint may answer at this status. */
export function errorProblems(endpoint, status, body) {
  if (!isObject(body)) return ['the error body is not a JSON object'];
  const problems = [];
  if (!ENDPOINT_ERRORS[endpoint]?.includes(body.error)) problems.push(`error ${body.error} is not one of ${endpoint}`);
  // 429 is always rate-limited and 503 always unavailable (with or without Retry-After, §5.1).
  if (status !== ERRORS[body.error])
    problems.push(`error ${body.error} came with ${status}, not ${ERRORS[body.error]}`);
  if ('message' in body && typeof body.message !== 'string') problems.push('message is not a string');
  return problems;
}

/** A successful answer's body (the token inside is checked separately with jws.verifyToken). */
export function answerProblems(endpoint, status, body) {
  if (endpoint === 'deactivate') {
    // Always 204 without a body (§5.4); the extension does not read the answer at all.
    if (status !== 204) return [`deactivate answered ${status}, not 204`];
    return body === undefined || body === '' ? [] : ['204 with a body'];
  }
  if (status !== 200) return [`${endpoint} answered ${status}`];
  if (!isObject(body)) return ['the body is not a JSON object'];
  const problems = [];
  if (typeof body.token !== 'string' || body.token.length === 0) problems.push('token is missing');
  else if (body.token.length > LIMITS.tokenLength) problems.push('token is longer than 4096');
  if (endpoint === 'activate' && !(typeof body.secret === 'string' && PATTERNS.secret.test(body.secret)))
    problems.push('secret is missing or breaks its rule');
  return problems;
}

// ---------------------------------------------------------------------------------------------------------------------
// The token (§3; the issuing rules and their table of cases: §5.1)

export const TOKEN_TYPE = 'devtab-licence';

/** The header: exactly alg, kid, typ. The reason the extension gives for a broken one is `unsupported`. */
export function headerProblems(header) {
  if (!isObject(header)) return ['the header is not an object'];
  const problems = [];
  const names = Object.keys(header).sort().join(',');
  if (names !== 'alg,kid,typ') problems.push(`header fields are ${names}, not alg,kid,typ`);
  if (header.alg !== 'ES256') problems.push(`alg is ${header.alg}`);
  if (header.typ !== TOKEN_TYPE) problems.push(`typ is ${header.typ}`);
  if (typeof header.kid !== 'string' || !PATTERNS.kid.test(header.kid)) problems.push('kid breaks its rule');
  return problems;
}

const isSeconds = v => Number.isInteger(v) && v >= 0;

/**
 * The token's fields as on the wire (seconds). `unsupported` when `v` is a number other than 1, `claims` for any other
 * break of §3.2. `term` and `renews` are optional; a wrong type is "not read" by the extension (§3.2) — reported in
 * `ignored` — and a problem only with `strict` (what the server must never send).
 */
export function claimsCheck(claims, { strict = false } = {}) {
  if (!isObject(claims)) return { reason: 'claims', problems: ['the payload is not an object'], ignored: [] };
  if (typeof claims.v === 'number' && claims.v !== 1)
    return { reason: 'unsupported', problems: [`v is ${claims.v}`], ignored: [] };
  const problems = [];
  const ignored = [];
  if (claims.v !== 1) problems.push('v is missing');
  if (typeof claims.lid !== 'string' || !PATTERNS.lid.test(claims.lid)) problems.push('lid breaks its rule');
  if (typeof claims.inst !== 'string' || !PATTERNS.inst.test(claims.inst)) problems.push('inst breaks its rule');
  if (claims.plan !== 'pro') problems.push(`plan is ${claims.plan}`);
  if (!STATUSES.includes(claims.status)) problems.push(`status is ${claims.status}`);
  if (claims.status === 'revoked') {
    if (!REVOKE_REASONS.includes(claims.reason)) problems.push('a revoked token without a known reason');
  } else if ('reason' in claims) problems.push('reason on a token that is not revoked');
  for (const name of ['iat', 'chk', 'warn', 'exp'])
    if (!isSeconds(claims[name])) problems.push(`${name} is not whole seconds`);
  if (!(claims.paid === null || isSeconds(claims.paid))) problems.push('paid is neither null nor whole seconds');
  if (problems.length === 0) {
    const { iat, chk, warn, exp, status } = claims;
    if (status === 'active' && !(iat <= chk && chk <= warn && warn <= exp)) problems.push('not iat ≤ chk ≤ warn ≤ exp');
    if (status !== 'active' && !(warn === exp && exp === iat && iat <= chk))
      problems.push('not warn = exp = iat ≤ chk');
  }
  if ('term' in claims && !TERMS.includes(claims.term)) ignored.push('term');
  if ('renews' in claims && typeof claims.renews !== 'boolean') ignored.push('renews');
  if (strict) for (const name of ignored) problems.push(`${name} has a wrong type`);
  return { reason: problems.length ? 'claims' : undefined, problems, ignored };
}

/** The end of what was paid, with the payment's grace for a subscription (§5.1): seconds, or null without one. */
export const paidEnd = licence =>
  licence.paid === null || licence.paid === undefined ? null : licence.paid + (licence.term ? SPANS.dunning : 0);

/**
 * What the server signs (§5.1, the issuing rules): the spans from `iat` and the licence. `licence` is
 * `{ status, reason?, paid: seconds | null, term?: 'month' | 'year', renews?: boolean }`; a licence with `paid` and
 * without `term` is a free (complimentary) licence with an end date. An `active` licence whose paid period and its
 * grace are over at `iat` is signed `ended`: an `active` token always has iat < chk ≤ warn ≤ exp.
 */
export function issueClaims({ lid, inst, iat, licence }) {
  const base = { v: 1, lid, inst, plan: 'pro' };
  if (licence.status === 'revoked')
    return {
      ...base,
      status: 'revoked',
      reason: licence.reason,
      iat,
      chk: iat,
      warn: iat,
      exp: iat,
      paid: licence.paid ?? null,
    };
  const end = paidEnd(licence);
  if (licence.status === 'ended' || (end !== null && end <= iat))
    return {
      ...base,
      status: 'ended',
      iat,
      chk: iat + SPANS.check,
      warn: iat,
      exp: iat,
      paid: licence.paid ?? null,
      ...(licence.term ? { term: licence.term } : {}),
      renews: false,
    };
  if (end === null) {
    const exp = iat + SPANS.window;
    return { ...base, status: 'active', iat, chk: iat + SPANS.check, warn: exp - SPANS.warnBefore, exp, paid: null };
  }
  const exp = Math.min(iat + SPANS.window, end);
  const chk = Math.min(iat + SPANS.check, exp);
  const warn = Math.max(exp - SPANS.warnBefore, chk);
  return {
    ...base,
    status: 'active',
    iat,
    chk,
    warn,
    exp,
    paid: licence.paid,
    ...(licence.term ? { term: licence.term, renews: licence.renews !== false } : { renews: false }),
  };
}

/** Problems with the issuing rules of §5.1 (on top of claimsCheck): what a server must sign. */
export function issueProblems(claims) {
  const { iat, chk, warn, exp, paid, status } = claims;
  const problems = [];
  if (status === 'revoked') {
    if ('term' in claims || 'renews' in claims) problems.push('term or renews on a revoked token (Б6)');
  } else if (status === 'ended') {
    if (chk !== iat + SPANS.check) problems.push('ended: chk is not iat + 7 days');
    if (claims.renews !== false) problems.push('ended: renews is not false (Б6)');
  } else if (paid === null) {
    if (exp !== iat + SPANS.window || chk !== iat + SPANS.check || warn !== exp - SPANS.warnBefore)
      problems.push('active without paid: spans are not 7 / 23 / 30 days');
    if ('term' in claims || 'renews' in claims) problems.push('term or renews on a licence without paid');
  } else {
    const end = paid + ('term' in claims ? SPANS.dunning : 0);
    if (end <= iat) problems.push('active after the paid period and its grace: must be ended');
    if (exp !== Math.min(iat + SPANS.window, end)) problems.push('active: exp is not min(iat + 30 d, paid + 14 d)');
    if (chk !== Math.min(iat + SPANS.check, exp)) problems.push('active: chk is not min(iat + 7 d, exp)');
    if (warn !== Math.max(exp - SPANS.warnBefore, chk)) problems.push('active: warn is not max(exp − 7 d, chk)');
  }
  return problems;
}

// ---------------------------------------------------------------------------------------------------------------------
// The plans catalog (pro-plans-and-payment.md §3.2, §3.5, §3.6, §4.7; backend.md §6.2.4)

/**
 * XTS, the code ISO 4217 reserves for tests, which every placeholder price uses. Neither Node 22 nor Chrome lists it
 * in Intl.supportedValuesOf('currency'), so a real price is held to that list alone and the test currency is allowed
 * only where a build's licence configuration names it (`testCurrencies` of the `test` and `mock` configurations,
 * pro-entitlement-implementation.md §6.2; plans §3.5).
 */
export const TEST_CURRENCY = 'XTS';

/** The currencies a real price may use: exactly what Intl lists (a production configuration). */
export const realCurrencies = () => new Set(Intl.supportedValuesOf('currency'));

/** The currencies of a development or test configuration: what Intl lists, plus the test currency. */
export const testCurrencies = () => new Set([...Intl.supportedValuesOf('currency'), TEST_CURRENCY]);

// eslint-disable-next-line no-control-regex
const FORBIDDEN_TEXT = /[\u{0}-\u{1f}\u{7f}-\u{9f}\u{202a}-\u{202e}\u{2066}-\u{2069}]/u;

/** A text field: undefined when fine, else the reason. Lengths count Unicode code points of the trimmed text. */
function textProblem(value, { max, min = 1, nullable }) {
  if (value === null) return nullable ? undefined : 'null';
  if (typeof value !== 'string') return 'not a string';
  if (FORBIDDEN_TEXT.test(value)) return 'control or bidi character';
  const length = [...value.trim()].length;
  if (length === 0) return nullable ? undefined : 'empty';
  if (length < min || length > max) return `length ${length} outside ${min}…${max}`;
  return undefined;
}

/** An address a page may open: https, no credentials, no port, exactly one of `payHosts`, at most 300 code points. */
export function payUrlProblem(value, payHosts) {
  if (typeof value !== 'string') return 'not a string';
  if ([...value].length > 300) return 'longer than 300';
  let url;
  try {
    url = new URL(value);
  } catch {
    return 'not a URL';
  }
  if (url.protocol !== 'https:') return 'not https';
  if (url.username || url.password) return 'carries credentials';
  if (url.port) return 'carries a port';
  if (!payHosts.includes(url.hostname)) return `host ${url.hostname} is not allowed`;
  return undefined;
}

/** One plan: the DropReason the extension would give (plans §3.6), with the detail, or undefined when it shows. */
function planDrop(plan, payHosts, currencies) {
  if (!isObject(plan)) return ['shape', 'not an object'];
  if (typeof plan.id !== 'string' || !PATTERNS.planId.test(plan.id)) return ['shape', 'id breaks its rule'];
  if (plan.kind !== 'free' && plan.kind !== 'paid') return ['kind', `kind ${plan.kind}`];
  if (plan.kind === 'free' && plan.term !== null) return ['term', 'a free plan with a term'];
  if (plan.kind === 'paid' && !TERMS.includes(plan.term)) return ['term', `term ${plan.term}`];
  for (const [name, rule] of [
    ['name', { max: 32 }],
    ['summary', { max: 140, nullable: true }],
    ['priceNote', { max: 60, nullable: true }],
    ['badge', { max: 24, nullable: true }],
  ]) {
    const why = textProblem(plan[name], rule);
    if (why) return ['text', `${name}: ${why}`];
  }
  if (!Array.isArray(plan.includes) || plan.includes.length > 8) return ['shape', 'includes is not 0–8 lines'];
  for (const line of plan.includes) {
    const why = textProblem(line, { max: 100 });
    if (why) return ['text', `includes: ${why}`];
  }
  if (plan.kind === 'free') {
    if (plan.price !== null) return ['price', 'a free plan with a price'];
    if (plan.checkoutUrl !== null) return ['shape', 'a free plan with a checkout address'];
  } else {
    const p = plan.price;
    if (!isObject(p) || !Number.isInteger(p.amount) || p.amount < 0 || p.amount > 100_000_000)
      return ['price', 'amount is not an integer within 0…100 000 000'];
    if (typeof p.currency !== 'string' || !PATTERNS.currency.test(p.currency) || !currencies.has(p.currency))
      return ['currency', `currency ${p.currency}`];
    const why = payUrlProblem(plan.checkoutUrl, payHosts);
    if (why) return ['url', `checkoutUrl: ${why}`];
  }
  if (!(plan.until === null || isSeconds(plan.until))) return ['shape', 'until is neither null nor whole seconds'];
  return undefined;
}

/**
 * The catalog as the extension's parseCatalog would read it (plans §3.6): whether the answer is usable at all, which
 * plans show and which are dropped (with the reason), and what the server should not have sent (`problems`, the
 * server-side rules: every plan valid, maxAge within bounds, manageUrl valid). `currencies` defaults to a production
 * configuration's (realCurrencies); pass testCurrencies() to read as a `test` or `mock` build does.
 */
export function catalogCheck(body, { payHosts, currencies = realCurrencies(), now } = {}) {
  if (!isObject(body)) return { ok: false, why: 'shape', problems: ['not an object'], kept: [], dropped: [] };
  if (typeof body.v === 'number' && body.v !== 1)
    return { ok: false, why: 'unsupported', problems: [`v is ${body.v}`], kept: [], dropped: [] };
  const problems = [];
  if (body.v !== 1) problems.push('v is missing');
  if (typeof body.catalog !== 'string' || !PATTERNS.catalog.test(body.catalog))
    problems.push('catalog breaks its rule');
  if (typeof body.maxAge !== 'number' || !Number.isFinite(body.maxAge)) problems.push('maxAge is not a number');
  if (!Array.isArray(body.plans)) problems.push('plans is not an array');
  // More plans than the cards show is a broken answer as a whole, not a list to cut (plans §3.6).
  else if (body.plans.length > 6) problems.push(`${body.plans.length} plans, more than 6`);
  if (problems.length) return { ok: false, why: 'shape', problems, kept: [], dropped: [] };

  const advisories = [];
  if (!Number.isInteger(body.maxAge) || body.maxAge < LIMITS.maxAgeMin || body.maxAge > LIMITS.maxAgeMax)
    advisories.push(`maxAge ${body.maxAge} outside ${LIMITS.maxAgeMin}…${LIMITS.maxAgeMax} (the extension clamps it)`);
  const manageWhy = body.manageUrl === null ? undefined : payUrlProblem(body.manageUrl, payHosts);
  if (manageWhy) advisories.push(`manageUrl: ${manageWhy} (the extension treats it as null)`);

  const kept = [];
  const dropped = [];
  const seen = new Set();
  body.plans.forEach((plan, index) => {
    const drop =
      planDrop(plan, payHosts, currencies) ?? (seen.has(plan.id) ? ['duplicate', `id ${plan.id}`] : undefined);
    if (drop) {
      dropped.push({ index, why: drop[0], detail: drop[1] });
      advisories.push(`plan ${index}: ${drop[0]} (${drop[1]})`);
    } else {
      seen.add(plan.id);
      if (now !== undefined && plan.until !== null && plan.until <= now)
        advisories.push(`plan ${plan.id}: offer ended`);
      else kept.push(plan.id);
    }
  });
  if (seen.size === 0) return { ok: false, why: 'empty', problems: advisories, kept, dropped };
  return {
    ok: true,
    maxAge: Math.min(Math.max(body.maxAge, LIMITS.maxAgeMin), LIMITS.maxAgeMax),
    manageUrl: manageWhy ? null : body.manageUrl,
    problems: advisories,
    kept,
    dropped,
  };
}

// ---------------------------------------------------------------------------------------------------------------------
// CORS and caching (§5.1; plans §4.3)

/** The licence endpoints' CORS headers when the request's origin is allowed (`allowOrigin`), else only Vary. */
export function licenceCorsHeaders(allowOrigin) {
  if (!allowOrigin) return { vary: 'Origin' };
  return {
    'access-control-allow-origin': allowOrigin,
    'access-control-allow-methods': 'POST',
    'access-control-allow-headers': 'Content-Type',
    'access-control-max-age': '86400',
    vary: 'Origin',
  };
}

/** The plans list is public: any origin, never credentials. */
export const PLANS_CORS_HEADERS = Object.freeze({ 'access-control-allow-origin': '*' });

/** The answer to a preflight of /v1/plans, which a simple GET never sends but a server still answers (plans §4.3). */
export const PLANS_PREFLIGHT_HEADERS = Object.freeze({
  ...PLANS_CORS_HEADERS,
  'access-control-allow-methods': 'GET',
  'access-control-max-age': '86400',
});

/**
 * The plans' Cache-Control: any HTTP cache revalidates with the ETag every time, so no cache adds its own age to the
 * server's maxAge (plans §4.3, §4.6). The extension asks with cache: 'no-store' and never sees a 304.
 */
export const PLANS_CACHE_CONTROL = 'no-cache';

/** An extension's origin as Chrome sends it: chrome-extension:// and a 32-letter id (a–p). */
export const EXTENSION_ORIGIN = /^chrome-extension:\/\/[a-p]{32}$/;
