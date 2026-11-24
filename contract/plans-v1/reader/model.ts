/**
 * The Pro plans as the server describes them (`GET /v1/plans`): the catalog's fields, what a valid plan is, and what
 * the extension does with what it does not know — a plan it cannot read is skipped and the rest are shown; a format
 * it cannot read is refused whole. Everything about the plans comes from the server; the build holds no price.
 * No chrome: the pages, the worker and the tests import it, and it never throws.
 */
import type { Term } from '@extension/licence';

export type { Term } from '@extension/licence';

export type PlanKind = 'free' | 'paid';

export interface Price {
  /** In the currency's minor units (cents); an integer ≥ 0. */
  readonly amount: number;
  /** ISO 4217, upper case, one Intl knows. */
  readonly currency: string;
}

export interface Plan {
  readonly id: string;
  readonly kind: PlanKind;
  readonly term: Term | null;
  readonly name: string;
  readonly summary: string | null;
  readonly includes: readonly string[];
  readonly price: Price | null;
  readonly priceNote: string | null;
  readonly badge: string | null;
  readonly checkoutUrl: string | null;
  /** Milliseconds; null — no end. */
  readonly until: number | null;
}

export interface Catalog {
  readonly v: 1;
  readonly catalog: string;
  /** Already clamped to 1 h … 7 days, in milliseconds. */
  readonly maxAgeMs: number;
  readonly manageUrl: string | null;
  /** In the order to show; at least one. */
  readonly plans: readonly Plan[];
}

export type DropReason = 'shape' | 'kind' | 'term' | 'text' | 'price' | 'currency' | 'url' | 'duplicate';

export type CatalogParse =
  | {
      readonly ok: true;
      readonly catalog: Catalog;
      readonly dropped: readonly { readonly index: number; readonly why: DropReason }[];
    }
  | { readonly ok: false; readonly why: 'shape' | 'unsupported' | 'empty' };

export interface CatalogRules {
  /** Hosts a checkout or manage address may point at, from the licence configuration. */
  readonly payHosts: readonly string[];
  /**
   * The currencies a price may use: Intl.supportedValuesOf('currency') plus the configuration's `testCurrencies`
   * (['XTS'] in `test` and `mock`, [] in `production`). Default: the Intl list alone.
   */
  readonly currencies?: ReadonlySet<string>;
}

/** `maxAge` is kept within these bounds (seconds on the wire). */
export const MAX_AGE_MIN_S = 3_600;
export const MAX_AGE_MAX_S = 604_800;
export const MAX_PLANS = 6;
export const MAX_INCLUDES = 8;
export const MAX_AMOUNT = 100_000_000;
export const MAX_URL = 300;
/** Lengths in Unicode code points, of a text after trimming its ends. */
export const TEXT_LIMITS = { name: 32, summary: 140, include: 100, priceNote: 60, badge: 24 } as const;

const CATALOG_ID = /^[A-Za-z0-9._-]{1,40}$/;
const PLAN_ID = /^[a-z0-9-]{1,32}$/;
const CURRENCY = /^[A-Z]{3}$/;
/** Control characters and the characters that steer the direction of text: they can turn a price or an address round. */
// eslint-disable-next-line no-control-regex -- control characters are exactly what this refuses
const FORBIDDEN = /[\u0000-\u001f\u007f-\u009f‪-‮⁦-⁩]/u;

/** The currencies this browser's Intl knows. */
export const intlCurrencies = (): ReadonlySet<string> => {
  try {
    return new Set(Intl.supportedValuesOf('currency'));
  } catch {
    return new Set();
  }
};

type Json = Record<string, unknown>;

const isObject = (value: unknown): value is Json =>
  typeof value === 'object' && value !== null && !Array.isArray(value);
const codePoints = (text: string) => [...text].length;

class Drop {
  constructor(readonly why: DropReason) {}
}

/**
 * A text field: `null` (or empty after trimming) when absent and allowed to be; trimmed otherwise. Throws a Drop for a
 * wrong type ('shape'), a forbidden character or a length out of bounds ('text').
 */
const text = (value: unknown, max: number, optional: boolean): string | null => {
  if (value === null && optional) return null;
  if (typeof value !== 'string') throw new Drop('shape');
  if (FORBIDDEN.test(value)) throw new Drop('text');
  const trimmed = value.trim();
  if (trimmed === '') {
    if (optional) return null;
    throw new Drop('text');
  }
  if (codePoints(trimmed) > max) throw new Drop('text');
  return trimmed;
};

