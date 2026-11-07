import { useQuery } from "@tanstack/react-query";
import { Button as AriaButton } from "react-aria-components";
import { quelle } from "../app/dienste";
import { useRolle } from "../app/rollen";
import {
  AgentAbgelehnt,
  type Anker,
  type Answer,
  antwort,
  fragen,
  WARUM_FRAGE,
} from "../data/agent";
import { useVerweigert, Verweigert } from "../data/verweigert";
import { Icon } from "../design/Icon";
import { Bewertung } from "./Bewertung";
import { Lesen, Satz } from "./Fragen";

/** Where a „Warum?" stands: reading, answered, or why there is no answer. */
export type WarumStand =
  | { kind: "liest"; schritte: Answer["schritte"] }
  | { kind: "antwort"; a: Answer }
  | { kind: "fehler"; text: string };

/**
 * The answer to „Warum?" as it stands — the disclosure first, then each
 * sentence with the read it cites and agentd's check of it, then „Hilfreich?".
 * Model-written, so set apart in the assistant's tint.
 */
export function WarumAntwort({
  stand,
  onNochmal,
}: {
  stand: WarumStand;
  onNochmal?: (() => void) | undefined;
}) {
  return (
    <section
      aria-label="Warum steht das hier?"
      className="squircle rounded-panel border border-ai-border bg-ai-fill p-4"
    >
      <p className="m-0 flex items-center gap-1.5 text-caption font-semibold text-ai">
        <Icon name="sparkle" size={12} />
        {WARUM_FRAGE}
      </p>
      {stand.kind === "liest" && <Lesen schritte={stand.schritte} />}
      {stand.kind === "fehler" && (
        <p className="m-0 mt-3 text-callout text-critical">{stand.text}</p>
      )}
      {stand.kind === "antwort" && (
        <div className="mt-3">
          <p className="m-0 mb-2 text-caption font-semibold text-ai">{stand.a.disclosure}</p>
          {stand.a.status === "succeeded" ? (
            <>
              <ul className="m-0 list-none divide-y divide-ai-border p-0">
                {stand.a.saetze.map((s) => (
                  <Satz key={s.text} s={s} />
                ))}
              </ul>
              {stand.a.schritte.length > 0 && (
                <p className="m-0 mt-3 text-caption text-text-2">
                  Gelesen: {[...new Set(stand.a.schritte.map((s) => quelle(s.quelle)))].join(", ")}
                </p>
              )}
              <div className="mt-3">
                <Bewertung run={stand.a.run} initial={stand.a.hilfreich} />
              </div>
            </>
          ) : (
            <div className="flex flex-wrap items-center gap-3">
              <p className="m-0 text-callout text-text-2">
                Der Assistent hat keine Antwort gefunden ({stand.a.status}).
              </p>
              {onNochmal && (
                <AriaButton
                  onPress={onNochmal}
                  className="focus-ring h-7 rounded-full border border-ai-border bg-surface px-3 text-body text-text data-[hovered]:bg-fill"
                >
                  Nochmal fragen
                </AriaButton>
              )}
            </div>
          )}
        </div>
      )}
    </section>
  );
}

/**
 * „Warum?" on a Heute item: one desk-assistant run with the item's Anker and
 * the one question, answered beside the item. The run is started once per
 * Anker for the Rolle — closing and reopening reads the same answer, and
 * nothing refetches it behind the person's back.
 */
export function Warum({ anker }: { anker: Anker }) {
  const { rolle } = useRolle();
  const verweigert = useVerweigert({ dienst: "agentd", aktion: "fragen" });
  const start = useQuery({
    queryKey: [rolle, "agentd", "warum", anker],
    queryFn: () => fragen(WARUM_FRAGE, anker),
    enabled: !verweigert,
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: false,
  });
  const run = start.data;
  const answer = useQuery({
    queryKey: [rolle, "agentd", "antwort", run],
    queryFn: () => antwort(run ?? ""),
    enabled: Boolean(run),
    refetchInterval: (q) => (q.state.data?.status === "running" ? 1000 : false),
  });
  const fehler = start.error ?? answer.error;
  const stand: WarumStand = verweigert
    ? { kind: "fehler", text: `Den Assistenten zu fragen ist Ihnen nicht erlaubt: ${verweigert}` }
    : fehler && !(fehler instanceof Verweigert)
      ? {
          kind: "fehler",
          text:
            fehler instanceof AgentAbgelehnt
              ? `Der Assistent hat die Frage nicht angenommen: ${fehler.message}`
              : "Der Assistent ist nicht erreichbar. Alles andere im Desk funktioniert weiter.",
        }
      : answer.data && answer.data.status !== "running"
        ? { kind: "antwort", a: answer.data }
        : { kind: "liest", schritte: answer.data?.schritte ?? [] };
  return <WarumAntwort stand={stand} onNochmal={() => void start.refetch()} />;
}
