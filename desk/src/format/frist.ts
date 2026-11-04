// The Frist-Uhr's text. The deadline instant is the service's; this only says
// how far away it is on the wall clock. Werktage are never counted here — a
// Werktag count is shown only when a service returns one (FR-010).

import { useEffect, useState } from "react";
import type { Instant } from "./brands";
import { epochMs } from "./date";

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
  if (ms < HOUR) return `${Math.max(1, Math.round(ms / MINUTE))} min`;
  if (ms < DAY) {
    const h = Math.floor(ms / HOUR);
    const m = Math.round((ms % HOUR) / MINUTE);
    return m ? `${h} h ${m} min` : `${h} h`;
  }
  const d = Math.round(ms / DAY);
  return d === 1 ? "1 Tag" : `${d} Tagen`;
}

/** How far `due` is from `now`, and how urgent that is. */
export function distance(due: Instant, now: number): Distance {
  const left = epochMs(due) - now;
  if (left <= 0) return { urgency: "lapsed", text: `abgelaufen vor ${span(-left)}` };
  const urgency: Urgency = left < 2 * HOUR ? "critical" : left < DAY ? "soon" : "calm";
  return { urgency, text: `in ${span(left)}` };
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
