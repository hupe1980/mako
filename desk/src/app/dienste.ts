// What an operator calls each service. The Desk names no internal service —
// `vertragd` is a deployment's word, „Kundenverwaltung" the operator's — in a
// caption, an error or a source chip.

import type { Service } from "../data/mcp";

export const DIENST: Record<Service | "agentd", string> = {
  makod: "Marktkommunikation",
  marktd: "Marktstammdaten",
  processd: "Prozessüberwachung",
  vertragd: "Kundenverwaltung",
  billingd: "Abrechnung",
  invoicd: "Rechnungseingang",
  edmd: "Messwerte",
  netzbilanzd: "Netzentgelte",
  accountingd: "Kundenkonten",
  agentd: "Assistent",
};

/** Every service an assistant skill may read, as an operator knows it. */
const ALLE: Record<string, string> = {
  ...DIENST,
  einsd: "EEG-Abrechnung",
  productd: "Produktkatalog",
  portald: "Kundenportal",
  sperrd: "Sperrungen",
  outputd: "Dokumente",
};

/** The title of a notice for a service that did not answer. */
export function nichtErreichbar(service: Service | "agentd"): string {
  return `${DIENST[service]} ist nicht erreichbar`;
}

const WERKZEUG: Record<string, string> = {
  get_process: "Prozesse",
  get_process_history: "Verlauf",
  get_queue_entry: "Freigabe",
  get_kunde: "Kunde",
  search_kunden: "Kundensuche",
  kunden_zu_malos: "Kunden",
  list_anmeldungen: "Anmeldungen",
  list_open_processes: "offene Prozesse",
  list_overdue_processes: "Fristen",
  get_receipt: "Rechnung",
  get_check_result: "Prüfbefunde",
  list_parked_bookings: "geparkte Buchungen",
  get_parked_booking: "geparkte Buchung",
  list_ledger: "Kontoauszug",
};

/** The service a `server/tool` reference reaches, as an operator knows it. */
export function dienstVon(ref: string): string {
  return ALLE[ref.split("/")[0] ?? ""] ?? "Dienst";
}

/** A tool call an answer cites, `server/tool`, as an operator reads it. */
export function quelle(ref: string): string {
  const [server = "", tool = ""] = ref.split("/");
  const dienst = ALLE[server] ?? "Dienst";
  const was = WERKZEUG[tool];
  return was ? `${dienst} · ${was}` : dienst;
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
    ? `${WERKZEUG[s.quelle.werkzeug] ?? ALLE[s.quelle.dienst] ?? "Dienst"} ${s.was}`
    : `${s.was.charAt(0).toUpperCase()}${s.was.slice(1)}`;
  return s.objekt ? `${was} · ${s.objekt}` : was;
}
