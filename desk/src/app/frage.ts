// The way from ⌘K to the assistant on screen: a page whose Anker carries a
// Fragen panel says so while it is mounted, and the palette hands it a
// question as an event. Neither knows the other's internals.

const EVENT = "desk:fragen";
let panels = 0;

/** A Fragen panel is on screen — so the palette may offer to ask. */
export function frageMoeglich(): boolean {
  return panels > 0;
}

/** Mark a Fragen panel mounted; the returned function unmarks it. */
export function fragenAnmelden(onFrage: (frage: string) => void): () => void {
  panels += 1;
  const listener = (e: Event) => onFrage((e as CustomEvent<string>).detail);
  window.addEventListener(EVENT, listener);
  return () => {
    panels -= 1;
    window.removeEventListener(EVENT, listener);
  };
}

/** Ask the panel on screen. */
export function frageStellen(frage: string): void {
  window.dispatchEvent(new CustomEvent(EVENT, { detail: frage }));
}
