// Every key the Desk answers to, in one table: the handlers match through it
// and the ⌘/ sheet prints it, so a key cannot work without being listed or be
// listed without working. Label and group are read from the catalogue each
// time, so the sheet speaks the Sprache in force.

import { type Katalog, katalog } from "../i18n";

export interface Shortcut {
  /** As printed on the keyboard. */
  keys: string;
  readonly label: string;
  readonly group: string;
  /** For a key the Desk itself listens for; list keys belong to React Aria. */
  match?: (e: KeyboardEvent) => boolean;
}

type Taste = Exclude<keyof Katalog["tasten"], "gruppe">;
type Gruppe = keyof Katalog["tasten"]["gruppe"];

/** A row of the table: its words from the catalogue, its keys and matcher as given. */
function taste(
  label: Taste,
  group: Gruppe,
  rest: { keys: string; match?: (e: KeyboardEvent) => boolean },
): Shortcut {
  return {
    ...rest,
    get label() {
      return katalog().tasten[label];
    },
    get group() {
      return katalog().tasten.gruppe[group];
    },
  };
}

const command = (e: KeyboardEvent) => (e.metaKey || e.ctrlKey) && !e.altKey;

export const PALETTE = taste("palette", "ueberall", {
  keys: "⌘K",
  match: (e) => command(e) && e.key.toLowerCase() === "k",
});

export const SHEET = taste("sheet", "ueberall", {
  keys: "⌘/",
  match: (e) => command(e) && e.key === "/",
});

export const FRAGEN = taste("fragen", "ueberall", {
  keys: "⌘J",
  match: (e) => command(e) && e.key.toLowerCase() === "j",
});

const feld = (t: Element | null) =>
  !!t &&
  (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || (t as HTMLElement).isContentEditable);

/**
 * The key went to a field the person types into — or to the list of one: a
 * search that keeps the focus in its field (React Aria's Autocomplete, ⌘K's
 * search) re-dispatches each key to its list, so the focus says whose it is.
 */
const typing = (e: KeyboardEvent) =>
  feld(e.target as Element | null) ||
  (typeof document !== "undefined" && feld(document.activeElement));

/** A bare letter, and not while the person types into a field. */
const letter = (k: string) => (e: KeyboardEvent) =>
  !typing(e) && !e.metaKey && !e.ctrlKey && !e.altKey && e.key.toLowerCase() === k;

export const NAECHSTE = taste("naechste", "listen", {
  keys: "J",
  match: letter("j"),
});
export const VORIGE = taste("vorige", "listen", {
  keys: "K",
  match: letter("k"),
});
export const FREIGEBEN = taste("freigeben", "entscheiden", { keys: "A", match: letter("a") });
export const ABLEHNEN = taste("ablehnen", "entscheiden", { keys: "R", match: letter("r") });

export const UEBERNEHMEN = taste("uebernehmen", "entscheiden", {
  keys: "U",
  match: letter("u"),
});

/**
 * `?` — Shift plus the key `/` sits on, whatever the layout: `e.key` is the
 * character typed. Free of ⌘/, which opens this sheet.
 */
export const WARUM = taste("warum", "listen", {
  keys: "?",
  match: (e) => !typing(e) && !e.metaKey && !e.ctrlKey && !e.altKey && e.key === "?",
});

/**
 * Takes back the newest held decision. Not while the person types into a
 * field, where ⌘Z is the field's own undo; with nothing held it is left alone.
 */
export const UNDO = taste("undo", "ueberall", {
  keys: "⌘Z",
  match: (e) => !typing(e) && command(e) && !e.shiftKey && e.key.toLowerCase() === "z",
});

export const ROLLE = taste("rolle", "ueberall", {
  keys: "⌃1 – ⌃4",
  match: (e) => e.ctrlKey && !e.metaKey && !e.altKey && /^[1-4]$/.test(e.key),
});

/**
 * Esc closes what is open — a dialog, a popover, the palette — and, with
 * nothing open, stops the assistant's live run: the key that already means
 * „back". The Desk listens only for the second; the first is React Aria's.
 */
export const STOPP = taste("stopp", "ueberall", {
  keys: "esc",
  match: (e) =>
    e.key === "Escape" &&
    !e.defaultPrevented &&
    !e.metaKey &&
    !e.ctrlKey &&
    !e.altKey &&
    !document.querySelector('[role="dialog"], [role="alertdialog"]'),
});

export const SHORTCUTS: Shortcut[] = [
  PALETTE,
  FRAGEN,
  ROLLE,
  UNDO,
  SHEET,
  STOPP,
  taste("zeileWaehlen", "listen", { keys: "↑ ↓" }),
  taste("zeileOeffnen", "listen", { keys: "↵" }),
  NAECHSTE,
  VORIGE,
  WARUM,
  FREIGEBEN,
  ABLEHNEN,
  UEBERNEHMEN,
  taste("senden", "entscheiden", { keys: "↵" }),
  taste("zweiteAktion", "entscheiden", { keys: "⌘↵" }),
];
