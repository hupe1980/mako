// An amount is an exact decimal string from the service and stays one: it is
// handed to Intl.NumberFormat as a string, which formats it without converting
// to a binary float, so `0.10000` cannot become `0.1000000000000000055…`.

import type { Decimal } from "./brands";

const euro = new Intl.NumberFormat("de-DE", {
  style: "currency",
  currency: "EUR",
  minimumFractionDigits: 2,
  maximumFractionDigits: 5,
});

const plain = new Intl.NumberFormat("de-DE", {
  minimumFractionDigits: 0,
  maximumFractionDigits: 20,
});

/** `1.234,56789 €` — an amount in euro, every digit the service sent. */
export function formatEuro(amount: Decimal): string {
  return euro.format(amount as Intl.StringNumericLiteral);
}

/** `0,1` — a decimal without unit, every significant digit kept. */
export function formatDecimal(value: Decimal): string {
  return plain.format(value as Intl.StringNumericLiteral);
}
