// One formatter per Sprache, built when first asked for: the Sprache decides
// the digits' grouping and the date's order, the formatter's options the rest.

import { type Sprache, sprache } from "../i18n";

const LOCALE: Record<Sprache, string> = { de: "de-DE", en: "en-GB" };

/** A formatter for the Sprache in force, made once per Sprache. */
export function perSprache<F>(make: (locale: string) => F): () => F {
  const made: Partial<Record<Sprache, F>> = {};
  return () => {
    const s = sprache();
    let f = made[s];
    if (f === undefined) {
      f = make(LOCALE[s]);
      made[s] = f;
    }
    return f;
  };
}
