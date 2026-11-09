// The facts a Freigabe lists, as the decision body carries them: processd
// names each fact and the values its segment admits as text, and the body
// takes the Abrechnungsintervall as the number of months it is.

import type { Fakt } from "../data/gen/processd";

/** The facts of a decision body. */
export interface FaktKoerper {
  bilanzkreis?: string;
  versorgungsart?: string;
  abrechnungsintervall_monate?: number;
}

/** The body fields of the facts filled in for `fakten`; a fact left empty is left out. */
export function faktKoerper(fakten: Fakt[], werte: Record<string, string>): FaktKoerper {
  const k: FaktKoerper = {};
  for (const fakt of fakten) {
    const wert = werte[fakt.feld]?.trim();
    if (!wert) continue;
    if (fakt.feld === "bilanzkreis") k.bilanzkreis = wert;
    else if (fakt.feld === "versorgungsart") k.versorgungsart = wert;
    else if (fakt.feld === "abrechnungsintervall_monate")
      k.abrechnungsintervall_monate = Number(wert);
  }
  return k;
}
