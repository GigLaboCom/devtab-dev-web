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

/** Sign-in attempts (`POST /auth/sign-in/`) allowed per client address in an hour. */
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
