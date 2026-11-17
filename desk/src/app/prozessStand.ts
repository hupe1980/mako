// A process's steps and its Nachweise, read off what the services state: the
// messages makod's history records, the deadlines makod still holds pending,
// and the state and APERAK error code processd's read model carries. Nothing
// here is read off an event's name, and nothing decides whether a step was
// right — a step is done when a service recorded it, open while a deadline
// for it is pending.

import type { DeadlineView, HistoryEntry } from "../data/gen/makod";
import type { Instant } from "../format/brands";
import { epochMs } from "../format/date";
import { eintrag, katalog } from "../i18n";
import { fristArt, zustand } from "./prozesse";

export type SchrittStand = "erledigt" | "jetzt" | "offen";

export interface Schritt {
  id: string;
  text: string;
  stand: SchrittStand;
}

/** processd's states that end a process. */
const ENDE = new Set(["completed", "rejected", "failed"]);

/** A pending deadline with its instant, the earliest first. */
export function offeneFristen(deadlines: DeadlineView[]): (DeadlineView & { due_at: Instant })[] {
  return deadlines
    .flatMap((d) => (d.due_at ? [{ ...d, due_at: d.due_at }] : []))
    .sort((a, b) => epochMs(a.due_at) - epochMs(b.due_at));
}

/** A market message's name on the page: its direction and PID, as the workflow stated them. */
export function nachricht(e: HistoryEntry): string {
  const k = katalog().verlauf;
  const richtung = e.richtung === "eingang" ? k.eingang : k.ausgang;
  return e.pid ? `${richtung} · PID ${e.pid}` : richtung;
}

/**
 * The step bar: every message recorded, then every deadline pending (what is
 * still awaited), then the end — processd's terminal state once it has one.
 * `state` is null when the page shows an earlier Stand, whose state no
 * service recorded.
 */
export function schritte(
  eintraege: HistoryEntry[],
  pending: DeadlineView[],
  state: string | null,
): Schritt[] {
  const k = katalog().prozessAnsicht;
  const out: Schritt[] = [];
  for (const e of eintraege)
    if (e.richtung) out.push({ id: `m${e.sequence}`, text: nachricht(e), stand: "erledigt" });
  for (const d of offeneFristen(pending))
    out.push({ id: d.deadline_id, text: eintrag(k.schrittArt, d.art), stand: "offen" });
  const ende = state !== null && ENDE.has(state);
  out.push({
    id: "ende",
    text: ende && state ? zustand(state) : k.abgeschlossen,
    stand: ende ? "erledigt" : "offen",
  });
  const jetzt = out.find((s) => s.stand === "offen");
  if (jetzt) jetzt.stand = "jetzt";
  return out;
}

export interface Nachweis {
  id: string;
  text: string;
  /** A service recorded it: the message is there. */
  belegt: boolean;
  /** The instant it was recorded, or the deadline it is awaited by. */
  zeit: Instant | null;
  frist: Instant | null;
  ref: string | null;
  /** processd recorded a refusal for it. */
  abgelehnt: boolean;
}

/**
 * The Nachweise as a chain: each message the history records, ticked; an
 * APERAK processd recorded with an error code, not ticked; every message a
 * pending deadline still waits for, not ticked.
 */
export function nachweise(
  eintraege: HistoryEntry[],
  pending: DeadlineView[],
  ercCode: string | null,
): Nachweis[] {
  const k = katalog().prozessAnsicht;
  const out: Nachweis[] = eintraege
    .filter((e) => e.richtung)
    .map((e) => ({
      id: `m${e.sequence}`,
      text: nachricht(e),
      belegt: true,
      zeit: e.recorded_at,
      frist: null,
      ref: e.message_ref,
      abgelehnt: false,
    }));
  if (ercCode)
    out.push({
      id: "erc",
      text: k.aperakFehler(ercCode),
      belegt: false,
      zeit: null,
      frist: null,
      ref: null,
      abgelehnt: true,
    });
  for (const d of offeneFristen(pending))
    out.push({
      id: d.deadline_id,
      text: fristArt(d.art),
      belegt: false,
      zeit: null,
      frist: d.due_at,
      ref: null,
      abgelehnt: false,
    });
  return out;
}
