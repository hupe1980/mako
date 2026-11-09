// A count as a service returned it. `atLeast` is the service saying its answer
// stopped at a cap — the number is a floor, shown as one.

import { perSprache } from "./locale";

const GROUPED = perSprache((locale) => new Intl.NumberFormat(locale, { maximumFractionDigits: 0 }));

export function formatCount(n: number, atLeast = false): string {
  return `${GROUPED().format(n)}${atLeast ? "+" : ""}`;
}
