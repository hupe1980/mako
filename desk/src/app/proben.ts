// The messages the Einrichtung rehearses: each a command whose door makod can
// stop at its pre-send check (makod's `probe::REHEARSABLE`), with sample facts
// to start from. The samples are the published fixture values the profiles
// and makod's own tests use — the canonical MaLo-ID `51238696012`, Erika
// Mustermann in Köln — never a real customer's data. The operator edits them
// before a run; the verdict is makod's.

import type { Katalog } from "../i18n";

/** A rehearsable command — each one the catalogue names. */
export type ProbeCommand = keyof Katalog["einrichtung"]["probe"]["arten"];

export interface ProbeArt {
  /** The command makod rehearses. */
  command: ProbeCommand;
  /** The Prüfidentifikator the door writes, as the label names it. */
  pid: string;
  /** The payload to start from, exactly as the command takes it. */
  beispiel: Record<string, unknown>;
}

const MALO = "51238696012";
const KUNDE = {
  vorname: "Erika",
  nachname: "Mustermann",
  anschrift: {
    strasse: "Hauptstraße",
    hausnummer: "1",
    postleitzahl: "50667",
    ort: "Köln",
    landescode: "DE",
  },
};

export const PROBEN: ProbeArt[] = [
  {
    command: "gpke.lieferbeginn.anmelden",
    pid: "55001",
    beispiel: {
      malo_id: MALO,
      lieferbeginn_datum: "2027-01-01",
      transaktionsgrund: "E03",
      bilanzkreis: "11XBK-LF-------9",
      haushaltskunde: true,
      enfg_grundlage: "ZG1",
      kunde: KUNDE,
    },
  },
  {
    command: "gpke.lieferende.anmelden",
    pid: "55004",
    beispiel: { malo_id: MALO, lieferende_datum: "2026-12-31", transaktionsgrund: "ZT5" },
  },
  {
    command: "gpke.kuendigung.anmelden",
    pid: "55016",
    beispiel: { malo_id: MALO, kuendigung_datum: "2026-12-31", lfa_mp_id: "9905550000010" },
  },
  {
    command: "geli.lieferbeginn.anmelden",
    pid: "44001",
    beispiel: {
      malo_id: MALO,
      process_date: "20270101",
      transaktionsgrund: "E03",
      bilanzkreis: "THE0BFH000000001",
      abrechnungsintervall_monate: 12,
      haushaltskunde: true,
      netznutzungsvertrag: "LIEFERANT_NB",
      netznutzung_zahler: "LIEFERANT",
      gemeinderabatt_prozent: "0",
      kunde: KUNDE,
      ablesekarte: KUNDE,
    },
  },
  {
    command: "geli.lieferende.anmelden",
    pid: "44004",
    beispiel: { malo_id: MALO, process_date: "20261231", transaktionsgrund: "ZT5" },
  },
  {
    command: "geli.kuendigung.anmelden",
    pid: "44016",
    beispiel: { malo_id: MALO, process_date: "20261231", lfa_mp_id: "9805550000010" },
  },
];

/** The facts the person typed, as a payload; `null` when they are not a JSON object. */
export function lesen(text: string): Record<string, unknown> | null {
  try {
    const v: unknown = JSON.parse(text);
    return v && typeof v === "object" && !Array.isArray(v) ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

/** A payload as the facts field shows it. */
export const zeigen = (v: unknown) => JSON.stringify(v, null, 2);
