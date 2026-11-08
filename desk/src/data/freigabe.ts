// A decision on a Freigabe goes through processd's REST door, under the
// person's own token: processd records who decided (§ 7a Abs. 5 EnWG parity
// evidence, GoBD) and dispatches the answer the entry holds. The Desk sends
// exactly the request its form shows — a proposal only ever fills the form.

import { problem, sentence } from "./problem";
import { verweigern } from "./verweigert";

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

/** Somebody decided the Freigabe first: who, when, and how. */
export class SchonEntschieden extends Error {
  override name = "SchonEntschieden";
  readonly status: string;
  readonly decidedBy: string | null;
  readonly decidedAt: string | null;
  constructor(status: string, decidedBy: string | null, decidedAt: string | null) {
    super(`already ${status}`);
    this.status = status;
    this.decidedBy = decidedBy;
    this.decidedAt = decidedAt;
  }
}

/**
 * Send a decision. `keepalive` lets it outlive the page — a decision held for
 * undo and sent on `pagehide`.
 */
export async function entscheiden(id: string, e: Entscheidung, keepalive = false): Promise<void> {
  const tuer = e.aktion === "freigeben" ? "approve" : "reject";
  const { aktion: _, ...body } = e;
  const res = await fetch(`/svc/processd/api/v1/queue/${encodeURIComponent(id)}/${tuer}`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    keepalive,
  });
  if (res.ok) return;
  const p = await problem(res);
  if (res.status === 409 && p.code === "already_decided") {
    const text = (k: string) => (typeof p[k] === "string" ? (p[k] as string) : null);
    throw new SchonEntschieden(text("status") ?? "", text("decided_by"), text("decided_at"));
  }
  if (res.status === 403)
    throw verweigern({ dienst: "processd", aktion: "entscheiden" }, sentence(p));
  throw new Abgelehnt(sentence(p));
}
