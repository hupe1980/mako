import { useRouter } from "@tanstack/react-router";
import { Button } from "react-aria-components";
import { Icon } from "../design/Icon";
import { katalog, useT } from "../i18n";
import { schrittZeile } from "./dienste";
import { beendet, type Lauf, useLaeufe } from "./laeufe";
import { useRolle } from "./rollen";

/** What a pill says: the run's latest step, or how it ended. */
function zuletzt(l: Lauf): string {
  const k = katalog().laufPills;
  if (l.antwort) return k.antwortBereit;
  if (l.abgebrochen) return k.gestoppt;
  const s = l.schritte[l.schritte.length - 1];
  return s ? schrittZeile(s) : k.ueberlegt;
}

/**
 * The runs that streamed on after their view was left, as pills in the top
 * bar — newest first, each with its latest step, as a Live Activity shows
 * one. Choosing a pill returns to the view that asked; once the answer has
 * been read there, the pill is gone. Only the Rolle's own runs are shown.
 */
export function LaufPills() {
  const t = useT();
  const { rolle } = useRolle();
  const router = useRouter();
  const pills = useLaeufe().filter(
    (l) => l.pille && l.rolle === rolle && !l.unlesbar && !l.gelesen && l.zuschauer === 0,
  );
  if (pills.length === 0) return null;
  return (
    <ul
      aria-label={t.laufPills.liste}
      className="m-0 flex min-w-0 list-none gap-2 overflow-hidden p-0"
    >
      {pills.map((l) => (
        <li key={l.run} className="einblenden min-w-0">
          <LaufPill lauf={l} onPress={() => router.history.push(l.zurueck)} />
        </li>
      ))}
    </ul>
  );
}

/** One pill: a live hairline, or how the run ended, and its latest step. */
export function LaufPill({ lauf, onPress }: { lauf: Lauf; onPress: () => void }) {
  const t = useT();
  return (
    <Button
      onPress={onPress}
      aria-description={t.laufPills.frage(lauf.frage)}
      className="morph focus-ring flex h-7 max-w-72 items-center gap-1.5 rounded-full border border-ai-border bg-ai-fill px-2.5 text-caption font-medium text-ai data-[hovered]:border-ai"
    >
      {beendet(lauf) ? (
        <Icon name={lauf.antwort ? "check" : "stop"} size={12} className="shrink-0" />
      ) : (
        <span aria-hidden className="haarlinie w-3 shrink-0" />
      )}
      <span className="truncate text-text">{zuletzt(lauf)}</span>
    </Button>
  );
}
