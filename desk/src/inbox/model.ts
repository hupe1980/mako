// The inbox's one shape. Every source maps its service's typed answer into
// `Eintrag`s; nothing here computes a Frist — `frist` is the instant the
// service returned, or none (FR-006, FR-007, FR-010).

import type { Anker } from "../data/agent";
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
  /** A Freigabe, a Disput and a Prüfposten are decided in place, from Heute. */
  freigabe?: Freigabe;
  disput?: Disput;
  pruefposten?: Pruefposten;
}

export type QuelleId =
  | "freigaben"
  | "ueberfaellig"
  | "anmeldungen"
  | "nicht_uebermittelt"
  | "korrekturbedarf"
  | "ableseauftraege"
  | "dispute"
  | "pruefposten"
  | "hinweise";

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
