// A decision on a Rechnungsdisput or a Prüfposten goes through the owning
// service's REST door, under the person's own token: the service checks its
// Cedar policy and records who decided. The Desk sends exactly the request its
// form shows — a proposal only ever fills the form.
//
// Two of them send a message: the Ablehnung of a disputed INVOIC (a REMADV to
// the Rechnungssteller) and the Freigabe of a held invoice (issued to the
// customer). They are confirmed and sent at once. Beilegen and Geprüft send
// nothing and are held for undo.

import { problem, sentence } from "./problem";
import { type Handlung, verweigern } from "./verweigert";

/** The service refused the decision; its words, shown in place. */
export class Abgelehnt extends Error {
  override name = "Abgelehnt";
}

/** The record moved on before this decision arrived — decided by somebody else. */
export class SchonErledigt extends Error {
  override name = "SchonErledigt";
}

async function senden(
  url: string,
  handlung: Handlung,
  body: object | null,
  keepalive = false,
): Promise<void> {
  const res = await fetch(
    url,
    body
      ? {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify(body),
          keepalive,
        }
      : { method: "POST", keepalive },
  );
  if (res.ok) return;
  const p = await problem(res);
  if (res.status === 403) throw verweigern(handlung, sentence(p));
  // Not in Dispute / not HELD / not in REVIEW any more: the record left the
  // state this decision was for.
  if (res.status === 409 || res.status === 404) throw new SchonErledigt(sentence(p));
  throw new Abgelehnt(sentence(p));
}

const RECEIPTS = "/svc/invoicd/api/v1/receipts";
const BILLING = "/svc/billingd/api/v1/billing";
const DISPUT: Handlung = { dienst: "invoicd", aktion: "entscheiden" };

/** Close a dispute settled out of band; sends nothing. */
export function beilegen(id: string, bemerkung: string, keepalive = false): Promise<void> {
  const note = bemerkung.trim();
  return senden(
    `${RECEIPTS}/${encodeURIComponent(id)}/resolve-dispute`,
    DISPUT,
    note ? { note } : {},
    keepalive,
  );
}

/** Send the refusal (REMADV) a disputed receipt is still owed. */
export function ablehnungSenden(id: string): Promise<void> {
  return senden(`${RECEIPTS}/${encodeURIComponent(id)}/dispatch-answer`, DISPUT, null);
}

/** Issue an invoice the risk gate held. */
export function freigeben(id: string): Promise<void> {
  return senden(
    `${BILLING}/${encodeURIComponent(id)}/release`,
    { dienst: "billingd", aktion: "freigeben" },
    null,
  );
}

/** Record that an issued invoice under review was looked at; sends nothing. */
export function geprueft(id: string, keepalive = false): Promise<void> {
  return senden(
    `${BILLING}/${encodeURIComponent(id)}/reviewed`,
    { dienst: "billingd", aktion: "pruefen" },
    null,
    keepalive,
  );
}
