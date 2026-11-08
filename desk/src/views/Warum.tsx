import { useQuery } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { Button as AriaButton } from "react-aria-components";
import { quelle } from "../app/dienste";
import { folgen, type Lauf } from "../app/laeufe";
import { useRolle } from "../app/rollen";
import {
  AgentAbgelehnt,
  type Anker,
  type Answer,
  fragen,
  type Schritt,
  WARUM_FRAGE,
} from "../data/agent";
import { useVerweigert, Verweigert } from "../data/verweigert";
import { Icon } from "../design/Icon";
import { Bewertung } from "./Bewertung";
import { Lesen, Satz } from "./Fragen";
import {
  Belege,
  SpurAnsicht,
  StoppKnopf,
  spurStand,
  useLaufStand,
  useOeffnen,
  useStoppTaste,
} from "./Spur";

/**
 * Where a „Warum?" stands: reading, answered, or why there is no answer.
 * `spur` is the streamed trail; without it the reads are the answer's own.
 */
export type WarumStand =
  | { kind: "liest"; schritte: Answer["schritte"]; spur?: Schritt[] }
  | { kind: "antwort"; a: Answer; spur?: Schritt[] }
  | { kind: "abgebrochen"; spur: Schritt[] }
  | { kind: "fehler"; text: string };

/**
 * The answer to „Warum?" as it stands — the disclosure first, then each
 * sentence with the read it cites and agentd's check of it, then „Hilfreich?".
 * Model-written, so set apart in the assistant's tint.
 */
export function WarumAntwort({
  stand,
  onNochmal,
  stopp,
}: {
  stand: WarumStand;
  onNochmal?: (() => void) | undefined;
  /** The live run, for its Stop. */
  stopp?: { run: string; lauf: Lauf | undefined } | undefined;
}) {
  const [oeffnen, aufSchritt] = useOeffnen();
  const spur = stand.kind === "fehler" ? undefined : stand.spur;
  return (
    <section
      aria-label="Warum steht das hier?"
      className="squircle rounded-panel border border-ai-border bg-ai-fill p-4"
    >
      <div className="flex min-h-9 items-center gap-2">
        <p className="m-0 flex flex-1 items-center gap-1.5 text-caption font-semibold text-ai">
          <Icon name="sparkle" size={12} />
          {WARUM_FRAGE}
        </p>
        {stand.kind === "liest" && stopp && <StoppKnopf run={stopp.run} lauf={stopp.lauf} />}
      </div>
      {spur && (
        <SpurAnsicht
          schritte={spur}
          stand={
            stand.kind === "abgebrochen"
              ? "abgebrochen"
              : stand.kind === "antwort"
                ? "beantwortet"
                : "live"
          }
          oeffnen={oeffnen}
        />
      )}
      {stand.kind === "liest" && !spur && <Lesen schritte={stand.schritte} />}
      {stand.kind === "fehler" && (
        <p className="m-0 mt-3 text-callout text-critical">{stand.text}</p>
      )}
      {stand.kind === "antwort" && (
        <div className="mt-3">
          <p className="m-0 mb-2 text-caption font-semibold text-ai">{stand.a.disclosure}</p>
          {stand.a.status === "succeeded" ? (
            <>
              <ul aria-label="Antwort" className="m-0 list-none divide-y divide-ai-border p-0">
                {stand.a.saetze.map((s) => (
                  <Satz
                    key={s.text}
                    s={s}
                    belege={spur && <Belege satz={s} schritte={spur} onOeffnen={aufSchritt} />}
                  />
                ))}
              </ul>
              {!spur && stand.a.schritte.length > 0 && (
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
 * the one question, answered beside the item, its reads streaming in as a
 * trail with Stop beside the question (and Esc). The run is started once per
 * Anker for the Rolle — closing and reopening reads the same answer, and
 * nothing refetches it behind the person's back.
 */
export function Warum({ anker }: { anker: Anker }) {
  const { rolle } = useRolle();
  const router = useRouter({ warn: false }) as ReturnType<typeof useRouter> | undefined;
  const verweigert = useVerweigert({ dienst: "agentd", aktion: "fragen" });
  const start = useQuery({
    queryKey: [rolle, "agentd", "warum", anker],
    queryFn: async () => {
      const run = await fragen(WARUM_FRAGE, anker);
      folgen(run, {
        frage: WARUM_FRAGE,
        anker: `warum:${JSON.stringify(anker)}`,
        rolle,
        zurueck: router?.state.location.href ?? `${location.pathname}${location.search}`,
        pille: false,
      });
      return run;
    },
    enabled: !verweigert,
    staleTime: Number.POSITIVE_INFINITY,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
    retry: false,
  });
  const run = start.data;
  const lauf = useLaufStand(run);
  useStoppTaste(run, lauf.live);
  const spur = lauf.gestreamt ? (lauf.lauf?.schritte ?? []) : undefined;
  const fehler = start.error ?? lauf.fehler;
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
      : spur && spurStand(lauf) === "abgebrochen"
        ? { kind: "abgebrochen", spur }
        : lauf.antwort
          ? { kind: "antwort", a: lauf.antwort, ...(spur ? { spur } : {}) }
          : { kind: "liest", schritte: lauf.gelesen, ...(spur ? { spur } : {}) };
  return (
    <WarumAntwort
      stand={stand}
      onNochmal={() => void start.refetch()}
      stopp={run && lauf.live ? { run, lauf: lauf.lauf } : undefined}
    />
  );
}
