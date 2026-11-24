/**
 * The licence's state at a moment: a pure function of what is stored (after the signature was checked), the clock and
 * the clock mark. Time never goes below the latest instant seen: putting the clock back does not extend Pro.
 */
import type { Claims, InvalidReason, RejectReason, RevokeReason, Term } from './types';

/** What a page or the worker says about the licence; every time in milliseconds. */
export interface LicenceFacts {
  readonly lid: string;
  /** The last successful check: the token's `iat`. */
  readonly checkedAt: number;
  readonly checkAfter: number;
  readonly warnFrom: number;
  readonly validUntil: number;
  readonly paidUntil: number | null;
  readonly term?: Term;
  readonly renews?: boolean;
}

export type Stored =
  | { readonly kind: 'none' }
  | { readonly kind: 'unreadable' }
  | { readonly kind: 'rejected'; readonly reason: RejectReason }
  | { readonly kind: 'token'; readonly claims: Claims; readonly inst: string };

export type EditionState =
  | { readonly kind: 'free' }
  | { readonly kind: 'active'; readonly licence: LicenceFacts; readonly due: boolean }
  | { readonly kind: 'grace'; readonly licence: LicenceFacts }
  | { readonly kind: 'expired'; readonly licence: LicenceFacts }
  | { readonly kind: 'ended'; readonly licence: LicenceFacts }
  | { readonly kind: 'revoked'; readonly licence: LicenceFacts; readonly reason: RevokeReason }
  | { readonly kind: 'invalid'; readonly reason: InvalidReason };

/** A clock mark that is not a finite non-negative number counts as 0. */
const markOf = (seen: number): number => (typeof seen === 'number' && Number.isFinite(seen) && seen >= 0 ? seen : 0);

/** max(now, seen, iat); a `seen` that is not a finite non-negative number counts as 0. */
export const decisionTime = (now: number, seen: number, claims?: Claims): number =>
  Math.max(now, markOf(seen), claims?.iat ?? 0);

/** What a page or the worker says about the licence, from the token's fields. */
export const factsOf = (claims: Claims): LicenceFacts => ({
  lid: claims.lid,
  checkedAt: claims.iat,
  checkAfter: claims.chk,
  warnFrom: claims.warn,
  validUntil: claims.exp,
  paidUntil: claims.paid,
  ...(claims.term !== undefined ? { term: claims.term } : {}),
  ...(claims.renews !== undefined ? { renews: claims.renews } : {}),
});

/** The table of states (pro-entitlement §4.1). Pure; never throws. */
export const evaluate = (stored: Stored, now: number, seen: number): EditionState => {
  switch (stored.kind) {
    case 'none':
      return { kind: 'free' };
    case 'unreadable':
      return { kind: 'invalid', reason: 'unreadable' };
    case 'rejected':
      return { kind: 'invalid', reason: stored.reason };
    case 'token': {
      const { claims } = stored;
      if (claims.inst !== stored.inst) return { kind: 'invalid', reason: 'foreign' };
      const licence = factsOf(claims);
      if (claims.status === 'ended') return { kind: 'ended', licence };
      if (claims.status === 'revoked') return { kind: 'revoked', licence, reason: claims.reason ?? 'deactivated' };
      const t = decisionTime(now, seen, claims);
      if (t >= claims.exp) return { kind: 'expired', licence };
      if (t >= claims.warn) return { kind: 'grace', licence };
      return { kind: 'active', licence, due: t >= claims.chk };
    }
    default:
      return { kind: 'invalid', reason: 'unreadable' };
  }
};

/** The next instant at which `evaluate` would give another answer (chk, warn or exp ahead of the decision time). */
export const nextBoundary = (stored: Stored, now: number, seen: number): number | undefined => {
  if (stored.kind !== 'token') return undefined;
  const { claims } = stored;
  if (claims.inst !== stored.inst || claims.status !== 'active') return undefined;
  const t = decisionTime(now, seen, claims);
  return [claims.chk, claims.warn, claims.exp].find(boundary => boundary > t);
};
