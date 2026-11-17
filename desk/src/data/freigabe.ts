// A decision on a Freigabe goes through processd's REST door, under the
// person's own token: processd records who decided (§ 7a Abs. 5 EnWG parity
// evidence, GoBD) and dispatches the answer the entry holds. The Desk sends
// exactly the request its form shows — a proposal only ever fills the form.

import type { FaktKoerper } from "../format/fakt";
import { type Problem, problem, sentence } from "./problem";
import { kopf, SOFORT, type Versand } from "./versand";
import { verweigern } from "./verweigert";

/** The decision body: the code, the Bemerkung, and the E/G facts the entry lists as still needed. */
export interface Entscheidung extends FaktKoerper {
  aktion: "freigeben" | "ablehnen";
  /** For a refusal, and for approving an escalation: the code of the entry's tree. */
  antwort_code?: string;
  antwort_tree?: string;
  bemerkung?: string;
  /** The hash of the body processd rehearsed and the person was shown; a stale one is refused (412). */
  body_sha256?: string;
}

/** processd refused the decision; its words, shown in place. */
export class Abgelehnt extends Error {
  override name = "Abgelehnt";
  /** The stable code (`pruefung_abgelehnt`, `nicht_geprueft`, `json_required`, …). */
  readonly code: string | undefined;
  /** The rules a refused check names (`E_0623`, an AHB rule). */
  readonly regeln: string[];
  readonly stufe: string | undefined;
  readonly grund: string | undefined;
  constructor(message: string, p?: Problem) {
    super(message);
    this.code = p?.code;
    this.regeln = Array.isArray(p?.regeln) ? (p.regeln as unknown[]).map(String) : [];
    this.stufe = typeof p?.stufe === "string" ? p.stufe : undefined;
    this.grund = typeof p?.grund === "string" ? p.grund : undefined;
  }
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
 * Send a decision, under the hold's Idempotency-Key; `keepalive` lets it
 * outlive the page — a decision held for undo and sent on `pagehide`.
 */
export async function entscheiden(id: string, e: Entscheidung, v: Versand = SOFORT): Promise<void> {
  const tuer = e.aktion === "freigeben" ? "approve" : "reject";
  const { aktion: _, ...body } = e;
  const res = await fetch(`/svc/processd/api/v1/queue/${encodeURIComponent(id)}/${tuer}`, {
    method: "POST",
    headers: kopf(v, true),
    body: JSON.stringify(body),
    keepalive: v.keepalive,
  });
  if (res.ok) return;
  const p = await problem(res);
  if (res.status === 409 && p.code === "already_decided") {
    const text = (k: string) => (typeof p[k] === "string" ? (p[k] as string) : null);
    throw new SchonEntschieden(text("status") ?? "", text("decided_by"), text("decided_at"));
  }
  if (res.status === 403)
    throw verweigern({ dienst: "processd", aktion: "entscheiden" }, sentence(p));
  throw new Abgelehnt(sentence(p), p);
}
