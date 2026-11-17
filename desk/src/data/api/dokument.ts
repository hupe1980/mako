// The services' OpenAPI documents merged the way the gateway routes them:
// each path under `/svc/<service>`, each component schema under
// `<service>.<Name>`, so two services may both call a type `Status`. The
// generator (`scripts/gen-api.ts`) writes the merged document and the types
// `openapi-fetch` reads; the drift banner fingerprints a served document the
// same way.

import { fnv, kanonisch } from "../oberflaeche.ts";

/** An OpenAPI 3.1 document, as far as the merge reads it. */
export interface Dokument {
  openapi: string;
  info: { title: string; version: string };
  paths?: Record<string, Record<string, unknown>>;
  components?: { schemas?: Record<string, unknown> };
}

/** What the Desk was generated against, per service. */
export interface Stand {
  version: string;
  /** `fnv` over the canonical document — changes with any change to it. */
  abdruck: string;
}

/** The fingerprint of a document, independent of key order. */
export function abdruck(d: unknown): string {
  return fnv(kanonisch(d));
}

/** Rewrite every `#/components/schemas/X` in `node` to the service's name for X. */
function umbenennen(node: unknown, service: string): unknown {
  if (Array.isArray(node)) return node.map((n) => umbenennen(n, service));
  if (node === null || typeof node !== "object") return node;
  return Object.fromEntries(
    Object.entries(node as Record<string, unknown>).map(([k, v]) => [
      k,
      k === "$ref" && typeof v === "string" && v.startsWith("#/components/schemas/")
        ? `#/components/schemas/${service}.${v.slice("#/components/schemas/".length)}`
        : umbenennen(v, service),
    ]),
  );
}

/** The merged document's name — a developer's, never shown on screen. */
const ZUSAMMEN = "mako services through the Desk gateway";

/** Merge each service's document into one, as the gateway serves them. */
export function zusammenfuehren(dokumente: Record<string, Dokument>): {
  dokument: Dokument;
  staende: Record<string, Stand>;
} {
  const paths: Record<string, Record<string, unknown>> = {};
  const schemas: Record<string, unknown> = {};
  const staende: Record<string, Stand> = {};
  for (const service of Object.keys(dokumente).sort()) {
    const d = dokumente[service] as Dokument;
    staende[service] = { version: d.info.version, abdruck: abdruck(d) };
    for (const [pfad, ops] of Object.entries(d.paths ?? {}))
      paths[`/svc/${service}${pfad}`] = umbenennen(ops, service) as Record<string, unknown>;
    for (const [name, schema] of Object.entries(d.components?.schemas ?? {}))
      schemas[`${service}.${name}`] = umbenennen(schema, service);
  }
  return {
    dokument: {
      openapi: "3.1.0",
      info: { title: ZUSAMMEN, version: "merged" },
      paths,
      components: { schemas },
    },
    staende,
  };
}
