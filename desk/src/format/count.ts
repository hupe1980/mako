// A count as a service returned it. `atLeast` is the service saying its answer
// stopped at a cap — the number is a floor, shown as one.

const GROUPED = new Intl.NumberFormat("de-DE", { maximumFractionDigits: 0 });

export function formatCount(n: number, atLeast = false): string {
  return `${GROUPED.format(n)}${atLeast ? "+" : ""}`;
}
