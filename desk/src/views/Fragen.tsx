import { useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useRef, useState } from "react";
import { Button as AriaButton, Form, Input, TextField } from "react-aria-components";
import { quelle } from "../app/dienste";
import { fragenAnmelden } from "../app/frage";
import { useHold } from "../app/Holds";
import { FRAGEN } from "../app/keys";
import { useRolle } from "../app/rollen";
import {
  AgentAbgelehnt,
  AKTION,
  type Anker,
  type AnkerArt,
  type Answer,
  ankerArt,
  antwort,
  type Bilanz,
  fragen,
  schicksal,
  type Vorschlag,
} from "../data/agent";
import { useVerweigert, Verweigert } from "../data/verweigert";
import { Icon } from "../design/Icon";
import type { BusinessDate } from "../format/brands";
import { formatBusinessDate } from "../format/date";
import { Bewertung } from "./Bewertung";
import { DESK_ASSISTANT, useKatalog, VorschlagBilanz } from "./Bilanz";

const VORSCHLAEGE: Record<AnkerArt, string[]> = {
  freigabe: ["Was schlägst du vor?", "Warum liegt das bei uns?", "Was kam zuletzt an?"],
  disput: ["Was schlägst du vor?", "Worauf beruht der Disput?", "Ist die Ablehnung versandt?"],
  kunde: [
    "Was ist bei diesem Kunden offen?",
    "Wie steht die Anmeldung?",
    "Welche Frist läuft zuerst ab?",
  ],
  malo: ["Was kam zuletzt an?", "Was ist hier offen?", "Wann endet die Frist?"],
  process: ["Was kam zuletzt an?", "Worauf wartet der Prozess?", "Wann endet die Frist?"],
  buchung: [
    "Was schlägst du vor?",
    "Warum hat das Hauptbuch sie abgelehnt?",
    "Ist der Beleg schon gebucht?",
  ],
};

/** One sentence of an answer, with the read it cites and agentd's check of it. */
export function Satz({ s }: { s: Answer["saetze"][number] }) {
  return (
    <li className="flex flex-col gap-1.5 py-2 first:pt-0 last:pb-0">
      <p className={`m-0 text-callout ${s.quelle ? "text-text" : "text-text-2 italic"}`}>
        {s.text}
      </p>
      {s.quelle && (
        <span className="flex flex-wrap items-center gap-2">
          <span className="inline-flex h-5 items-center gap-1 rounded-full border border-ai-border px-2 text-caption font-medium text-ai">
            <Icon name="doc" size={11} />
            {quelle(s.quelle)}
          </span>
          {!s.belegt && (
            <span
              title="Eine Zahl, ein Datum oder eine Kennung dieses Satzes steht nicht in der zitierten Abfrage."
              className="inline-flex h-5 items-center gap-1 rounded-full bg-soon-fill px-2 text-caption font-semibold text-soon"
            >
              <Icon name="warn" size={11} />
              nicht belegt
            </span>
          )}
        </span>
      )}
    </li>
  );
}

/** `anthropic/claude-sonnet-5` → `claude-sonnet-5 · anthropic`. */
function modellName(m: string): string {
  const [anbieter, name] = m.split("/");
  return name ? `${name} · ${anbieter}` : m;
}

/** What the run is reading, each read ticked off once it answered. */
export function Lesen({ schritte }: { schritte: Answer["schritte"] }) {
  if (schritte.length === 0) {
    return (
      <p role="status" className="m-0 mt-4 flex items-center gap-2 text-callout text-text-2">
        <span className="inline-block size-2 animate-pulse rounded-full bg-ai" aria-hidden="true" />
        Der Assistent überlegt, was er lesen muss …
      </p>
    );
  }
  return (
    <ul
      role="status"
      aria-label="Der Assistent liest"
      className="m-0 mt-4 flex list-none flex-col gap-1.5 p-0"
    >
      {schritte.map((s, i) => (
        // biome-ignore lint/suspicious/noArrayIndexKey: a run's reads only append, so a position names one read for good
        <li key={i} className="flex items-center gap-2 text-callout text-text-2">
          {s.fertig ? (
            <Icon name="check" size={14} className="text-ai" />
          ) : (
            <span
              className="inline-block size-2 animate-pulse rounded-full bg-ai"
              aria-hidden="true"
            />
          )}
          <span className={s.fertig ? "" : "text-text"}>
            {s.fertig ? "Gelesen" : "Liest"}: {quelle(s.quelle)}
          </span>
        </li>
      ))}
    </ul>
  );
}

/**
 * The assistant's proposal for a Freigabe, a Disput or a geparkte Buchung:
 * what to do, the code, finding or date it rests on, and the one sentence why,
 * checked like any other — with one line of how its proposals fared.
 * „Übernehmen" fills the decision form and sends nothing; „Verwerfen" hides it
 * and, once its hold ends, says so beside the run — a routine decision, taken
 * back with „Rückgängig" (FR-029).
 */
