// One Desk, worn one Rolle at a time. The Rolle decides the accent,
// the sidebar and its Heute sources; switching clears every
// cached read, so no row of one Rolle can render under another.

import { useQueryClient } from "@tanstack/react-query";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useState } from "react";
import { flushSync } from "react-dom";
import { ROLLE } from "./keys";

export type Rolle = "netz" | "lieferant" | "messstelle" | "energieservice";

/** Each Rolle starts from its Anker; `home` is where the Desk opens for it. */
export const ROLLEN: Record<
  Rolle,
  { label: string; marktrolle: string; key: string; home: "/heute" | "/kunden" | "/prozess" }
> = {
  netz: { label: "Netz", marktrolle: "NB", key: "1", home: "/heute" },
  lieferant: { label: "Lieferant", marktrolle: "LF", key: "2", home: "/heute" },
  messstelle: { label: "Messstelle", marktrolle: "MSB", key: "3", home: "/heute" },
  energieservice: { label: "Energieservice", marktrolle: "ESA", key: "4", home: "/heute" },
};

const ORDER: Rolle[] = ["netz", "lieferant", "messstelle", "energieservice"];

/**
 * The Rollen this person may wear. Every Rolle is offered until the gateway's
 * identity answer narrows it (spec Open Question 1); what each Rolle shows is
 * the service's Cedar answer, not a Desk filter.
 */
export function heldRollen(): Rolle[] {
  return ORDER;
}

const STORE = "mako-desk.rolle";

function remembered(held: Rolle[]): Rolle {
  try {
    const r = localStorage.getItem(STORE) as Rolle | null;
    if (r && held.includes(r)) return r;
  } catch {
    // storage blocked: fall back to the first Rolle
  }
  return held[0] ?? "netz";
}

interface RolleState {
  rolle: Rolle;
  held: Rolle[];
  wear: (r: Rolle) => void;
}

const Ctx = createContext<RolleState | null>(null);

export function RolleProvider({
  children,
  held = heldRollen(),
}: {
  children: ReactNode;
  held?: Rolle[];
}) {
  const queries = useQueryClient();
  const [rolle, setRolle] = useState<Rolle>(() => remembered(held));

  useEffect(() => {
    document.documentElement.dataset.rolle = rolle;
  }, [rolle]);

  const wear = useCallback(
    (next: Rolle) => {
      if (next === rolle || !held.includes(next)) return;
      const apply = () => {
        queries.clear();
        flushSync(() => setRolle(next));
      };
      if (document.startViewTransition) document.startViewTransition(apply);
      else apply();
      try {
        localStorage.setItem(STORE, next);
      } catch {
        // storage blocked: the choice lasts for this tab
      }
    },
    [rolle, held, queries],
  );

  useEffect(() => {
    if (held.length < 2) return;
    const onKey = (e: KeyboardEvent) => {
      if (!ROLLE.match?.(e)) return;
      const match = held.find((r) => ROLLEN[r].key === e.key);
      if (match) {
        e.preventDefault();
        wear(match);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [held, wear]);

  return <Ctx.Provider value={{ rolle, held, wear }}>{children}</Ctx.Provider>;
}

export function useRolle(): RolleState {
  const state = useContext(Ctx);
  if (!state) throw new Error("useRolle outside RolleProvider");
  return state;
}
