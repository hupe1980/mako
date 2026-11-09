// A skill's record as text. The counts are agentd's (agentplane's track
// record over its window); this layer only says them, and says a share only
// where there are enough verdicts for one to mean something.

import { katalog } from "../i18n";
import { formatCount } from "./count";
import { perSprache } from "./locale";

/** Below this many verdicts a share is noise: the count alone is shown. */
export const MINDEST_URTEILE = 5;

const prozent = perSprache(
  (locale) => new Intl.NumberFormat(locale, { style: "percent", maximumFractionDigits: 0 }),
);

/** `teil` of `ganz` as a percentage, „86 %"; `ganz` is never zero here. */
export function formatAnteil(teil: number, ganz: number): string {
  return prozent().format(teil / ganz);
}

/** „1 Bewertung", „7 Bewertungen". */
export function formatBewertungen(n: number): string {
  return katalog().bilanz.bewertungen(n, formatCount(n));
}

/** The „Hilfreich?" verdicts as one phrase, with a share only from five up. */
export function hilfreichText(h: { ja: number; nein: number }): string {
  const n = h.ja + h.nein;
  if (n === 0) return katalog().bilanz.keineBewertungen;
  if (n < MINDEST_URTEILE) return formatBewertungen(n);
  return katalog().bilanz.hilfreich(formatAnteil(h.ja, n), formatBewertungen(n));
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
  const b = katalog().bilanz;
  if (n === 0) return b.keinVorschlag;
  if (n < MINDEST_URTEILE) return b.entschieden(n, formatCount(n));
  return b.uebernommen(formatCount(v.sent ?? 0), formatCount(n));
}

/** Each outcome's count, „14 übernommen · 2 angepasst · 1 verworfen". */
export function vorschlagAufschluesselung(v: Schicksale): string {
  return katalog().bilanz.aufschluesselung(
    formatCount(v.sent ?? 0),
    formatCount(v.edited ?? 0),
    formatCount(v.dismissed ?? 0),
  );
}
