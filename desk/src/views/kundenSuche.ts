// The Lieferant's Anker search, shared by the Kunden page and ⌘K: what a
// person types goes to vertragd's `search_kunden` after a short pause — the
// Desk keeps no index of customers.

import { useQuery } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useRolle } from "../app/rollen";
import { call } from "../data/mcp";

/** `value`, once it has stopped changing for `ms`. */
export function useSettled<T>(value: T, ms = 200): T {
  const [settled, setSettled] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setSettled(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return settled;
}

/** The customers matching `typed`, from three characters on. */
export function useKundenSuche(typed: string) {
  const { rolle } = useRolle();
  const query = useSettled(typed.trim());
  return useQuery({
    queryKey: [rolle, "vertragd", "search_kunden", query],
    queryFn: () => call("vertragd", "search_kunden", { query }),
    enabled: rolle === "lieferant" && query.length >= 3,
    placeholderData: (previous) => previous,
  });
}

/** Up to two initials for the Anker card's monogram. */
export function monogram(name: string | null | undefined): string {
  const words = (name ?? "").split(/\s+/).filter(Boolean);
  const letters = words.length > 1 ? [words[0], words[words.length - 1]] : words.slice(0, 1);
  return letters.map((w) => w?.[0]?.toUpperCase() ?? "").join("") || "?";
}
