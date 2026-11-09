// What an operator calls each service. The Desk names no internal service —
// `vertragd` is a deployment's word, „Kundenverwaltung" the operator's — in a
// caption, an error or a source chip.

import type { Service } from "../data/mcp";
import { katalog } from "../i18n";

type Dienstname = keyof ReturnType<typeof katalog>["dienstNamen"];

/** The name of `id` in the Sprache in force; read each time it is shown. */
function benannt<K extends Dienstname>(ids: readonly K[]): Record<K, string> {
  const out = {} as Record<K, string>;
  for (const id of ids)
    Object.defineProperty(out, id, { enumerable: true, get: () => katalog().dienstNamen[id] });
  return out;
}

const GELESEN = [
  "makod",
  "marktd",
  "processd",
  "vertragd",
  "billingd",
  "invoicd",
  "edmd",
  "netzbilanzd",
  "accountingd",
  "agentd",
] as const satisfies readonly (Service | "agentd")[];

/** The services the Desk reads, as an operator calls them. */
export const DIENST: Record<Service | "agentd", string> = benannt(GELESEN);

/** Every service an assistant skill may read, as an operator knows it. */
const ALLE: Record<string, string> = benannt([
  ...GELESEN,
  "einsd",
  "productd",
  "portald",
  "sperrd",
  "outputd",
]);

/** The fallback for a service the Desk does not know. */
const dienst = () => katalog().dienstNamen.unbekannt;

/** The title of a notice for a service that did not answer. */
export function nichtErreichbar(service: Service | "agentd"): string {
  return katalog().dienst.nichtErreichbar(DIENST[service]);
}

const werkzeug = (tool: string): string | undefined => katalog().werkzeugNamen[tool];

/** The service a `server/tool` reference reaches, as an operator knows it. */
export function dienstVon(ref: string): string {
  return ALLE[ref.split("/")[0] ?? ""] ?? dienst();
}

/** A tool call an answer cites, `server/tool`, as an operator reads it. */
export function quelle(ref: string): string {
  const [server = "", tool = ""] = ref.split("/");
  const name = ALLE[server] ?? dienst();
  const was = werkzeug(tool);
  return was ? `${name} · ${was}` : name;
}

/**
 * One read of the assistant's trail as a line: what the tool reads as an
 * operator calls it, agentd's verb, then the object agentd names —
 * „Verlauf gelesen · Prozess 4711". A read whose source is hidden keeps
 * agentd's verb and object alone.
 */
export function schrittZeile(s: {
  was: string;
  objekt: string | null;
  quelle: { dienst: string; werkzeug: string } | null;
}): string {
  const was = s.quelle
    ? `${werkzeug(s.quelle.werkzeug) ?? ALLE[s.quelle.dienst] ?? dienst()} ${s.was}`
    : `${s.was.charAt(0).toUpperCase()}${s.was.slice(1)}`;
  return s.objekt ? `${was} · ${s.objekt}` : was;
}
