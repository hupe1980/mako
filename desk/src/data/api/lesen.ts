// The REST door's reads: a GET typed by the services' documents
// (`schema.ts`), built without a client library so Heute's sources stay light.
// Writes and the views that make them use `openapi-fetch` (`client.ts`).

import { type NurRest, type Service, ServiceUnavailable } from "../mcp";
import { problem, sentence } from "../problem";
import type { paths } from "./schema";

/** The service answered and said no — not found, not permitted, not valid — in its words. */
export class RestAbgelehnt extends Error {
  override name = "RestAbgelehnt";
  readonly status: number;
  /** The service's stable code, for the catalogue's sentence. */
  readonly code: string | undefined;
  constructor(status: number, code: string | undefined, message: string) {
    super(message);
    this.status = status;
    this.code = code;
  }
}

/** The paths a GET reaches. */
type GetPfad = {
  [P in keyof paths]: paths[P] extends { get: unknown } ? P : never;
}[keyof paths];

type GetOperation<P extends GetPfad> = paths[P] extends { get: infer O } ? O : never;

/** A GET's parameters, as the document states them. */
export type Parameter<P extends GetPfad> =
  GetOperation<P> extends { parameters: infer X } ? X : never;

/** A GET's 200 body, exactly as the generated types name it (branded dates and decimals kept). */
export type Antwort<P extends GetPfad> =
  GetOperation<P> extends {
    responses: { 200: { content: { "application/json": infer T } } };
  }
    ? T
    : never;

/**
 * Read `pfad` of `service`: the parameters and the answer typed by the
 * service's document, the request built here — a GET needs no client, so the
 * first paint carries none (`openapi-fetch` comes with the views that write).
 */
export async function lesen<P extends GetPfad>(
  service: Service | NurRest,
  pfad: P,
  params: Parameter<P>,
  signal?: AbortSignal,
): Promise<Antwort<P>> {
  const p = params as { path?: Record<string, unknown>; query?: Record<string, unknown> };
  let url = pfad as string;
  for (const [k, v] of Object.entries(p.path ?? {}))
    url = url.replace(`{${k}}`, encodeURIComponent(String(v)));
  const q = new URLSearchParams();
  for (const [k, v] of Object.entries(p.query ?? {})) {
    if (v === null || v === undefined) continue;
    for (const x of Array.isArray(v) ? v : [v]) q.append(k, String(x));
  }
  const suche = q.toString();
  let res: Response;
  try {
    res = await fetch(suche ? `${url}?${suche}` : url, {
      headers: { accept: "application/json" },
      ...(signal && { signal }),
    });
  } catch (e) {
    throw new ServiceUnavailable(service, e);
  }
  if (res.ok) return (await res.json()) as Antwort<P>;
  const pr = await problem(res);
  if (res.status >= 500) throw new ServiceUnavailable(service, new Error(sentence(pr)));
  throw new RestAbgelehnt(res.status, pr.code, sentence(pr));
}
