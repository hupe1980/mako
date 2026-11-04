// Values the Desk may show but never compute with. A service sends them as
// strings; the generated types brand them, and only src/format can render one.

declare const brand: unique symbol;

/** An instant, RFC 3339 in UTC, as a service sent it. */
export type Instant = string & { readonly [brand]: "Instant" };

/** A German calendar date, `YYYY-MM-DD` — never a JS `Date`. */
export type BusinessDate = string & { readonly [brand]: "BusinessDate" };

/** An exact decimal amount as a string — never a JS `number`. */
export type Decimal = string & { readonly [brand]: "Decimal" };
