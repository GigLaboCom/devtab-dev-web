/**
 * Sign-in's settings, read on the server when it runs (one build, configured
 * by its environment). The site gets only public values: the Supabase
 * project's address and its publishable key, which anyone may see. Without
 * both, sign-in is off: no session is read and the sign-in page says so.
 */
export interface AuthConfig {
  /** The Supabase project's address, as the browser reaches it. */
  url: string;
  /** The project's publishable key: public by design. */
  key: string;
}

export function authConfig(): AuthConfig | null {
  const url = process.env.DEVTAB_SUPABASE_URL?.trim().replace(/\/+$/, '');
  const key = process.env.DEVTAB_SUPABASE_PUBLISHABLE_KEY?.trim();
  return url && key ? { url, key } : null;
}

/**
 * The ways to sign in a deployment can offer:
 *
 * - `email`: a mail with a sign-in link and a one-time code;
 * - `password`: an e-mail address and a password, with signing up (the
 *   address confirmed by a mail), and a forgotten password reset by a mail.
 */
export const signInMethods = ['email', 'password'] as const;
export type SignInMethod = (typeof signInMethods)[number];

/**
 * The methods this deployment offers: `DEVTAB_SIGN_IN_METHODS`, the names of
 * `signInMethods` separated by commas; unset or empty, `email` alone. A name
 * it does not know is ignored. A method left out is not shown, and its pages
 * and endpoints answer 404.
 */
export function enabledMethods(): ReadonlySet<SignInMethod> {
  const raw = process.env.DEVTAB_SIGN_IN_METHODS?.trim();
  if (!raw) return new Set(['email']);
  const known = new Set<string>(signInMethods);
  return new Set(
    raw
      .split(',')
      .map(name => name.trim().toLowerCase())
      .filter((name): name is SignInMethod => known.has(name)),
  );
}

/** Sign-in attempts (the forms under `/auth/`) allowed per client address in an hour. */
export function signInLimit(): number {
  const limit = Number.parseInt(process.env.DEVTAB_RATE_SIGN_IN_PER_IP ?? '', 10);
  return Number.isInteger(limit) && limit > 0 ? limit : 10;
}

/** A path of this site to go to after signing in; anything else becomes `fallback`. */
export function safeNext(next: string | null | undefined, fallback: string): string {
  if (!next || !next.startsWith('/') || next.startsWith('//') || next.startsWith('/\\') || /[\r\n]/.test(next)) {
    return fallback;
  }
  return next;
}
