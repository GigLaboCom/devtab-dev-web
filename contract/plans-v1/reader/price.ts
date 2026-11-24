/**
 * A plan's price on its card: the amount in the currency's major units by the currency's own rule (its ISO 4217
 * digits — USD 2, JPY 0, KWD 3), in en-US like every date of Settings → Pro. No chrome.
 */
import type { Price, Term } from './model';

const digitsOf = (currency: string): number =>
  new Intl.NumberFormat('en-US', { style: 'currency', currency }).resolvedOptions().maximumFractionDigits ?? 2;

/** The amount in the currency's major units, by the currency's own rule (ISO 4217 digits), in en-US. */
export const formatPrice = (price: Price): string =>
  new Intl.NumberFormat('en-US', { style: 'currency', currency: price.currency, currencyDisplay: 'symbol' }).format(
    price.amount / 10 ** digitsOf(price.currency),
  );

/** "per month" / "per year". */
export const periodText = (term: Term): string => (term === 'month' ? 'per month' : 'per year');