/** An address a card may link to: https, no user or password, no port, a host exactly one of `payHosts`, ≤ 300 code points. */
export const payUrl = (value: string, payHosts: readonly string[]): string | undefined => {
  if (codePoints(value) > MAX_URL || FORBIDDEN.test(value)) return undefined;
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return undefined;
  }
  if (url.protocol !== 'https:' || url.username !== '' || url.password !== '' || url.port !== '') return undefined;
  // URL drops a default port (":443"); the address as it came must not name one at all.
  const authority = /^https:\/\/([^/?#]*)/i.exec(value)?.[1] ?? '';
  if (authority.includes(':') || authority.includes('@')) return undefined;
  if (!payHosts.includes(url.hostname)) return undefined;
  return value;
};

const parsePlan = (raw: unknown, rules: CatalogRules, currencies: ReadonlySet<string>): Plan => {
  if (!isObject(raw)) throw new Drop('shape');
  const { id, kind, term, includes, price, checkoutUrl, until } = raw;
  if (typeof id !== 'string' || !PLAN_ID.test(id)) throw new Drop('shape');
  if (kind !== 'free' && kind !== 'paid') throw new Drop('kind');
  if (kind === 'paid' ? term !== 'month' && term !== 'year' : term !== null) throw new Drop('term');
  const name = text(raw.name, TEXT_LIMITS.name, false)!;
  const summary = text(raw.summary, TEXT_LIMITS.summary, true);
  if (!Array.isArray(includes)) throw new Drop('shape');
  if (includes.length > MAX_INCLUDES) throw new Drop('text');
  const lines = includes.map(line => text(line, TEXT_LIMITS.include, true)).filter((l): l is string => l !== null);
  const priceNote = text(raw.priceNote, TEXT_LIMITS.priceNote, true);
  const badge = text(raw.badge, TEXT_LIMITS.badge, true);
  let parsedPrice: Price | null = null;
  if (kind === 'free') {
    if (price !== null) throw new Drop('price');
  } else {
    if (!isObject(price)) throw new Drop('price');
    const { amount, currency } = price;
    if (typeof amount !== 'number' || !Number.isInteger(amount) || amount < 0 || amount > MAX_AMOUNT)
      throw new Drop('price');
    if (typeof currency !== 'string' || !CURRENCY.test(currency) || !currencies.has(currency))
      throw new Drop('currency');
    parsedPrice = { amount, currency };
  }
  let checkout: string | null = null;
  if (kind === 'free') {
    if (checkoutUrl !== null) throw new Drop('shape');
  } else {
    if (typeof checkoutUrl !== 'string') throw new Drop('shape');
    checkout = payUrl(checkoutUrl, rules.payHosts) ?? null;
    if (checkout === null) throw new Drop('url');
  }
  if (!(until === null || (typeof until === 'number' && Number.isSafeInteger(until) && until >= 0)))
    throw new Drop('shape');
  return {
    id,
    kind,
    term: kind === 'paid' ? (term as Term) : null,
    name,
    summary,
    includes: lines,
    price: parsedPrice,
    priceNote,
    badge,
    checkoutUrl: checkout,
    until: until === null ? null : until * 1000,
  };
};

/** The catalog and its plans over parsed JSON. Pure, never throws. */
export const parseCatalog = (raw: unknown, rules: CatalogRules): CatalogParse => {
  try {
    if (!isObject(raw)) return { ok: false, why: 'shape' };
    if (raw.v !== 1) return { ok: false, why: typeof raw.v === 'number' ? 'unsupported' : 'shape' };
    const { catalog, maxAge, manageUrl, plans } = raw;
    if (typeof catalog !== 'string' || !CATALOG_ID.test(catalog)) return { ok: false, why: 'shape' };
    if (typeof maxAge !== 'number' || !Number.isFinite(maxAge)) return { ok: false, why: 'shape' };
    if (!(manageUrl === null || typeof manageUrl === 'string')) return { ok: false, why: 'shape' };
    if (!Array.isArray(plans)) return { ok: false, why: 'shape' };
    if (plans.length === 0) return { ok: false, why: 'empty' };
    if (plans.length > MAX_PLANS) return { ok: false, why: 'shape' };
    const currencies = rules.currencies ?? intlCurrencies();
    const kept: Plan[] = [];
    const dropped: { index: number; why: DropReason }[] = [];
    plans.forEach((entry, index) => {
      try {
        const plan = parsePlan(entry, rules, currencies);
        if (kept.some(p => p.id === plan.id)) throw new Drop('duplicate');
        kept.push(plan);
      } catch (error) {
        if (!(error instanceof Drop)) throw error;
        dropped.push({ index, why: error.why });
      }
    });
    if (kept.length === 0) return { ok: false, why: 'empty' };
    return {
      ok: true,
      catalog: {
        v: 1,
        catalog,
        maxAgeMs: Math.min(Math.max(maxAge, MAX_AGE_MIN_S), MAX_AGE_MAX_S) * 1000,
        manageUrl: manageUrl === null ? null : (payUrl(manageUrl, rules.payHosts) ?? null),
        plans: kept,
      },
      dropped,
    };
  } catch {
    return { ok: false, why: 'shape' };
  }
};

/** The plans to show at `now`: those whose `until` is null or ahead. */
export const plansAt = (catalog: Catalog, now: number): readonly Plan[] =>
  catalog.plans.filter(plan => plan.until === null || plan.until > now);
