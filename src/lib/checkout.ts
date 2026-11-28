/**
 * The purchase, on the site's server: starting a checkout of a plan, signed
 * in or (where the deployment allows it) without an account, and reading how
 * it went for the return page. The API's internal listener does the work
 * (`POST /internal/checkouts`, `/internal/checkouts/guest`,
 * `GET /internal/checkouts/{id}`, `POST /internal/checkouts/{id}/claim`).
 *
 * A guest checkout's answer carries a secret: the only thing that reads the
 * checkout before the buyer's account exists. It stays in an `HttpOnly`
 * cookie of the browser that started it (`devtab-checkout`, a day, with the
 * checkout's id), never in an address or a page.
 */
import type { AstroCookies } from 'astro';
import { apiFromServer } from './api/client.ts';
import type { components } from './api/schema.gen.ts';

export type StartedCheckout = components['schemas']['StartedCheckout'];
export type StartedGuestCheckout = components['schemas']['StartedGuestCheckout'];
export type CheckoutState = components['schemas']['CheckoutState'];
export type GuestCheckoutState = components['schemas']['GuestCheckoutState'];

/**
 * Whether anyone may buy without signing in: `DEVTAB_GUEST_CHECKOUT=true`, a
 * deployment's setting (off unless set). The API has its own setting of the
 * same name and needs the Supabase secret key to invite buyers; without
 * either it refuses, and the page says so.
 */
export function guestCheckoutEnabled(): boolean {
  return process.env.DEVTAB_GUEST_CHECKOUT?.trim().toLowerCase() === 'true';
}

/** The cookie that holds a guest checkout's id and secret. */
export const CHECKOUT_COOKIE = 'devtab-checkout';
/** As long as the key waits to be shown. */
const CHECKOUT_MAX_AGE_S = 86_400;
const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;

/** A checkout's id as the return address carries it, or null. */
export function checkoutRef(raw: string | null): string | null {
  const ref = raw?.trim().toLowerCase() ?? '';
  return UUID.test(ref) ? ref : null;
}

/** Keeps a guest checkout's secret in this browser, replacing any earlier one. */
export function keepClaim(cookies: AstroCookies, url: URL, checkout: string, claim: string): void {
  cookies.set(CHECKOUT_COOKIE, `${checkout}.${claim}`, {
    httpOnly: true,
    secure: url.protocol === 'https:',
    // Sent on the provider's way back, a top-level navigation to the return page.
    sameSite: 'lax',
    path: '/',
    maxAge: CHECKOUT_MAX_AGE_S,
  });
}

/** This browser's secret of the guest checkout `checkout`, if it started it. */
export function claimOf(cookies: AstroCookies, checkout: string): string | null {
  const value = cookies.get(CHECKOUT_COOKIE)?.value ?? '';
  const dot = value.indexOf('.');
  if (dot < 0 || value.slice(0, dot) !== checkout) return null;
  const claim = value.slice(dot + 1);
  return /^[A-Za-z0-9_-]{32,128}$/.test(claim) ? claim : null;
}

/** What starting a checkout came to: the provider's address, or why not. */
export type StartResult =
  | { ok: true; redirect: string; checkout: string; claim?: string }
  | { ok: false; error: 'unavailable' | 'members' | 'signed-out' | 'guest-off' | 'payments' };

/**
 * Starts a checkout of `plan` in `locale` (`members` for a team): as the
 * signed-in person of `locals`, or without an account when `guest`.
 */
export async function startCheckout(
  locals: App.Locals,
  guest: boolean,
  body: { plan: string; locale: string; members?: number },
): Promise<StartResult> {
  const path = guest ? '/internal/checkouts/guest' : '/internal/checkouts';
  const { status, body: answer } = await apiFromServer<StartedGuestCheckout & { error?: string }>(
    guest ? null : locals,
    path,
    { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify(body) },
  );
  if (status === 201 && answer) {
    // The site sends the buyer to the provider's page; the embedded form
    // would need Stripe.js, which the site does not load.
    if (!answer.redirect_url) {
      console.error('checkout: the API started an embedded checkout, which the site does not show');
      return { ok: false, error: 'payments' };
    }
    return {
      ok: true,
      redirect: answer.redirect_url,
      checkout: answer.checkout,
      ...(guest && answer.claim ? { claim: answer.claim } : {}),
    };
  }
  if (status !== 404 && status !== 409) console.error(`POST ${path} answered ${status} ${answer?.error ?? ''}`);
  if (status === 401) return { ok: false, error: 'signed-out' };
  if (status === 400) return { ok: false, error: 'members' };
  if (status === 404 || status === 409) return { ok: false, error: 'unavailable' };
  if (answer?.error === 'guest-checkout-off' || answer?.error === 'guest-checkout-unavailable') {
    return { ok: false, error: 'guest-off' };
  }
  return { ok: false, error: 'payments' };
}

/** The signed-in person's checkout, or null when it is not theirs. */
export async function checkoutOfPerson(locals: App.Locals, checkout: string): Promise<CheckoutState | null> {
  const { status, body } = await apiFromServer<CheckoutState>(locals, `/internal/checkouts/${checkout}`);
  return status === 200 ? body : null;
}

/** A guest checkout as the browser holding its secret sees it, or null. */
export async function checkoutOfBrowser(checkout: string, claim: string): Promise<GuestCheckoutState | null> {
  const { status, body } = await apiFromServer<GuestCheckoutState>(null, `/internal/checkouts/${checkout}/claim`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ claim }),
  });
  return status === 200 ? body : null;
}
