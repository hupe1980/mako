// Every key the Desk answers to, in one table: the handlers match through it
// and the ⌘/ sheet prints it, so a key cannot work without being listed or be
// listed without working.

export interface Shortcut {
  /** As printed on the keyboard. */
  keys: string;
  label: string;
  group: "Überall" | "Listen" | "Entscheiden";
  /** For a key the Desk itself listens for; list keys belong to React Aria. */
  match?: (e: KeyboardEvent) => boolean;
}

const command = (e: KeyboardEvent) => (e.metaKey || e.ctrlKey) && !e.altKey;

export const PALETTE: Shortcut = {
  keys: "⌘K",
  label: "Suchen oder Befehl",
  group: "Überall",
  match: (e) => command(e) && e.key.toLowerCase() === "k",
};

export const SHEET: Shortcut = {
  keys: "⌘/",
  label: "Tastaturkürzel",
  group: "Überall",
  match: (e) => command(e) && e.key === "/",
};

export const FRAGEN: Shortcut = {
  keys: "⌘J",
  label: "Den Assistenten fragen",
  group: "Überall",
  match: (e) => command(e) && e.key.toLowerCase() === "j",
};

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

export const NAECHSTE: Shortcut = {
  keys: "J",
  label: "Nächster Eintrag",
  group: "Listen",
  match: letter("j"),
};
export const VORIGE: Shortcut = {
  keys: "K",
  label: "Voriger Eintrag",
  group: "Listen",
  match: letter("k"),
};
export const FREIGEBEN: Shortcut = {
  keys: "A",
  label:
    "Freigeben wählen — beim Disput Beilegen, beim Prüfposten Geprüft, bei einer geparkten Buchung Neu buchen",
  group: "Entscheiden",
  match: letter("a"),
};
export const ABLEHNEN: Shortcut = {
  keys: "R",
  label: "Ablehnen wählen — beim Disput Ablehnung senden, bei einer geparkten Buchung Verwerfen",
  group: "Entscheiden",
  match: letter("r"),
};

export const UEBERNEHMEN: Shortcut = {
  keys: "U",
  label: "Vorschlag des Assistenten übernehmen",
  group: "Entscheiden",
  match: letter("u"),
};

/**
 * `?` — Shift plus the key `/` sits on, whatever the layout: `e.key` is the
 * character typed. Free of ⌘/, which opens this sheet.
 */
export const WARUM: Shortcut = {
  keys: "?",
  label: "Warum steht der Eintrag hier? Den Assistenten fragen",
  group: "Listen",
  match: (e) => !typing(e) && !e.metaKey && !e.ctrlKey && !e.altKey && e.key === "?",
};

/**
 * Takes back the newest held decision. Not while the person types into a
 * field, where ⌘Z is the field's own undo; with nothing held it is left alone.
 */
export const UNDO: Shortcut = {
  keys: "⌘Z",
  label: "Letzte Entscheidung zurücknehmen",
  group: "Überall",
  match: (e) => !typing(e) && command(e) && !e.shiftKey && e.key.toLowerCase() === "z",
};

export const ROLLE: Shortcut = {
  keys: "⌃1 – ⌃4",
  label: "Rolle wechseln",
  group: "Überall",
  match: (e) => e.ctrlKey && !e.metaKey && !e.altKey && /^[1-4]$/.test(e.key),
};

/**
 * Esc closes what is open — a dialog, a popover, the palette — and, with
 * nothing open, stops the assistant's live run: the key that already means
 * „back". The Desk listens only for the second; the first is React Aria's.
 */
export const STOPP: Shortcut = {
  keys: "esc",
  label: "Schließen — oder die laufende Antwort des Assistenten stoppen",
  group: "Überall",
  match: (e) =>
    e.key === "Escape" &&
    !e.defaultPrevented &&
    !e.metaKey &&
    !e.ctrlKey &&
    !e.altKey &&
    !document.querySelector('[role="dialog"], [role="alertdialog"]'),
};

export const SHORTCUTS: Shortcut[] = [
  PALETTE,
  FRAGEN,
  ROLLE,
  UNDO,
  SHEET,
  STOPP,
  { keys: "↑ ↓", label: "Zeile wählen", group: "Listen" },
  { keys: "↵", label: "Zeile öffnen", group: "Listen" },
  NAECHSTE,
  VORIGE,
  WARUM,
  FREIGEBEN,
  ABLEHNEN,
  UEBERNEHMEN,
  { keys: "↵", label: "Senden — fünf Sekunden lang mit ⌘Z zurückzunehmen", group: "Entscheiden" },
  { keys: "⌘↵", label: "In ⌘K: die zweite Aktion des Eintrags wählen", group: "Entscheiden" },
];
