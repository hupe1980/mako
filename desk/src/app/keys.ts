// Every key the Desk answers to, in one table: the handlers match through it
// and the ⌘/ sheet prints it, so a key cannot work without being listed or be
// listed without working.

export interface Shortcut {
  /** As printed on the keyboard. */
  keys: string;
  label: string;
  group: "Überall" | "Listen" | "Freigaben";
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

/** The key went to a field the person types into. */
const typing = (e: KeyboardEvent) => {
  const t = e.target as HTMLElement | null;
  return !!t && (t.tagName === "INPUT" || t.tagName === "TEXTAREA" || t.isContentEditable);
};

/** A bare letter, and not while the person types into a field. */
const letter = (k: string) => (e: KeyboardEvent) =>
  !typing(e) && !e.metaKey && !e.ctrlKey && !e.altKey && e.key.toLowerCase() === k;

export const NAECHSTE: Shortcut = {
  keys: "J",
  label: "Nächste Freigabe",
  group: "Freigaben",
  match: letter("j"),
};
export const VORIGE: Shortcut = {
  keys: "K",
  label: "Vorige Freigabe",
  group: "Freigaben",
  match: letter("k"),
};
export const FREIGEBEN: Shortcut = {
  keys: "A",
  label: "Freigeben wählen",
  group: "Freigaben",
  match: letter("a"),
};
export const ABLEHNEN: Shortcut = {
  keys: "R",
  label: "Ablehnen wählen",
  group: "Freigaben",
  match: letter("r"),
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

export const SHORTCUTS: Shortcut[] = [
  PALETTE,
  FRAGEN,
  ROLLE,
  UNDO,
  SHEET,
  { keys: "esc", label: "Schließen", group: "Überall" },
  { keys: "↑ ↓", label: "Zeile wählen", group: "Listen" },
  { keys: "↵", label: "Zeile öffnen", group: "Listen" },
  NAECHSTE,
  VORIGE,
  FREIGEBEN,
  ABLEHNEN,
  { keys: "↵", label: "Senden bestätigen", group: "Freigaben" },
];
