// A decision on a Klärfall goes through processd's REST door under the
// person's own token, held for undo like every decision and sent with the
// hold's Idempotency-Key: send the correction, close the case, or take it.

import { problem, sentence } from "./problem";
import { kopf, type Versand } from "./versand";
import { verweigern } from "./verweigert";

/** processd refused it; its stable code says why (`begruendung_required`, `case_closed`, …). */
export class KlaerfallAbgelehnt extends Error {
  override name = "KlaerfallAbgelehnt";
  readonly code: string | undefined;
  constructor(message: string, code: string | undefined) {
    super(message);
    this.code = code;
  }
}

export type KlaerfallTuer = "dispatch" | "resolve" | "assign";

/** Send `body` to the case's door. */
export async function klaerfallSenden(
  id: string,
  tuer: KlaerfallTuer,
  body: Record<string, unknown>,
  v: Versand,
): Promise<void> {
  const res = await fetch(`/svc/processd/api/v1/klaerfaelle/${encodeURIComponent(id)}/${tuer}`, {
    method: "POST",
    headers: kopf(v, true),
    body: JSON.stringify(body),
    keepalive: v.keepalive,
  });
  if (res.ok) return;
  const p = await problem(res);
  if (res.status === 403)
    throw verweigern({ dienst: "processd", aktion: "entscheiden" }, sentence(p));
  throw new KlaerfallAbgelehnt(sentence(p), p.code);
}
