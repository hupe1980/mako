// The inbox's one shape. Every source maps its service's typed answer into
// `Eintrag`s; nothing here computes a Frist — `frist` is the instant the
// service returned, or none (FR-006, FR-007, FR-010).

import type { Anker } from "../data/agent";
import type { GeparkteBuchung } from "../data/gen/accountingd";
import type { Pruefposten } from "../data/gen/billingd";
import type { Disput } from "../data/gen/invoicd";
import type { Freigabe } from "../data/gen/processd";
import type { Instant } from "../format/brands";

/** Where opening an item goes. */
export type Ziel =
  | { to: "/freigaben"; search: { id: string } }
  | { to: "/prozess"; search: { key: string } }
  | { to: "/kunde/$id"; params: { id: string } };

export interface Eintrag {
  /** Unique across sources: one process reached by two sources is one item. */
  id: string;
  quelle: QuelleId;
  /** The subject: a customer's name, else the identifier. */
  titel: string;
  /** Whether `titel` is an identifier, set in mono. */
  kennung: boolean;
  /** The second line: identifiers, PID — what tells two items apart. */
  detail: string;
  /** Why it waits, in the service's words. */
  grund: string;
  frist: Instant | null;
  /** The MaLo or process the agent's Hinweise are filed under. */
  bezug: string[];
  /** What the assistant is asked about when this item has focus. */
  anker: Anker | null;
  /** Where ↵ goes; none while the Desk has no view of the record. */
  ziel: Ziel | null;
  /** A Freigabe, a Disput, a Prüfposten and a geparkte Buchung are decided in place, from Heute. */
  freigabe?: Freigabe;
  disput?: Disput;
  pruefposten?: Pruefposten;
  geparkt?: GeparkteBuchung;
}

/** Every source Heute reads, in no particular order. */
export const QUELLE_IDS = [
  "freigaben",
  "ueberfaellig",
  "anmeldungen",
  "nicht_uebermittelt",
  "korrekturbedarf",
  "ableseauftraege",
  "dispute",
  "pruefposten",
  "geparkt",
  "hinweise",
] as const;

export type QuelleId = (typeof QUELLE_IDS)[number];

/**
 * The sources whose items do not offer „Warum?", each with why. „Warum?" asks
 * the desk-assistant why an item waits, and it can only answer from the record
 * that put the item there — so a source whose record it does not read would
 * get an answer about something else, said with the same confidence.
 */
export const OHNE_WARUM: Partial<Record<QuelleId, string>> = {
  korrekturbedarf:
    "Ein Bilanzierungsgebiet ist kein Anker des Assistenten, und die Netzbilanzierung liest er nicht.",
  ableseauftraege:
    "Der Assistent liest keine Ableseaufträge; zur MaLo nennte er Prozesse, nicht den Auftrag.",
  pruefposten:
    "Der Assistent liest die Rechnungsprüfung nicht; zur MaLo nennte er nicht, warum die Rechnung angehalten ist.",
};

/** The Anker „Warum?" asks about for an item, or null where it is not offered. */
export function warumAnker(e: Eintrag): Anker | null {
  return OHNE_WARUM[e.quelle] ? null : e.anker;
}

/** One source's answer this refresh. */
export interface Stand {
  quelle: QuelleId;
  /** `Freigaben`, `Überfällige Prozesse` — what the footer names. */
  label: string;
  eintraege: Eintrag[];
  laedt: boolean;
  /** The service's refusal or failure, in its words; null when it answered. */
  fehler: string | null;
  /** The service returned a full page: there are more than shown. */
  mehr: boolean;
}
