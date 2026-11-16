// agentd opens every model-written text with its machine marking
// (`MACHINE_GENERATED`, „AI-generated. ", Verordnung (EU) 2024/1689 Art. 50).
// That marker is the service's, in English; the operator reads the Desk's own
// disclosure in the chosen Sprache in its place — never no disclosure.

import { katalog } from "../i18n";

/** agentd's marker, as it opens a text. */
const MARK = "AI-generated.";

/** The disclosure to show for agentd's `disclosure` field: the catalogue's, or none where agentd sent none. */
export function disclosure(said: string | null | undefined): string {
  return said?.trim() ? katalog().aiNote.disclosure : "";
}

/** A model-written text with agentd's leading marker said in the catalogue's words. */
export function disclosed(text: string): string {
  const t = text.trimStart();
  return t.startsWith(MARK) ? `${katalog().aiNote.disclosure}${t.slice(MARK.length)}` : text;
}
