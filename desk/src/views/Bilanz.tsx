import { useQuery } from "@tanstack/react-query";
import { useRolle } from "../app/rollen";
import { type Bilanz, katalog } from "../data/agent";
import { Icon } from "../design/Icon";
import { vorschlagText } from "../format/bilanz";
import { formatCount } from "../format/count";

/** The skill that answers on the Desk and prepares every proposal on a decision. */
export const DESK_ASSISTANT = "desk-assistant";

/**
 * agentd's catalog — every skill's Beipackzettel and its record — read once
 * for the Rolle and shared by every view that names a skill. A catalog that
 * cannot be read means the assistant is absent.
 */
export function useKatalog() {
  const { rolle } = useRolle();
  return useQuery({
    queryKey: [rolle, "agentd", "katalog"],
    queryFn: katalog,
    staleTime: 300_000,
    retry: false,
  });
}

/** A skill's record from the catalog; null while unread, absent, or not activated. */
export function useBilanz(name: string = DESK_ASSISTANT): Bilanz | null {
  return useKatalog().data?.find((f) => f.name === name)?.bilanz ?? null;
}

/** „In 30 Tagen: …" — the window agentd states, never one assumed here. */
export function imFenster(b: Bilanz, text: string): string {
  return `In ${formatCount(b.tage)} Tagen: ${text}`;
}

/**
 * One line under a proposal: what became of the assistant's proposals in the
 * window — how far to trust this one, from the record, never a verdict on it.
 */
export function VorschlagBilanz({ bilanz }: { bilanz?: Bilanz | null | undefined }) {
  const gelesen = useBilanz();
  const b = bilanz === undefined ? gelesen : bilanz;
  if (!b) return null;
  return (
    <p className="m-0 mt-2 flex items-center gap-1.5 text-caption text-text-2">
      <Icon name="bars" size={11} className="shrink-0" />
      {imFenster(b, vorschlagText(b.vorschlag))}
    </p>
  );
}
