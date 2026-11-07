// A geparkte Buchung — a draft the ledger refused for good — is resolved
// through accountingd's REST door, under the person's own token: the service
// checks its Cedar policy (`post-entry`) and records who resolved it. Both
// moves are final, so the Desk confirms each before it sends; neither is held.
//
// The draft's key carries `:`, so it is always encoded into the path.

import type { BusinessDate } from "../format/brands";
import { problem, sentence } from "./problem";
import { Abgelehnt, SchonErledigt } from "./rechnung";
import { type Handlung, verweigern } from "./verweigert";

/** The rebooking was taken, and the ledger refused it again: it is parked anew. */
export class ErneutGeparkt extends Error {
  override name = "ErneutGeparkt";
}

const BOOKINGS = "/svc/accountingd/api/v1/bookings";
const BUCHEN: Handlung = { dienst: "accountingd", aktion: "buchen" };

async function senden(url: string, body: object): Promise<void> {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (res.ok) return;
  const p = await problem(res);
  if (res.status === 403) throw verweigern(BUCHEN, sentence(p));
  if (res.status === 409) throw new ErneutGeparkt(sentence(p));
  if (res.status === 404) throw new SchonErledigt(sentence(p));
  throw new Abgelehnt(sentence(p));
}

/** Book a parked draft on `buchungsdatum`, which must lie in an open month. */
export function neuBuchen(key: string, buchungsdatum: BusinessDate): Promise<void> {
  return senden(`${BOOKINGS}/${encodeURIComponent(key)}/rebook`, { buchungsdatum });
}

/** Drop a parked draft; `grund` is kept with it for the audit trail. */
export function verwerfen(key: string, grund: string): Promise<void> {
  return senden(`${BOOKINGS}/${encodeURIComponent(key)}/discard`, { grund: grund.trim() });
}

/** Each Buchungsart accountingd books, as the Lieferant reads it. */
const ART: Record<string, string> = {
  RECHNUNG: "Rechnung",
  STORNO: "Storno",
  KORREKTUR: "Korrektur",
  GUTSCHRIFT: "Gutschrift",
  ABSCHLAG: "Abschlag",
  ABSCHLAG_VERRECHNUNG: "Abschlagsverrechnung",
  ZAHLUNG: "Zahlung",
  BANKRUECKLAST: "Bankrücklastschrift",
  SEPA_STORNO: "SEPA-Storno",
  MAHNGEBUEHR: "Mahngebühr",
  VERZUGSZINSEN: "Verzugszinsen",
  EEG_GUTSCHRIFT: "EEG-Gutschrift",
  EEG_MARKTPRAEMIE: "EEG-Marktprämie",
  JAHRESABSCHLUSS: "Jahresabschluss",
};

/** A Buchungsart in words; one the Desk does not know is shown as sent. */
export function buchungsart(art: string): string {
  return ART[art] ?? art;
}

/**
 * The ledger's refusal code in a few words, for a row's headline; the
 * service's own `hinweis` says what to do about it.
 */
export function abgelehntWeil(code: number | null): string {
  switch (code) {
    case 301:
      return "Periode festgeschrieben";
    case 300:
      return "keine Periode für das Datum";
    case 304:
      return "Konto an dem Tag nicht eröffnet";
    case 200:
      return "Schlüssel trägt eine andere Buchung";
    case null:
      return "Buchung unvollständig";
    default:
      return "vom Hauptbuch abgelehnt";
  }
}
