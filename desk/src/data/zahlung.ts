// Writes on accountingd's money: assign a payment to the account the service
// proposed, mark a Guthaben paid. Held for undo like every decision and sent
// with the hold's Idempotency-Key; a race lost is said in accountingd's code.

import { problem, sentence } from "./problem";
import { kopf, type Versand } from "./versand";
import { verweigern } from "./verweigert";

/** accountingd refused; its stable code says why (`payment_already_assigned`, …). */
export class ZahlungAbgelehnt extends Error {
  override name = "ZahlungAbgelehnt";
  readonly code: string | undefined;
  constructor(message: string, code: string | undefined) {
    super(message);
    this.code = code;
  }
}

async function senden(url: string, body: object, v: Versand): Promise<void> {
  const res = await fetch(url, {
    method: "POST",
    headers: kopf(v, true),
    body: JSON.stringify(body),
    keepalive: v.keepalive,
  });
  if (res.ok) return;
  const p = await problem(res);
  if (res.status === 403)
    throw verweigern({ dienst: "accountingd", aktion: "buchen" }, sentence(p));
  throw new ZahlungAbgelehnt(sentence(p), p.code);
}

/** Assign a received payment to a Marktlokation's account. */
export function zuordnen(
  paymentId: string,
  ziel: { malo_id: string; lf_mp_id?: string },
  v: Versand,
): Promise<void> {
  return senden(
    `/svc/accountingd/api/v1/payments/${encodeURIComponent(paymentId)}/assign`,
    ziel,
    v,
  );
}

/** Record a Guthaben as paid out on `paid_on`. */
export function ausgezahlt(
  payoutId: string,
  body: { paid_on: string; reference?: string },
  v: Versand,
): Promise<void> {
  return senden(
    `/svc/accountingd/api/v1/credit-payouts/${encodeURIComponent(payoutId)}/paid`,
    body,
    v,
  );
}
