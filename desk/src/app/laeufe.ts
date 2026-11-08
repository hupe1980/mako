// The runs this tab follows. A run's stream belongs to the tab, not to the
// view that asked: leaving Fragen mid-answer keeps it streaming, and the top
// bar shows it as a pill until its answer has been read on its own view. Each
// run is what agentd's stream said of it — its steps, its answer or its stop —
// and nothing the Desk worked out.

import { useEffect, useSyncExternalStore } from "react";
import {
  type Abbruch,
  type Answer,
  type Schritt,
  schritte,
  stoppen as stoppenBeiAgentd,
} from "../data/agent";
import type { Rolle } from "./rollen";

export interface Lauf {
  run: string;
  frage: string;
  /** The Anker the question is about, as a key: a view finds its run by it. */
  anker: string;
  rolle: Rolle;
  /** Where the run was asked, so its pill returns there. */
  zurueck: string;
  /**
   * Whether leaving its view leaves a pill. A „Warum?" is a row's own
   * answer, reopened from the row, so it has none.
   */
  pille: boolean;
  /** Larger is newer. */
  nr: number;
  schritte: Schritt[];
  antwort: Answer | null;
  abgebrochen: Abbruch | null;
  /** The stream cannot be read: the view reads the answer instead. */
  unlesbar: boolean;
  /** A Stop is on its way to agentd. */
  stoppt: boolean;
  /** agentd refused the Stop, in its words. */
  stoppFehler: string | null;
  /** Its conclusion has been on its view. */
  gelesen: boolean;
  /** Views showing it now. */
  zuschauer: number;
}

/** The run has nothing more to stream. */
export const beendet = (l: Lauf) => Boolean(l.antwort || l.abgebrochen);

let laeufe = new Map<string, Lauf>();
const schliessen = new Map<string, () => void>();
const listeners = new Set<() => void>();
let naechste = 0;

function aendern(run: string, f: (l: Lauf) => Partial<Lauf>) {
  const alt = laeufe.get(run);
  if (!alt) return;
  const neu = { ...alt, ...f(alt) };
  // A conclusion with its view on screen is read the moment it arrives.
  if (neu.zuschauer > 0 && beendet(neu)) neu.gelesen = true;
  laeufe = new Map(laeufe).set(run, neu);
  for (const l of listeners) l();
}

/**
 * Follow `run`: open its stream once, whichever view asks first. A run
 * already followed is left as it is.
 */
export function folgen(
  run: string,
  meta: { frage: string; anker: string; rolle: Rolle; zurueck: string; pille: boolean },
): void {
  if (laeufe.has(run)) return;
  naechste += 1;
  laeufe = new Map(laeufe).set(run, {
    run,
    ...meta,
    nr: naechste,
    schritte: [],
    antwort: null,
    abgebrochen: null,
    unlesbar: false,
    stoppt: false,
    stoppFehler: null,
    gelesen: false,
    zuschauer: 0,
  });
  for (const l of listeners) l();
  schliessen.set(
    run,
    schritte(run, {
      schritt: (s) => aendern(run, (l) => ({ schritte: [...l.schritte, s] })),
      antwort: (a) => aendern(run, () => ({ antwort: a, stoppt: false })),
      abgebrochen: (a) => aendern(run, () => ({ abgebrochen: a, stoppt: false })),
      unlesbar: () => aendern(run, () => ({ unlesbar: true })),
    }),
  );
}

/**
 * Stop a live run, once: a second Stop while the first is on its way, or on a
 * run that has ended, sends nothing.
 */
export function stoppen(run: string): void {
  const l = laeufe.get(run);
  if (!l || l.stoppt || beendet(l)) return;
  aendern(run, () => ({ stoppt: true, stoppFehler: null }));
  stoppenBeiAgentd(run).catch((e: unknown) =>
    aendern(run, () => ({
      stoppt: false,
      stoppFehler: e instanceof Error ? e.message : String(e),
    })),
  );
}

/** Forget every run and close their streams — a test's clean slate. */
export function laeufeVergessen(): void {
  for (const close of schliessen.values()) close();
  schliessen.clear();
  laeufe = new Map();
  for (const l of listeners) l();
}

const subscribe = (l: () => void) => {
  listeners.add(l);
  return () => listeners.delete(l);
};

/** Every run followed, newest first. */
export function useLaeufe(): Lauf[] {
  const all = useSyncExternalStore(subscribe, () => laeufe);
  return [...all.values()].sort((a, b) => b.nr - a.nr);
}

/** One run; undefined before it is followed. */
export function useLauf(run: string | undefined): Lauf | undefined {
  return useSyncExternalStore(subscribe, () => (run ? laeufe.get(run) : undefined));
}

/**
 * The run a view left behind for `anker` and has not read yet — the one its
 * pill returns to, picked up again when the view mounts.
 */
export function offenerLauf(anker: string, rolle: Rolle): Lauf | undefined {
  let best: Lauf | undefined;
  for (const l of laeufe.values()) {
    if (l.anker === anker && l.rolle === rolle && !l.gelesen && !l.unlesbar) {
      if (!best || l.nr > best.nr) best = l;
    }
  }
  return best;
}

/** The view showing `run` is on screen while this is mounted. */
export function useZuschauen(run: string | undefined): void {
  useEffect(() => {
    if (!run) return;
    aendern(run, (l) => ({ zuschauer: l.zuschauer + 1 }));
    return () => aendern(run, (l) => ({ zuschauer: Math.max(0, l.zuschauer - 1) }));
  }, [run]);
}
