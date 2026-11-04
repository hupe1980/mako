// A decision on a Freigabe goes through processd's REST door, under the
// person's own token: processd records who decided (§ 7a Abs. 5 EnWG parity
// evidence, GoBD) and dispatches the answer the entry holds. The Desk sends
// exactly the request its form shows — a proposal only ever fills the form.

export interface Entscheidung {
  aktion: "freigeben" | "ablehnen";
  /** For a refusal, and for approving an escalation: the code of the entry's tree. */
  antwort_code?: string;
  antwort_tree?: string;
  bemerkung?: string;
}

/** processd refused the decision; its words, shown in place. */
export class Abgelehnt extends Error {
  override name = "Abgelehnt";
}

export async function entscheiden(id: string, e: Entscheidung): Promise<void> {
  const tuer = e.aktion === "freigeben" ? "approve" : "reject";
  const { aktion: _, ...body } = e;
  const res = await fetch(`/svc/processd/api/v1/queue/${encodeURIComponent(id)}/${tuer}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
  });
  if (res.ok) return;
  const text = await res.text();
  let message = text;
  try {
    const json = JSON.parse(text) as { error?: string; message?: string };
    message = json.message ?? json.error ?? text;
  } catch {
    // processd answered with text; show it as it came.
  }
  throw new Abgelehnt(message || `Die Prozessüberwachung hat abgelehnt (${res.status}).`);
}
