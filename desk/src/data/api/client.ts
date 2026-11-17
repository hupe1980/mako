// The Desk's REST door: `openapi-fetch` over the types `npm run gen` writes
// from the services' committed OpenAPI documents (`schema.ts`). A path or
// method no document describes does not type-check — the Desk calls what
// integrators call, nothing else. Views move here service by service as each
// document lands; until then they read the service's MCP tools (`../mcp.ts`).

import createClient from "openapi-fetch";
import { type Service, ServiceUnavailable } from "../mcp";
import { problem, sentence } from "../problem";
import type { paths } from "./schema";

const origin = typeof location === "undefined" ? "http://localhost" : location.origin;

/** `fetch` is looked up per call, so a test double installed later is used. */
export const api = createClient<paths>({
  baseUrl: origin,
  fetch: (request) => globalThis.fetch(request),
});

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

type Ergebnis = { data?: unknown; error?: unknown; response: Response };

/**
 * Await an `openapi-fetch` call: the data on success; the service's refusal
 * as `RestAbgelehnt`; a service that could not be reached, or failed before it
 * answered (5xx), as `ServiceUnavailable` — the same split the MCP door makes.
 */
export async function rest<R extends Ergebnis>(
  service: Service,
  request: Promise<R>,
): Promise<NonNullable<R["data"]>> {
  let r: R;
  try {
    r = await request;
  } catch (e) {
    throw new ServiceUnavailable(service, e);
  }
  if (r.response.ok) return r.data as NonNullable<R["data"]>;
  const p = await problem(
    new Response(r.error === undefined ? "" : JSON.stringify(r.error), {
      status: r.response.status,
    }),
  );
  if (r.response.status >= 500) throw new ServiceUnavailable(service, new Error(sentence(p)));
  throw new RestAbgelehnt(r.response.status, p.code, sentence(p));
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
 * service's document. `openapi-fetch` builds the request; its own mapping of
 * the answer's type would lose the Desk's brands, so the answer is typed here.
 */
export function lesen<P extends GetPfad>(
  service: Service,
  pfad: P,
  params: Parameter<P>,
): Promise<Antwort<P>> {
  return rest(service, api.GET(pfad as never, { params } as never)) as Promise<Antwort<P>>;
}
