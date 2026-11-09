// The Frist-Uhr's text. The deadline instant is the service's; this only says
// how far away it is on the wall clock. Werktage are never counted here — a
// Werktag count is shown only when a service returns one.

import { useEffect, useState } from "react";
import { katalog } from "../i18n";
import type { Instant } from "./brands";
import { berlinDayNumber, epochMs } from "./date";

export type Urgency = "lapsed" | "critical" | "soon" | "calm";

export interface Distance {
  urgency: Urgency;
  /** `in 5 h 12 min`, `in 3 Tagen`, `abgelaufen vor 2 h`. */
  text: string;
}

const MINUTE = 60_000;
const HOUR = 60 * MINUTE;
const DAY = 24 * HOUR;

function span(ms: number): string {
  const z = katalog().zeit;
  if (ms < HOUR) return z.minuten(String(Math.max(1, Math.round(ms / MINUTE))));
  if (ms < DAY) {
    const h = Math.floor(ms / HOUR);
    const m = Math.round((ms % HOUR) / MINUTE);
    return m ? z.stundenMinuten(String(h), String(m)) : z.stunden(String(h));
  }
  const d = Math.round(ms / DAY);
  return z.tage(d, String(d));
}

/** How far `due` is from `now`, and how urgent that is. */
export function distance(due: Instant, now: number): Distance {
  const left = epochMs(due) - now;
  const z = katalog().zeit;
  if (left <= 0) return { urgency: "lapsed", text: z.abgelaufenVor(span(-left)) };
  const urgency: Urgency = left < 2 * HOUR ? "critical" : left < DAY ? "soon" : "calm";
  return { urgency, text: z.in(span(left)) };
}

/** `seit 3 Tagen`, `seit 2 h` — how long ago `since` was, on the wall clock. */
export function seit(since: Instant, now: number): string {
  return katalog().zeit.seit(span(Math.max(0, now - epochMs(since))));
}

/** The current time, re-read every `everyMs` while mounted. */
export function useNow(everyMs = 30_000): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const id = setInterval(() => setNow(Date.now()), everyMs);
    return () => clearInterval(id);
  }, [everyMs]);
  return now;
}

/** Where an inbox item sits by its Frist: the bands Heute is read in. */
export type Band = "ueberfaellig" | "heute" | "woche" | "spaeter" | "ohne";

export const BAENDER: { band: Band; readonly titel: string }[] = (
  ["ueberfaellig", "heute", "woche", "spaeter", "ohne"] as const
).map((band) => ({
  band,
  // Read from the catalogue each time a band is shown.
  get titel() {
    return katalog().baender[band];
  },
}));

/**
 * The band of a Frist the service returned, against `now`: lapsed, due on
 * today's Berlin calendar day, within the next seven, or later. The edges are
 * Berlin days, because a Frist is a German one; the instant is the service's.
 */
export function band(due: Instant | null | undefined, now: number): Band {
  if (!due) return "ohne";
  const at = epochMs(due);
  if (at <= now) return "ueberfaellig";
  const tage = berlinDayNumber(at) - berlinDayNumber(now);
  return tage <= 0 ? "heute" : tage <= 7 ? "woche" : "spaeter";
}
