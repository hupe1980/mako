// How a decision leaves the Desk: `keepalive` when the page is closing, and
// the Idempotency-Key the hold gave it, so a request repeated by the browser
// or the gateway is one decision at the service.

export interface Versand {
  /** Outlive the page — a hold flushed on `pagehide`. */
  keepalive: boolean;
  /** The hold's Idempotency-Key; null for a request no hold made. */
  schluessel: string | null;
}

/** A request sent from the page, not from a hold. */
export const SOFORT: Versand = { keepalive: false, schluessel: null };

/** The headers of a decision's request, JSON body or none. */
export function kopf(v: Versand, json: boolean): Record<string, string> {
  return {
    ...(json ? { "content-type": "application/json" } : {}),
    ...(v.schluessel ? { "idempotency-key": v.schluessel } : {}),
  };
}
