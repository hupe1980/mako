// Every key the Desk answers to, in one table: the handlers match through it
// and the ⌘/ sheet prints it, so a key cannot work without being listed or be
// listed without working.

export interface Shortcut {
  /** As printed on the keyboard. */
  keys: string;
  label: string;
  group: "Überall" | "Listen";
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
  SHEET,
  { keys: "esc", label: "Schließen", group: "Überall" },
  { keys: "↑ ↓", label: "Zeile wählen", group: "Listen" },
  { keys: "↵", label: "Zeile öffnen", group: "Listen" },
];
