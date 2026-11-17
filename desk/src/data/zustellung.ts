// A service's webhook subscriptions (`mako_service::subscriptions`), the API an
// integrator uses: every Desk service that serves it, under the same paths
// and shapes, so one module reads and writes them all. Types are edmd's —
// the shared module's — from the generated documents.

import type {
  EdmdDelivery,
  EdmdEndpoint,
  EdmdEndpointCreated,
  EdmdTestDelivery,
} from "./api/schema";
import type { NurRest, Service } from "./mcp";
import { ServiceUnavailable } from "./mcp";
import { problem, sentence } from "./problem";

export type Endpunkt = EdmdEndpoint;
export type Zustellung = EdmdDelivery;

/** The services whose subscription API the Desk's gateway reaches. */
export const MIT_ZUSTELLUNGEN = [
  "processd",
  "marktd",
  "vertragd",
  "edmd",
  "invoicd",
  "billingd",
  "netzbilanzd",
  "productd",
] as const satisfies readonly (Service | NurRest)[];
export type ZustellDienst = (typeof MIT_ZUSTELLUNGEN)[number];

/** The service refused; its stable code says why. */
export class ZustellungAbgelehnt extends Error {
  override name = "ZustellungAbgelehnt";
  readonly code: string | undefined;
  constructor(message: string, code: string | undefined) {
    super(message);
    this.code = code;
  }
}

async function anfrage<T>(dienst: ZustellDienst, pfad: string, init?: RequestInit): Promise<T> {
  let res: Response;
  try {
    res = await fetch(`/svc/${dienst}/api/v1${pfad}`, {
      ...init,
      headers: {
        accept: "application/json",
        ...(init?.body ? { "content-type": "application/json" } : {}),
      },
    });
  } catch (e) {
    throw new ServiceUnavailable(dienst, e);
  }
  if (res.ok) return (res.status === 204 ? null : await res.json()) as T;
  const p = await problem(res);
  if (res.status >= 500) throw new ServiceUnavailable(dienst, new Error(sentence(p)));
  throw new ZustellungAbgelehnt(sentence(p), p.code);
}

const post = <T>(dienst: ZustellDienst, pfad: string, body?: object) =>
  anfrage<T>(dienst, pfad, { method: "POST", ...(body ? { body: JSON.stringify(body) } : {}) });

export const endpunkte = (d: ZustellDienst) =>
  anfrage<{ items: Endpunkt[] }>(d, "/webhooks/endpoints?limit=100");
export const zustellungen = (d: ZustellDienst, id: string) =>
  anfrage<{ items: Zustellung[] }>(
    d,
    `/webhooks/endpoints/${encodeURIComponent(id)}/deliveries?limit=50`,
  );
export const anlegen = (
  d: ZustellDienst,
  body: { name: string; url: string; event_types?: string[] },
) => post<EdmdEndpointCreated>(d, "/webhooks/endpoints", body);
export const testen = (d: ZustellDienst, id: string) =>
  post<EdmdTestDelivery>(d, `/webhooks/endpoints/${encodeURIComponent(id)}/test`);
export const schluesselErneuern = (d: ZustellDienst, id: string, overlapHours: number) =>
  post<{ secret: string; previous_expires_at: string }>(
    d,
    `/webhooks/endpoints/${encodeURIComponent(id)}/rotate-secret`,
    { overlap_hours: overlapHours },
  );
export const erneutSenden = (d: ZustellDienst, zustellung: string) =>
  post<{ queued: number }>(d, `/webhooks/deliveries/${encodeURIComponent(zustellung)}/replay`);
export const zeitraumErneut = (d: ZustellDienst, id: string, from: string, to: string) =>
  post<{ queued: number }>(d, `/webhooks/endpoints/${encodeURIComponent(id)}/replay`, { from, to });
