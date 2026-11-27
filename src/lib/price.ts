/**
 * A plan's price on the site: the amount comes in the currency's minor units
 * and is shown in its major units by the currency's own rule (its ISO 4217
 * digits: USD 2, JPY 0), formatted for the page's locale. The same rule as
 * the extension's cards, so that both show the same number.
 */
import type { Locale } from '../i18n/config.ts';

const digitsOf = (currency: string): number =>
  new Intl.NumberFormat('en-US', { style: 'currency', currency }).resolvedOptions().maximumFractionDigits ?? 2;

export function formatPrice(price: { amount: number; currency: string }, locale: Locale): string {
  return new Intl.NumberFormat(locale, { style: 'currency', currency: price.currency }).format(
    price.amount / 10 ** digitsOf(price.currency),
  );
}
