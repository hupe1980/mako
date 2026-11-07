// Who may do what is each service's Cedar policy's answer, never the Desk's
// (FR-022): no role is mapped to an action in desk/src. The Desk shows every
// action, and when a service answers one with a 403 it shows the service's
// reason where the action was and offers that action of that service no more
// this session — the tab's sessionStorage, so a reload keeps it and a new
// sign-in in a new tab asks again.

import { useSyncExternalStore } from "react";

/** An action the Desk offers, by the service that decides it. */
export type Handlung =
  | { dienst: "processd"; aktion: "entscheiden" }
  | { dienst: "invoicd"; aktion: "entscheiden" }
  | { dienst: "billingd"; aktion: "freigeben" | "pruefen" }
  // Neu buchen and Verwerfen are one Cedar action (`post-entry`): a refusal
  // of either is a refusal of both.
  | { dienst: "accountingd"; aktion: "buchen" }
  | { dienst: "agentd"; aktion: "fragen" | "bewerten" };

/** The service refused the person this action: its reason, as it wrote it. */
export class Verweigert extends Error {
  override name = "Verweigert";
  readonly handlung: Handlung;
  constructor(handlung: Handlung, reason: string) {
    super(reason);
    this.handlung = handlung;
  }
}

const STORE = "mako-desk.verweigert";
const key = (h: Handlung) => `${h.dienst}/${h.aktion}`;

function read(): Record<string, string> {
  try {
    return JSON.parse(sessionStorage.getItem(STORE) ?? "{}") as Record<string, string>;
  } catch {
    return {};
  }
}

let refused: Record<string, string> = read();
const listeners = new Set<() => void>();

/** Record a 403 and return the error to throw. */
export function verweigern(h: Handlung, reason: string): Verweigert {
  refused = { ...refused, [key(h)]: reason };
  try {
    sessionStorage.setItem(STORE, JSON.stringify(refused));
  } catch {
    // Storage blocked: the refusal holds for this page only.
  }
  for (const l of listeners) l();
  return new Verweigert(h, reason);
}

/** The service's reason when it refused this action this session, else null. */
export function useVerweigert(h: Handlung): string | null {
  const k = key(h);
  return useSyncExternalStore(
    (l) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    () => refused[k] ?? null,
  );
}

/** Forget every refusal — for tests, which share one module. */
export function vergessen(): void {
  refused = {};
  try {
    sessionStorage.removeItem(STORE);
  } catch {
    // nothing stored
  }
  for (const l of listeners) l();
}
