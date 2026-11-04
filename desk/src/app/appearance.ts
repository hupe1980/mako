// Erscheinungsbild: System, Hell or Dunkel — a per-browser convenience. The
// tokens are `light-dark()` pairs, so `data-theme` on the root is all it sets.

import { useCallback, useState } from "react";

export type Appearance = "system" | "light" | "dark";

const STORE = "mako-desk.appearance";

function remembered(): Appearance {
  try {
    const a = localStorage.getItem(STORE);
    if (a === "light" || a === "dark") return a;
  } catch {
    // storage blocked: follow the system
  }
  return "system";
}

function apply(a: Appearance) {
  if (a === "system") delete document.documentElement.dataset.theme;
  else document.documentElement.dataset.theme = a;
}

/** Applied before the first render, so a chosen appearance never flashes. */
export function applyRememberedAppearance() {
  apply(remembered());
}

export function useAppearance(): [Appearance, (a: Appearance) => void] {
  const [appearance, setState] = useState(remembered);
  const set = useCallback((a: Appearance) => {
    apply(a);
    setState(a);
    try {
      if (a === "system") localStorage.removeItem(STORE);
      else localStorage.setItem(STORE, a);
    } catch {
      // storage blocked: the choice lasts for this tab
    }
  }, []);
  return [appearance, set];
}
