/** The right a token grants: one value today; a list of rights in the future would be a new field, not a new value. */
export type Plan = 'pro';
/** A subscription's billing period; shown on the plan cards only, it grants nothing. */
export type Term = 'month' | 'year';
export type LicenceStatus = 'active' | 'ended' | 'revoked';
export type RevokeReason = 'refund' | 'deactivated' | 'key-blocked' | 'seat-limit' | 'unknown-installation';
export type InvalidReason =
  'unreadable' | 'malformed' | 'unsupported' | 'unknown-key' | 'signature' | 'claims' | 'foreign';

/** The token's fields after parsing; every time in milliseconds since the epoch (seconds on the wire). */
export interface Claims {
  readonly v: 1;
  readonly lid: string;
  readonly inst: string;
  readonly plan: Plan;
  readonly status: LicenceStatus;
  readonly reason?: RevokeReason;
  readonly iat: number;
  readonly chk: number;
  readonly warn: number;
  readonly exp: number;
  readonly paid: number | null;
  /** Shown on the plan cards only; absent when missing or of a wrong type. */
  readonly term?: Term;
  readonly renews?: boolean;
}

/** A public key the build trusts: its id and the key as a JWK (P-256, coordinates in base64url). */
export interface PublicKeyEntry {
  readonly kid: string;
  readonly jwk: { readonly kty: 'EC'; readonly crv: 'P-256'; readonly x: string; readonly y: string };
}

/** Why a token was refused by `verifyToken`; `unreadable` and `foreign` come from the record around it. */
export type RejectReason = Exclude<InvalidReason, 'unreadable' | 'foreign'>;

export type Verified =
  | { readonly ok: true; readonly claims: Claims; readonly kid: string }
  | { readonly ok: false; readonly reason: RejectReason };
