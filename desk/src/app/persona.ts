// The demo sign-in's persona: who the Desk works as where the runtime
// configuration offers personas (`just demo`, `just dev`). The choice is a
// cookie the edge reads — Vite's proxy in development, nginx in the demo —
// to forward that persona's token; the Desk itself never holds one. Switching
// clears every cached read, so no answer given to one persona renders under
// another.

import { useQueryClient } from "@tanstack/react-query";
import { useCallback, useSyncExternalStore } from "react";
import { type Persona, useLaufzeit } from "./laufzeit";

export const COOKIE = "mako_persona";

/** The persona id the cookie names, if any. */
export function cookiePersona(cookie: string): string | null {
  for (const teil of cookie.split(";")) {
    const [k, v] = teil.trim().split("=");
    if (k === COOKIE && v) return decodeURIComponent(v);
  }
  return null;
}

// One choice for the whole Desk: the account menu that sets it and the
// identity that names it read the same value.
const hoerer = new Set<() => void>();
const lesen = (): string | null =>
  typeof document === "undefined" ? null : cookiePersona(document.cookie);
const abonnieren = (f: () => void) => {
  hoerer.add(f);
  return () => hoerer.delete(f);
};

export interface PersonaState {
  /** Every persona on offer; empty outside the demo. */
  personas: Persona[];
  /** The persona the edge answers as; null outside the demo. */
  aktiv: Persona | null;
  waehlen: (id: string) => void;
}

export function usePersona(): PersonaState {
  const { demo } = useLaufzeit();
  const queries = useQueryClient();
  const gewaehlt = useSyncExternalStore(abonnieren, lesen, () => null);
  const personas = demo?.personas ?? [];
  const aktiv =
    personas.find((p) => p.id === gewaehlt) ??
    personas.find((p) => p.id === demo?.standard) ??
    null;
  const waehlen = useCallback(
    (id: string) => {
      if (!personas.some((p) => p.id === id) || id === aktiv?.id) return;
      // biome-ignore lint/suspicious/noDocumentCookie: the edge reads this cookie; there is no other store it can see
      document.cookie = `${COOKIE}=${encodeURIComponent(id)}; path=/; SameSite=Strict`;
      queries.clear();
      for (const f of hoerer) f();
    },
    [personas, aktiv, queries],
  );
  return { personas, aktiv, waehlen };
}