export function VorschlagKarte({
  v,
  run,
  onUebernehmen,
  onVerwerfen,
  disclosure,
  hinweis,
  befund,
  bilanz,
}: {
  v: Vorschlag;
  run: string;
  /** Absent where the proposal leaves nothing to fill in. */
  onUebernehmen?: ((v: Vorschlag, run: string) => void) | undefined;
  /** Told when the person sets the proposal aside (`true`) and takes that back (`false`). */
  onVerwerfen?: ((verworfen: boolean) => void) | undefined;
  /** agentd's disclosure, where the card stands outside the answer that carries it. */
  disclosure?: string;
  /** What taking it in would mean, where there is nothing to take in. */
  hinweis?: string | undefined;
  /**
   * The finding a Disput's proposal rests on, as the Rechnungseingang names it
   * in German; the proposal itself carries only the finding's identifier.
   */
  befund?: string | undefined;
  /** The assistant's record for the card's last line; read from the catalog when absent. */
  bilanz?: Bilanz | null | undefined;
}) {
  const [weg, setWeg] = useState(false);
  const hold = useHold();
  if (weg) return null;
  return (
    <div className="mt-3 rounded-[12px] border border-ai bg-surface p-3">
      {disclosure && <p className="m-0 mb-1 text-caption font-semibold text-ai">{disclosure}</p>}
      <p className="m-0 flex flex-wrap items-center gap-2 text-caption font-semibold text-ai">
        <Icon name="sparkle" size={12} />
        Vorschlag
        <span className="text-text">
          {AKTION[v.aktion]}
          {v.antwortcode && <span className="ml-1 font-mono">{v.antwortcode}</span>}
          {v.aktion === "neu_buchen" && v.buchungsdatum && (
            <span className="ml-1">am {formatBusinessDate(v.buchungsdatum as BusinessDate)}</span>
          )}
        </span>
        {befund && (
          <span className="font-normal text-text-2">
            wegen <span className="text-text">{befund}</span>
          </span>
        )}
      </p>
      <p className="m-0 mt-1 text-callout text-text">{v.begruendung}</p>
      {hinweis && <p className="m-0 mt-1 text-caption text-text-2">{hinweis}</p>}
      <div className="mt-2 flex flex-wrap items-center gap-2">
        {v.quelle && (
          <span className="inline-flex h-5 items-center gap-1 rounded-full border border-ai-border px-2 text-caption font-medium text-ai">
            <Icon name="doc" size={11} />
            {quelle(v.quelle)}
          </span>
        )}
        {!v.belegt && (
          <span className="inline-flex h-5 items-center gap-1 rounded-full bg-soon-fill px-2 text-caption font-semibold text-soon">
            <Icon name="warn" size={11} />
            nicht belegt
          </span>
        )}
        <span className="flex-1" />
        <AriaButton
          onPress={() => {
            setWeg(true);
            onVerwerfen?.(true);
            hold(
              `vorschlag/${run}`,
              "Vorschlag verworfen",
              // What became of a proposal is the assistant's quality signal;
              // a lost one costs nothing the person sees.
              (keepalive) => schicksal(run, "dismissed", keepalive),
              () => {
                setWeg(false);
                onVerwerfen?.(false);
              },
            );
          }}
          className="focus-ring h-7 rounded-full px-3 text-body text-text-2 data-[hovered]:bg-fill data-[hovered]:text-text"
        >
          Verwerfen
        </AriaButton>
        {onUebernehmen && (
          <AriaButton
            onPress={() => onUebernehmen(v, run)}
            aria-keyshortcuts="U"
            className="focus-ring inline-flex h-7 items-center gap-2 rounded-full bg-ai px-3 text-body font-semibold text-on-accent"
          >
            In die Entscheidung übernehmen
            <kbd aria-hidden className="font-sans text-caption opacity-80">
              U
            </kbd>
          </AriaButton>
        )}
      </div>
      <VorschlagBilanz bilanz={bilanz} />
    </div>
  );
}

/**
 * Fragen: ask the assistant about the Anker on screen. The answer comes
 * sentence by sentence, each with the service read it rests on, and a sentence
 * whose figures that read does not carry is marked — agentd checked it, the
 * Desk only shows the verdict.
 */
