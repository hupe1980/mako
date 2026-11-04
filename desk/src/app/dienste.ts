// What an operator calls each service. The Desk names no internal service —
// `vertragd` is a deployment's word, „Kundenverwaltung" the operator's — in a
// caption, an error or a source chip.

import type { Service } from "../data/mcp";

export const DIENST: Record<Service | "agentd", string> = {
  makod: "Marktkommunikation",
  processd: "Prozessüberwachung",
  vertragd: "Kundenverwaltung",
  agentd: "Assistent",
};

/** Every service an assistant skill may read, as an operator knows it. */
const ALLE: Record<string, string> = {
  ...DIENST,
  marktd: "Marktstammdaten",
  billingd: "Abrechnung",
  invoicd: "Rechnungseingang",
  accountingd: "Kundenkonten",
  edmd: "Messwerte",
  einsd: "EEG-Abrechnung",
  netzbilanzd: "Netzentgelte",
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
