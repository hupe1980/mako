// The Desk's Sprache: German, with English selectable.
//
// Every string a view shows comes from a catalogue: `useT()` in a component,
// `katalog()` in a plain function (the `format/` layer, a label table). The
// choice is the viewer's, kept in the browser like the Erscheinungsbild, and
// `<html lang>` follows it, so a screen reader speaks each language as itself.
// A Frist stays a Berlin instant in either language: the Sprache picks the
// number and date style of `src/format/`, never the time zone.

import { useSyncExternalStore } from "react";
import { de, type Katalog } from "./de";

export type { Katalog } from "./de";

export type Sprache = "de" | "en";

/** Each Sprache under its own name, as a language menu lists them. */
export const SPRACHEN: { id: Sprache; name: string }[] = [
  { id: "de", name: "Deutsch" },
  { id: "en", name: "English" },
];

// German is the Desk's first language and ships with the first paint;
// English loads when it is chosen (`ladeSprache`), so the first paint of
// the German Desk does not carry a second catalogue.
const KATALOGE: Partial<Record<Sprache, Katalog>> = { de };

/** Load `s`'s catalogue if it is not yet; resolves once `katalog()` can answer in it. */
export async function ladeSprache(s: Sprache): Promise<void> {
  if (KATALOGE[s]) return;
  if (s === "en") KATALOGE.en = (await import("./en")).en;
}

const STORE = "mako-desk.sprache";

function remembered(): Sprache {
  try {
    if (localStorage.getItem(STORE) === "en") return "en";
  } catch {
    // storage blocked: German
  }
  return "de";
}

let aktuell: Sprache = "de";
let ersatz: Katalog | null = null;
/** Moves on every change, so `useT` renders again for a catalogue swap too. */
let stand = 0;
const gestand = () => stand;
const hoerer = new Set<() => void>();

function anwenden(s: Sprache) {
  if (typeof document !== "undefined") document.documentElement.lang = s;
}

/** The Sprache in force — for code that formats outside a component. */
export function sprache(): Sprache {
  return aktuell;
}

/** The catalogue in force — for code that names things outside a component. */
export function katalog(): Katalog {
  return ersatz ?? KATALOGE[aktuell] ?? de;
}

/** Choose the Sprache: every component reading `useT()` renders again in it, once its catalogue is loaded. */
export function setSprache(s: Sprache) {
  if (!KATALOGE[s]) {
    void ladeSprache(s).then(() => setSprache(s));
    return;
  }
  aktuell = s;
  anwenden(s);
  try {
    if (s === "de") localStorage.removeItem(STORE);
    else localStorage.setItem(STORE, s);
  } catch {
    // storage blocked: the choice lasts for this tab
  }
  stand += 1;
  for (const h of hoerer) h();
}

/**
 * Applied before the first render, so `<html lang>` and the catalogue are
 * right from the start: an English Desk waits for its catalogue, a German one
 * does not wait at all.
 */
export async function applyRememberedSprache(): Promise<void> {
  const s = remembered();
  await ladeSprache(s);
  aktuell = s;
  anwenden(aktuell);
}

function abonnieren(h: () => void) {
  hoerer.add(h);
  return () => hoerer.delete(h);
}

/** The Sprache and its setter, for the account menu. */
export function useSprache(): [Sprache, (s: Sprache) => void] {
  return [useSyncExternalStore(abonnieren, sprache, sprache), setSprache];
}

/** The catalogue in force; the component renders again when the Sprache changes. */
export function useT(): Katalog {
  useSyncExternalStore(abonnieren, gestand, gestand);
  return katalog();
}

/**
 * Tests only: render with `k` in place of the chosen catalogue — a pseudo
 * catalogue marks every string, so a test can find the ones that bypass it.
 * `null` restores the chosen one.
 */
export function katalogErsetzen(k: Katalog | null) {
  ersatz = k;
  stand += 1;
  for (const h of hoerer) h();
}

/** Render a service code in the current language, including unfamiliar values. */
export function eintrag(tabelle: Record<string, string>, key: string, fallback?: string): string {
  const value = Object.hasOwn(tabelle, key) ? tabelle[key] : undefined;
  return value ?? fallback ?? katalog().values.unbekannt;
}
