// An amount is an exact decimal string from the service and stays one: it is
// handed to Intl.NumberFormat as a string, which formats it without converting
// to a binary float, so `0.10000` cannot become `0.1000000000000000055…`.

import type { Decimal } from "./brands";
import { perSprache } from "./locale";

const euro = perSprache(
  (locale) =>
    new Intl.NumberFormat(locale, {
      style: "currency",
      currency: "EUR",
      minimumFractionDigits: 2,
      maximumFractionDigits: 5,
    }),
);

const plain = perSprache(
  (locale) =>
    new Intl.NumberFormat(locale, {
      minimumFractionDigits: 0,
      maximumFractionDigits: 20,
    }),
);

/** `1.234,56789 €` — an amount in euro, every digit the service sent. */
export function formatEuro(amount: Decimal): string {
  return euro().format(amount as Intl.StringNumericLiteral);
}

/** `0,1` — a decimal without unit, every significant digit kept. */
export function formatDecimal(value: Decimal): string {
  return plain().format(value as Intl.StringNumericLiteral);
}

/**
 * `-12,34 €` — a whole number of cents, as a ledger keeps it. The digits are
 * rearranged into a decimal string, so no float division ever touches it.
 */
export function formatCent(ct: number): string {
  const sign = ct < 0 ? "-" : "";
  const digits = Math.abs(Math.trunc(ct)).toString().padStart(3, "0");
  return euro().format(
    `${sign}${digits.slice(0, -2)}.${digits.slice(-2)}` as Intl.StringNumericLiteral,
  );
}
