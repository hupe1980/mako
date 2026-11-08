// The evidence an answer points at. A sentence's source number names a read
// — `dienst/werkzeug/ref` — and every row on screen that shows what that read
// returned registers under the same key; focusing or hovering the number
// marks each such row wherever it is. One mark at a time, for the whole tab.

import { useSyncExternalStore } from "react";
import type { SchrittQuelle } from "../data/agent";

let markiert: string | null = null;
const listeners = new Set<() => void>();

/** The key a read and the rows showing its result share. */
export function belegSchluessel(q: SchrittQuelle): string {
  return q.ref ? `${q.dienst}/${q.werkzeug}/${q.ref}` : `${q.dienst}/${q.werkzeug}`;
}

/** Mark the rows of `key`; null clears the mark. */
export function markieren(key: string | null): void {
  if (markiert === key) return;
  markiert = key;
  for (const l of listeners) l();
}

/** Clear the mark only if it is still `key` — a blur after the next focus leaves that one. */
export function entmarkieren(key: string): void {
  if (markiert === key) markieren(null);
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/**
 * Props for a row that shows a read's result: `data-markiert` while a
 * citation of that read has focus or the pointer, styled by `.beleg-ziel`.
 */
export function useBelegZiel(key: string | null): {
  "data-beleg": string | undefined;
  "data-markiert": true | undefined;
} {
  const an = useSyncExternalStore(subscribe, () => key !== null && markiert === key);
  return { "data-beleg": key ?? undefined, "data-markiert": an || undefined };
}
