// A skill's record as text. The counts are agentd's (agentplane's track
// record over its window); this layer only says them, and says a share only
// where there are enough verdicts for one to mean something (spec M74 FR-006).

import { formatCount } from "./count";

/** Below this many verdicts a share is noise: the count alone is shown. */
export const MINDEST_URTEILE = 5;

const PROZENT = new Intl.NumberFormat("de-DE", { style: "percent", maximumFractionDigits: 0 });

/** `teil` of `ganz` as a percentage, „86 %"; `ganz` is never zero here. */
export function formatAnteil(teil: number, ganz: number): string {
  return PROZENT.format(teil / ganz);
}

/** „1 Bewertung", „7 Bewertungen". */
export function formatBewertungen(n: number): string {
  return `${formatCount(n)} ${n === 1 ? "Bewertung" : "Bewertungen"}`;
}

/** The „Hilfreich?" verdicts as one phrase, with a share only from five up. */
export function hilfreichText(h: { ja: number; nein: number }): string {
  const n = h.ja + h.nein;
  if (n === 0) return "Noch keine Bewertungen";
  if (n < MINDEST_URTEILE) return formatBewertungen(n);
  return `${formatAnteil(h.ja, n)} hilfreich · ${formatBewertungen(n)}`;
}

/** What became of the proposals, by agentd's labels. */
export interface Schicksale {
  sent?: number;
  edited?: number;
  dismissed?: number;
}

/** How many proposals a person decided on, by any outcome. */
export function entschieden(v: Schicksale): number {
  return (v.sent ?? 0) + (v.edited ?? 0) + (v.dismissed ?? 0);
}

/**
 * The proposals as one phrase: „14 von 17 Vorschlägen übernommen" from five
 * decided up, the count alone below, and „noch keiner" for none.
 */
export function vorschlagText(v: Schicksale): string {
  const n = entschieden(v);
  if (n === 0) return "Noch kein Vorschlag entschieden";
  if (n < MINDEST_URTEILE)
    return `${formatCount(n)} ${n === 1 ? "Vorschlag" : "Vorschläge"} entschieden`;
  return `${formatCount(v.sent ?? 0)} von ${formatCount(n)} Vorschlägen übernommen`;
}

/** Each outcome's count, „14 übernommen · 2 angepasst · 1 verworfen". */
export function vorschlagAufschluesselung(v: Schicksale): string {
  return [
    `${formatCount(v.sent ?? 0)} übernommen`,
    `${formatCount(v.edited ?? 0)} angepasst`,
    `${formatCount(v.dismissed ?? 0)} verworfen`,
  ].join(" · ");
}
