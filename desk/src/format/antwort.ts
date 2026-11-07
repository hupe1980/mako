/**
 * What a market answer said, for its Verlauf line: the code, and the BDEW's
 * wording from the list that defines it. Without a wording the code stands
 * with its list — never with a meaning guessed from another tree.
 */
export interface AntwortTeile {
  code: string;
  text: string;
}

export function antwortTeile(entry: {
  antwortcode?: string | null;
  codeliste?: string | null;
  bedeutung?: string | null;
}): AntwortTeile | null {
  if (!entry.antwortcode) return null;
  if (entry.bedeutung) return { code: entry.antwortcode, text: entry.bedeutung };
  return {
    code: entry.antwortcode,
    text: entry.codeliste ? `aus ${entry.codeliste}, ohne Wortlaut im Katalog` : "ohne Codeliste",
  };
}
