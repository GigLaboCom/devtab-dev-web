/**
 * The trial without a card, on the site's server: whether the deployment
 * offers one comes with the plans (`trialDays` of `GET /internal/plans`), and
 * a signed-in person starts theirs with `POST /internal/trials`, which
 * answers its key once.
 */
import { apiFromServer } from './api/client.ts';
import type { components } from './api/schema.gen.ts';

export type StartedTrial = components['schemas']['StartedTrial'];

/** What starting a trial came to. */
export type TrialResult =
  | { ok: true; trial: StartedTrial }
  | { ok: false; error: 'used' | 'not-eligible' | 'off' | 'signed-out' | 'unavailable' };

/** Starts the signed-in person's trial, its mails in `locale`. */
export async function startTrial(locals: App.Locals, locale: string): Promise<TrialResult> {
  const { status, body } = await apiFromServer<StartedTrial & { error?: string }>(locals, '/internal/trials', {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify({ locale }),
  });
  if (status === 201 && body) return { ok: true, trial: body };
  if (body?.error === 'trial-used') return { ok: false, error: 'used' };
  if (body?.error === 'trial-not-eligible') return { ok: false, error: 'not-eligible' };
  if (body?.error === 'trial-off') return { ok: false, error: 'off' };
  if (status === 401) return { ok: false, error: 'signed-out' };
  console.error(`POST /internal/trials answered ${status} ${body?.error ?? ''}`);
  return { ok: false, error: 'unavailable' };
}