export function Fragen({
  anker,
  onVorschlag,
}: {
  anker: Anker;
  /** Where a proposal goes when the person takes it — the decision form. */
  onVorschlag?: (v: Vorschlag, run: string) => void;
}) {
  const { rolle } = useRolle();
  const [frage, setFrage] = useState("");
  const feld = useRef<HTMLInputElement>(null);
  // The catalog names the model that answers, and a catalog that cannot be
  // read means agentd is absent: then Fragen is not offered at all.
  const skill = useKatalog();
  const modell = skill.data?.find((f) => f.name === DESK_ASSISTANT)?.modell;
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!FRAGEN.match?.(e)) return;
      e.preventDefault();
      feld.current?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
  const verweigert = useVerweigert({ dienst: "agentd", aktion: "fragen" });
  const kind = ankerArt(anker);
  const start = useMutation({ mutationFn: (q: string) => fragen(q, anker) });
  const run = start.data;
  const answer = useQuery({
    queryKey: [rolle, "agentd", "antwort", run],
    queryFn: () => antwort(run ?? ""),
    enabled: Boolean(run),
    refetchInterval: (q) => (q.state.data?.status === "running" ? 1000 : false),
  });
  const ask = (q: string) => {
    if (!q.trim()) return;
    setFrage(q);
    start.mutate(q.trim());
  };
  // ⌘K can ask too: the palette offers it while this panel is on screen.
  const askRef = useRef(ask);
  askRef.current = ask;
  useEffect(() => fragenAnmelden((q) => askRef.current(q)), []);
  const reading = start.isPending || answer.data?.status === "running" || (run && !answer.data);
  if (skill.isError) return null;
  if (verweigert) {
    return (
      <section
        aria-label="Fragen"
        className="squircle flex items-center gap-2 rounded-panel border border-ai-border bg-ai-fill p-4 text-callout text-text-2"
      >
        <Icon name="sparkle" size={16} className="shrink-0 text-ai" />
        Den Assistenten zu fragen ist Ihnen nicht erlaubt: {verweigert}
      </section>
    );
  }
  const fehler = start.error ?? answer.error;

  return (
    <section
      aria-label="Fragen"
      className="squircle rounded-panel border border-ai-border bg-ai-fill p-4"
    >
      <Form
        onSubmit={(e) => {
          e.preventDefault();
          ask(frage);
        }}
        className="flex items-center gap-2"
      >
        <Icon name="sparkle" size={16} className="shrink-0 text-ai" />
        <TextField
          aria-label="Frage an den Assistenten"
          value={frage}
          onChange={setFrage}
          className="flex-1"
        >
          <Input
            ref={feld}
            placeholder="Fragen Sie den Assistenten …  ⌘J"
            className="h-9 w-full rounded-control bg-surface px-3 text-callout text-text outline-none placeholder:text-text-2 focus:ring-2 focus:ring-ai-border"
          />
        </TextField>
        <AriaButton
          type="submit"
          isDisabled={!frage.trim() || Boolean(reading)}
          className="focus-ring h-9 rounded-control bg-ai px-3 text-body font-semibold text-on-accent data-[disabled]:opacity-40"
        >
          Fragen
        </AriaButton>
      </Form>

      {modell && (
        <p className="m-0 mt-2 pl-6 text-caption text-text-2">
          Antwortet mit {modellName(modell)} · liest nur, handelt nie
        </p>
      )}

      {!run && !start.isPending && (
        <div className="mt-3 flex flex-wrap gap-2">
          {VORSCHLAEGE[kind].map((v) => (
            <AriaButton
              key={v}
              onPress={() => ask(v)}
              className="focus-ring h-7 rounded-full border border-ai-border bg-surface px-3 text-body text-text data-[hovered]:bg-fill"
            >
              {v}
            </AriaButton>
          ))}
        </div>
      )}

      {reading && <Lesen schritte={answer.data?.schritte ?? []} />}

      {fehler && !(fehler instanceof Verweigert) && (
        <p className="m-0 mt-4 text-callout text-critical">
          {fehler instanceof AgentAbgelehnt
            ? `Der Assistent hat die Frage nicht angenommen: ${fehler.message}`
            : "Der Assistent ist nicht erreichbar. Alles andere im Desk funktioniert weiter."}
        </p>
      )}

      {answer.data && answer.data.status !== "running" && (
        <div className="mt-4">
          <p className="m-0 mb-2 text-caption font-semibold text-ai">{answer.data.disclosure}</p>
          {answer.data.status === "succeeded" ? (
            <>
              <ul className="m-0 list-none divide-y divide-ai-border p-0">
                {answer.data.saetze.map((s) => (
                  <Satz key={s.text} s={s} />
                ))}
              </ul>
              {answer.data.vorschlag && onVorschlag && (
                <VorschlagKarte
                  v={answer.data.vorschlag}
                  run={answer.data.run}
                  onUebernehmen={onVorschlag}
                />
              )}
              {answer.data.schritte.length > 0 && (
                <p className="m-0 mt-3 text-caption text-text-2">
                  Gelesen:{" "}
                  {[...new Set(answer.data.schritte.map((s) => quelle(s.quelle)))].join(", ")}
                </p>
              )}
              <div className="mt-3">
                <Bewertung run={answer.data.run} initial={answer.data.hilfreich} />
              </div>
            </>
          ) : (
            <div className="flex flex-wrap items-center gap-3">
              <p className="m-0 text-callout text-text-2">
                Der Assistent hat keine Antwort gefunden ({answer.data.status}).
              </p>
              <AriaButton
                onPress={() => ask(frage)}
                className="focus-ring h-7 rounded-full border border-ai-border bg-surface px-3 text-body text-text data-[hovered]:bg-fill"
              >
                Nochmal fragen
              </AriaButton>
            </div>
          )}
        </div>
      )}
    </section>
  );
}
