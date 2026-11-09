// A geparkte Buchung — a draft the ledger refused for good — is resolved
// through accountingd's REST door, under the person's own token: the service
// checks its Cedar policy (`post-entry`) and records who resolved it. Both
// moves are final once sent, so each is held for undo before it leaves — in
// its view as in ⌘K — and neither asks confirmation.
//
// The draft's key carries `:`, so it is always encoded into the path.

import type { BusinessDate } from "../format/brands";
import { eintrag, katalog } from "../i18n";
import { problem, sentence } from "./problem";
import { Abgelehnt, SchonErledigt } from "./rechnung";
import { type Handlung, verweigern } from "./verweigert";

/** The rebooking was taken, and the ledger refused it again: it is parked anew. */
export class ErneutGeparkt extends Error {
  override name = "ErneutGeparkt";
}

const BOOKINGS = "/svc/accountingd/api/v1/bookings";
const BUCHEN: Handlung = { dienst: "accountingd", aktion: "buchen" };

async function senden(url: string, body: object, keepalive: boolean): Promise<void> {
  const res = await fetch(url, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    keepalive,
  });
  if (res.ok) return;
  const p = await problem(res);
  if (res.status === 403) throw verweigern(BUCHEN, sentence(p));
  if (res.status === 409) throw new ErneutGeparkt(sentence(p));
  if (res.status === 404) throw new SchonErledigt(sentence(p));
  throw new Abgelehnt(sentence(p));
}

/** Book a parked draft on `buchungsdatum`, which must lie in an open month. */
export function neuBuchen(
  key: string,
  buchungsdatum: BusinessDate,
  keepalive = false,
): Promise<void> {
  return senden(`${BOOKINGS}/${encodeURIComponent(key)}/rebook`, { buchungsdatum }, keepalive);
}

/** Drop a parked draft; `grund` is kept with it for the audit trail. */
export function verwerfen(key: string, grund: string, keepalive = false): Promise<void> {
  return senden(
    `${BOOKINGS}/${encodeURIComponent(key)}/discard`,
    { grund: grund.trim() },
    keepalive,
  );
}

/** A Buchungsart in words (the catalogue's `buchungsarten`); one the Desk does not know is shown as sent. */
export function buchungsart(art: string): string {
  return eintrag(katalog().buchungsarten.art, art);
}

/**
 * The ledger's refusal code in a few words, for a row's headline; the
 * service's own `hinweis` says what to do about it.
 */
export function abgelehntWeil(code: number | null): string {
  const w = katalog().buchungsarten.abgelehnt;
  switch (code) {
    case 301:
      return w.festgeschrieben;
    case 300:
      return w.keinePeriode;
    case 304:
      return w.kontoNichtEroeffnet;
    case 200:
      return w.schluesselBelegt;
    case null:
      return w.unvollstaendig;
    default:
      return w.abgelehnt;
  }
}
