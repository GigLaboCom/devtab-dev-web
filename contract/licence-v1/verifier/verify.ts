/**
 * The licence token: a compact JWS (RFC 7515) signed with ES256 — ECDSA on P-256 with SHA-256 through WebCrypto, a
 * 64-byte r‖s signature (IEEE P1363) over exactly the ASCII bytes `<header>.<payload>` as they came. The JSON is never
 * rebuilt for the check. The checks run in a fixed order and the first break gives the reason:
 *
 *   1. at most 4 096 characters, exactly three parts, each strict base64url, header and payload JSON objects → malformed
 *   2. the header is exactly alg, kid, typ with alg ES256, typ devtab-licence and a kid by its rule        → unsupported
 *   3. the signature is exactly 64 bytes                                                                 → malformed
 *   4. the kid is one of the build's keys                                                               → unknown-key
 *   5. the signature verifies                                                                             → signature
 *   6. the payload: a numeric `v` other than 1 → unsupported; any other break of the fields               → claims
 *
 * The header comes before the signature's size, so `alg: none` without a signature and HS256 with a 32-byte HMAC are
 * `unsupported`, and a DER signature under a good header is `malformed`.
 */
import { base64urlDecode } from './base64url';
import type { Claims, LicenceStatus, PublicKeyEntry, RejectReason, RevokeReason, Term, Verified } from './types';

/** A longer token is `malformed`; a real one is about 450 characters. */
export const MAX_TOKEN_LENGTH = 4096;
/** The header's `typ`: tells our token from any other JWT signed by mistake with the same key. */
export const TOKEN_TYPE = 'devtab-licence';
const SIGNATURE_BYTES = 64;

const KID = /^[a-z0-9-]{1,32}$/;
const LID = /^[A-Za-z0-9_-]{8,64}$/;
const INST = /^[A-Za-z0-9_-]{22}$/;
const STATUSES: readonly LicenceStatus[] = ['active', 'ended', 'revoked'];
const REASONS: readonly RevokeReason[] = ['refund', 'deactivated', 'key-blocked', 'seat-limit', 'unknown-installation'];
const TERMS: readonly Term[] = ['month', 'year'];

const ALGORITHM = { name: 'ECDSA', namedCurve: 'P-256' } as const;
const SIGNING = { name: 'ECDSA', hash: 'SHA-256' } as const;

type JsonObject = Record<string, unknown>;

const isObject = (value: unknown): value is JsonObject =>
  typeof value === 'object' && value !== null && !Array.isArray(value);

const jsonObject = (bytes: Uint8Array): JsonObject | undefined => {
  try {
    const value: unknown = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(bytes));
    return isObject(value) ? value : undefined;
  } catch {
    return undefined;
  }
};

const headerFits = (header: JsonObject): boolean =>
  Object.keys(header).sort().join(',') === 'alg,kid,typ' &&
  header.alg === 'ES256' &&
  header.typ === TOKEN_TYPE &&
  typeof header.kid === 'string' &&
  KID.test(header.kid);

/** Whole Unix seconds that still fit in milliseconds. */
const isSeconds = (value: unknown): value is number =>
  typeof value === 'number' && Number.isSafeInteger(value) && value >= 0 && Number.isSafeInteger(value * 1000);

/** The fields of an already parsed payload, or `undefined` when they break the format (a wrong `term`/`renews` is dropped). */
export const parseClaims = (payload: unknown): Claims | undefined => {
  if (!isObject(payload)) return undefined;
  const { v, lid, inst, plan, status, reason, iat, chk, warn, exp, paid, term, renews } = payload;
  if (v !== 1) return undefined;
  if (typeof lid !== 'string' || !LID.test(lid)) return undefined;
  if (typeof inst !== 'string' || !INST.test(inst)) return undefined;
  if (plan !== 'pro') return undefined;
  if (!STATUSES.includes(status as LicenceStatus)) return undefined;
  if (status === 'revoked' ? !REASONS.includes(reason as RevokeReason) : 'reason' in payload) return undefined;
  if (!isSeconds(iat) || !isSeconds(chk) || !isSeconds(warn) || !isSeconds(exp)) return undefined;
  if (!(paid === null || isSeconds(paid))) return undefined;
  if (status === 'active' ? !(iat <= chk && chk <= warn && warn <= exp) : !(warn === exp && exp === iat && iat <= chk))
    return undefined;
  return {
    v: 1,
    lid,
    inst,
    plan: 'pro',
    status: status as LicenceStatus,
    ...(status === 'revoked' ? { reason: reason as RevokeReason } : {}),
    iat: iat * 1000,
    chk: chk * 1000,
    warn: warn * 1000,
    exp: exp * 1000,
    paid: paid === null ? null : paid * 1000,
    // Fields for the plan cards only: a wrong type is "not read", never a refusal of the token (that would turn Pro off).
    ...(TERMS.includes(term as Term) ? { term: term as Term } : {}),
    ...(typeof renews === 'boolean' ? { renews } : {}),
  };
};

const refuse = (reason: RejectReason): Verified => ({ ok: false, reason });

const signatureHolds = async (
  given: SubtleCrypto | undefined,
  entry: PublicKeyEntry,
  signature: Uint8Array<ArrayBuffer>,
  signed: string,
): Promise<boolean> => {
  try {
    const subtle = given ?? globalThis.crypto.subtle;
    const { kty, crv, x, y } = entry.jwk;
    const key = await subtle.importKey('jwk', { kty, crv, x, y, ext: true }, ALGORITHM, false, ['verify']);
    return await subtle.verify(SIGNING, key, signature, new TextEncoder().encode(signed));
  } catch {
    return false;
  }
};

/** Parses and checks a compact JWS: header, signature with one of `keys`, fields. Never rejects. */
export const verifyToken = async (
  compact: string,
  keys: readonly PublicKeyEntry[],
  subtle?: SubtleCrypto,
): Promise<Verified> => {
  try {
    if (typeof compact !== 'string' || compact.length > MAX_TOKEN_LENGTH) return refuse('malformed');
    const parts = compact.split('.');
    if (parts.length !== 3) return refuse('malformed');
    const [headPart, bodyPart, signaturePart] = parts as [string, string, string];
    const head = base64urlDecode(headPart);
    const body = base64urlDecode(bodyPart);
    const signature = base64urlDecode(signaturePart);
    if (!head || !body || !signature) return refuse('malformed');
    const header = jsonObject(head);
    const payload = jsonObject(body);
    if (!header || !payload) return refuse('malformed');
    if (!headerFits(header)) return refuse('unsupported');
    if (signature.length !== SIGNATURE_BYTES) return refuse('malformed');
    const kid = header.kid as string;
    const entry = keys.find(k => k.kid === kid);
    if (!entry) return refuse('unknown-key');
    if (!(await signatureHolds(subtle, entry, signature, `${headPart}.${bodyPart}`))) return refuse('signature');
    if (typeof payload.v === 'number' && payload.v !== 1) return refuse('unsupported');
    const claims = parseClaims(payload);
    return claims ? { ok: true, claims, kid } : refuse('claims');
  } catch {
    // Nothing above throws by design; whatever does still gives a reason, never a rejection.
    return refuse('malformed');
  }
};
