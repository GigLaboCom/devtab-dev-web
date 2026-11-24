/**
 * The record `plans` in chrome.storage.local: the last accepted answer of `GET /v1/plans` and the attempts around it.
 * Only the service worker writes it; the pages read it here, parse its body again (they trust no record) and follow
 * its changes. Types and the pages' reader; no chrome.
 */
export const PLANS_KEY = 'plans';

export type PlansFailure = 'offline' | 'server' | 'bad-answer' | 'unsupported' | 'rate-limited' | 'server-busy';

export interface PlansRecord {
  readonly v: 1;
  /** The worker's clock when the accepted answer arrived, ms. Absent: never fetched. */
  readonly fetchedAt?: number;
  /** The accepted answer exactly as parsed JSON; the pages parse it again with parseCatalog. */
  readonly body?: unknown;
  /** The last attempt that did not replace `body`. */
  readonly failure?: { readonly at: number; readonly why: PlansFailure };
  readonly attempts: {
    readonly last?: number;
    readonly failures: number;
    readonly retryAfter?: number;
    readonly retryCause?: 'rate-limited' | 'server-busy';
  };
}

const FAILURES: readonly PlansFailure[] = [
  'offline',
  'server',
  'bad-answer',
  'unsupported',
  'rate-limited',
  'server-busy',
];
const DAY = 24 * 60 * 60 * 1000;
/** A list older than this is not shown at all, as if there were none. */
export const STALE_FOR_MS = 30 * DAY;

type Json = Record<string, unknown>;
const isObject = (value: unknown): value is Json =>
  typeof value === 'object' && value !== null && !Array.isArray(value);
const isTime = (value: unknown): value is number => typeof value === 'number' && Number.isFinite(value);
const optional = (value: unknown, check: (v: unknown) => boolean) => value === undefined || check(value);

/** `none`: no record; `unreadable`: another shape or `v`. Unknown extra fields are ignored. */
export const readPlansRecord = (raw: unknown): PlansRecord | 'none' | 'unreadable' => {
  if (raw === undefined || raw === null) return 'none';
  if (!isObject(raw) || raw.v !== 1) return 'unreadable';
  const { fetchedAt, body, failure, attempts } = raw;
  if (!optional(fetchedAt, isTime)) return 'unreadable';
  if (!optional(failure, f => isObject(f) && isTime(f.at) && FAILURES.includes(f.why as PlansFailure)))
    return 'unreadable';
  if (
    !isObject(attempts) ||
    !(typeof attempts.failures === 'number' && Number.isInteger(attempts.failures) && attempts.failures >= 0) ||
    !optional(attempts.last, isTime) ||
    !optional(attempts.retryAfter, isTime) ||
    !optional(attempts.retryCause, c => c === 'rate-limited' || c === 'server-busy')
  )
    return 'unreadable';
  return {
    v: 1,
    ...(fetchedAt !== undefined ? { fetchedAt: fetchedAt as number } : {}),
    ...(body !== undefined ? { body } : {}),
    ...(failure !== undefined ? { failure: failure as PlansRecord['failure'] } : {}),
    attempts: attempts as PlansRecord['attempts'],
  };
};

export type Freshness = 'fresh' | 'stale' | 'expired';

/** Fresh while fetchedAt ≤ now < fetchedAt + maxAge; stale up to 30 days after fetchedAt (or with the clock put back before it); else expired. */
export const freshness = (fetchedAt: number, maxAgeMs: number, now: number): Freshness => {
  if (now < fetchedAt) return 'stale';
  if (now < fetchedAt + maxAgeMs) return 'fresh';
  if (now < fetchedAt + STALE_FOR_MS) return 'stale';
  return 'expired';
};
