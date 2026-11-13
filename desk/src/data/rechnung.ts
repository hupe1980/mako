// A decision on a Rechnungsdisput or a Prüfposten goes through the owning
// service's REST door, under the person's own token: the service checks its
// Cedar policy and records who decided. The Desk sends exactly the request its
// form shows — a proposal only ever fills the form.
//
// Two of them send a message: the Ablehnung of a disputed INVOIC (a REMADV to
// the Rechnungssteller) and the Freigabe of a held invoice (issued to the
// customer). Like Beilegen and Geprüft, which send nothing, each is held for
// undo before it leaves — in its view as in ⌘K — and none asks confirmation.

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
export function ablehnungSenden(id: string, keepalive = false): Promise<void> {
  return senden(`${RECEIPTS}/${encodeURIComponent(id)}/dispatch-answer`, DISPUT, null, keepalive);
}

const FESTSTELLEN: Handlung = { dienst: "invoicd", aktion: "feststellen" };

/** A held invoice's open Prüfschritt, as invoicd lists it. */
export interface OffenerSchritt {
  pruefschritt: number;
  /** The `SG26 LIN` Positionsnummer on the Positionsebene; null on Kopf and Summe. */
  position: number | null;
  detail: string;
}

/**
 * The open Prüfschritte of a held invoice, read from invoicd's `offen` —
 * `{ ebene: { ebene: "position", position }, pruefschritt, detail }`. An entry
 * of another shape is kept with what it names, never dropped.
 */
export function offeneSchritte(offen: unknown[]): OffenerSchritt[] {
  return offen.flatMap((o) => {
    if (!o || typeof o !== "object") return [];
    const r = o as { pruefschritt?: unknown; detail?: unknown; ebene?: unknown };
    if (typeof r.pruefschritt !== "number") return [];
    const ebene = r.ebene as { position?: unknown } | string | undefined;
    const position =
      ebene && typeof ebene === "object" && typeof ebene.position === "number"
        ? ebene.position
        : null;
    return [
      {
        pruefschritt: r.pruefschritt,
        position,
        detail: typeof r.detail === "string" ? r.detail : JSON.stringify(o),
      },
    ];
  });
}

/** The answers a statement gives, as `POST /receipts/{id}/feststellungen` takes them. */
export interface Feststellungen {
  leistung_erbracht?: Record<string, boolean>;
  preis_wie_angebot?: Record<string, boolean>;
  steuersatz_korrekt?: Record<string, boolean>;
  preisblatt_version_gueltig?: boolean;
}

/** State what a held invoice's walk could not read; invoicd walks it again. */
export function feststellen(
  id: string,
  feststellungen: Feststellungen,
  begruendung: string,
  keepalive = false,
): Promise<void> {
  return senden(
    `${RECEIPTS}/${encodeURIComponent(id)}/feststellungen`,
    FESTSTELLEN,
    { feststellungen, begruendung: begruendung.trim() },
    keepalive,
  );
}

/** A Leistungsnachweis for an ordered Leistung (Prüfschritt 310), as invoicd records it. */
export interface Leistungsnachweis {
  rechnungssteller_mp_id: string;
  bestellung_ref: string;
  artikel_id: string;
  /** `YYYY-MM-DD`. */
  von: string;
  /** `YYYY-MM-DD`, inclusive. */
  bis: string;
  erbracht: boolean;
  nachweis: string;
}

/** Record a Leistungsnachweis; every invoice held on its order walks again. */
export function leistungsnachweisErfassen(n: Leistungsnachweis, keepalive = false): Promise<void> {
  return senden("/svc/invoicd/api/v1/leistungsnachweise", FESTSTELLEN, n, keepalive);
}

/** Issue an invoice the risk gate held. */
export function freigeben(id: string, keepalive = false): Promise<void> {
  return senden(
    `${BILLING}/${encodeURIComponent(id)}/release`,
    { dienst: "billingd", aktion: "freigeben" },
    null,
    keepalive,
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
