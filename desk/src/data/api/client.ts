// The Desk's REST door: `openapi-fetch` over the types `npm run gen` writes
// from the services' committed OpenAPI documents (`schema.ts`). A path or
// method no document describes does not type-check — the Desk calls what
// integrators call, nothing else. Views move here service by service as each
// document lands; until then they read the service's MCP tools (`../mcp.ts`).

import createClient from "openapi-fetch";
import { type Service, ServiceUnavailable } from "../mcp";
import { problem, sentence } from "../problem";
import { RestAbgelehnt } from "./lesen";
import type { paths } from "./schema";

const origin = typeof location === "undefined" ? "http://localhost" : location.origin;

/**
 * `fetch` is looked up per call, so a test double installed later is used,
 * and called as the MCP door calls it — a same-origin path and an init — so
 * one double answers both doors.
 */
export const api = createClient<paths>({
  baseUrl: origin,
  fetch: async (request) => {
    const url = new URL(request.url);
    const body =
      request.method === "GET" || request.method === "HEAD" ? undefined : await request.text();
    return globalThis.fetch(`${url.pathname}${url.search}`, {
      method: request.method,
      headers: request.headers,
      ...(body !== undefined ? { body } : {}),
    });
  },
});

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

export { type Antwort, lesen, type Parameter, RestAbgelehnt } from "./lesen";
