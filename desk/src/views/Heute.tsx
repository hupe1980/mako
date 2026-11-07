import { useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { type RefObject, useEffect, useMemo, useRef, useState } from "react";
import { NAECHSTE, UEBERNEHMEN, VORIGE } from "../app/keys";
import { useRolle } from "../app/rollen";
import { AKTION, type Answer, type Hinweis } from "../data/agent";
import { FristUhr } from "../design/FristUhr";
import { Icon, type IconName } from "../design/Icon";
import { EmptyState, Panel } from "../design/Panel";
import { formatCount } from "../format/count";
import { epochMs } from "../format/date";
import { BAENDER, type Band, band, useNow } from "../format/frist";
import type { Eintrag, QuelleId, Stand } from "../inbox/model";
import { alsEintrag, useBenannt, useHinweise, useQuellen, useVorschlaege } from "../inbox/quellen";
import { Bewertung } from "./Bewertung";
import { Fragen } from "./Fragen";
import { Entscheiden } from "./Freigaben";
import { Lage } from "./Lage";
import { DisputEntscheiden, PruefpostenEntscheiden } from "./Rechnungen";

/** What a row's glyph says about where it came from. */
const GLYPH: Record<QuelleId, IconName> = {
  freigaben: "check",
  ueberfaellig: "warn",
  anmeldungen: "flow",
  nicht_uebermittelt: "tray",
  korrekturbedarf: "bars",
  ableseauftraege: "clock",
  dispute: "doc",
  pruefposten: "doc",
  hinweise: "sparkle",
};

/** At most this many rows in the DOM, however many items wait (SC-008). */
const SICHTBAR = 100;

/**
 * Every source's items as one list: one item per id, the agent's Hinweise
 * attached to the item that carries their Anker — and those no item carries
 * listed in their own right — ordered by the Frist the service returned,
 * items without one last.
 */
function zusammenfuehren(
  staende: Stand[],
  hinweise: Hinweis[],
): { eintraege: Eintrag[]; zu: Map<string, Hinweis[]> } {
  const gesehen = new Map<string, Eintrag>();
  for (const s of staende)
    for (const e of s.eintraege) if (!gesehen.has(e.id)) gesehen.set(e.id, e);
  const eintraege = [...gesehen.values()];
  const zu = new Map<string, Hinweis[]>();
  for (const h of hinweise) {
    const traeger = eintraege.find((e) => h.anker.some((a) => e.bezug.includes(a.wert)));
    if (traeger) zu.set(traeger.id, [...(zu.get(traeger.id) ?? []), h]);
    else eintraege.push(alsEintrag(h));
  }
  eintraege.sort((a, b) => {
    if (!a.frist || !b.frist) return a.frist ? -1 : b.frist ? 1 : 0;
    return epochMs(a.frist) - epochMs(b.frist);
  });
  return { eintraege, zu };
}

function HinweisZeile({ h }: { h: Hinweis }) {
  return (
    <p className="m-0 flex gap-2 rounded-[10px] border border-ai-border bg-ai-fill px-3 py-2 text-body text-text">
      <Icon name="sparkle" size={14} className="mt-0.5 shrink-0 text-ai" />
      <span className="min-w-0">
        <span className="font-semibold text-ai">{h.titel ?? h.skill ?? "Assistent"}: </span>
        <span className="italic">{h.text}</span>
      </span>
    </p>
  );
}

/**
 * The proposal waiting on a Freigabe or a Disput, one line under its row:
 * model-written, so set apart and carrying the disclosure; `U` takes it into
 * the decision.
 */
function VorschlagZeile({ a, befund }: { a: Answer; befund?: string | undefined }) {
  const v = a.vorschlag;
  if (!v) {
    if (a.status !== "running") return null;
    return (
      <p className="m-0 flex items-center gap-2 text-caption text-ai">
        <Icon name="sparkle" size={12} className="animate-pulse" />
        Der Assistent bereitet einen Vorschlag vor …
      </p>
    );
  }
  return (
    <p className="m-0 flex gap-2 rounded-[10px] border border-ai-border bg-ai-fill px-3 py-2 text-body text-text">
      <Icon name="sparkle" size={14} className="mt-0.5 shrink-0 text-ai" />
      <span className="min-w-0 flex-1">
        <span className="font-semibold text-ai">
          {a.disclosure} Vorschlag: {AKTION[v.aktion]}
          {v.antwortcode && <span className="ml-1 font-mono">{v.antwortcode}</span>}
          {befund && <span className="ml-1 font-normal">wegen {befund}</span>}
          {" — "}
        </span>
        <span className="italic">{v.begruendung}</span>
        {!v.belegt && (
          <span className="ml-2 inline-flex h-5 items-center gap-1 rounded-full bg-soon-fill px-2 align-middle text-caption font-semibold text-soon">
            <Icon name="warn" size={11} />
            nicht belegt
          </span>
        )}
      </span>
      <kbd
        aria-hidden
        className="shrink-0 self-center rounded-[6px] border border-ai-border px-1.5 font-sans text-caption text-ai"
      >
        U
      </kbd>
    </p>
  );
}

function Zeile({
  e,
  hinweise,
  vorschlag,
  sofort,
  aktiv,
  offen,
  index,
  onFocus,
  onOeffnen,
  onEntschieden,
  zeilen,
}: {
  e: Eintrag;
  hinweise: Hinweis[];
  /** The proposal waiting on a Freigabe or Disput row. */
  vorschlag: Answer | undefined;
  /** `U` opened the decision: take the proposal into it. */
  sofort: boolean;
  aktiv: boolean;
  /** The item's decision is open under it. */
  offen: boolean;
  index: number;
  onFocus: (i: number) => void;
  onOeffnen: (id: string | null) => void;
  onEntschieden: () => void;
  zeilen: RefObject<(HTMLButtonElement | null)[]>;
}) {
  const router = useRouter();
  const ai = e.quelle === "hinweise";
  const entscheidbar = Boolean(e.freigabe || e.disput || e.pruefposten);
  return (
    <li className="border-b border-separator last:border-b-0">
      <button
        type="button"
        ref={(el) => {
          zeilen.current[index] = el;
        }}
        tabIndex={aktiv ? 0 : -1}
        aria-expanded={entscheidbar ? offen : undefined}
        onFocus={() => onFocus(index)}
        onClick={() => {
          // A Freigabe, a Disput and a Prüfposten are decided here;
          // everything else opens its view.
          if (entscheidbar) onOeffnen(offen ? null : e.id);
          else if (e.ziel) router.navigate(e.ziel);
        }}
        className={`focus-ring flex w-full cursor-default items-start gap-4 border-0 px-5 py-3 text-left outline-none data-[aktiv=true]:bg-fill hover:bg-fill ${ai ? "bg-ai-fill" : "bg-transparent"}`}
        data-aktiv={aktiv}
      >
        <span
          className={`mt-0.5 flex size-8 shrink-0 items-center justify-center rounded-full ${ai ? "bg-ai-fill text-ai" : "bg-[color-mix(in_srgb,var(--accent)_14%,transparent)] text-accent"}`}
        >
          <Icon name={GLYPH[e.quelle]} size={16} />
        </span>
        <span className="min-w-0 flex-1">
          <span
            className={`block truncate text-callout font-medium text-text ${e.kennung ? "font-mono" : ""}`}
          >
            {e.titel}
          </span>
          <span className={`block text-body ${ai ? "text-text italic" : "text-text-2"}`}>
            {e.grund}
          </span>
          {e.detail && (
            <span className="block truncate font-mono text-caption text-text-2">{e.detail}</span>
          )}
        </span>
        <span className="flex shrink-0 flex-col items-end gap-1 pt-0.5">
          {e.frist ? (
            <FristUhr due={e.frist} />
          ) : (
            <span className="text-body text-text-2">keine Frist</span>
          )}
          {entscheidbar && (
            <span className="text-caption text-accent">{offen ? "schließen" : "entscheiden"}</span>
          )}
        </span>
      </button>
      {vorschlag && !offen && (
        <div className="pr-5 pb-3 pl-[4.25rem]">
          <VorschlagZeile
            a={vorschlag}
            befund={
              e.disput?.befunde.find((b) => b.art === vorschlag.vorschlag?.befund)?.bezeichnung
            }
          />
        </div>
      )}
      {hinweise.length > 0 && (
        <div className="flex flex-col gap-2 pr-5 pb-3 pl-[4.25rem]">
          {hinweise.map((h) => (
            <div key={h.task}>
              <HinweisZeile h={h} />
              {aktiv && h.run && (
                <div className="mt-1.5">
                  <Bewertung run={h.run} />
                </div>
              )}
            </div>
          ))}
        </div>
      )}
      {offen && entscheidbar && (
        <div className="flex flex-col gap-3 border-t border-separator bg-fill/40 px-5 py-4">
          {e.freigabe && <Entscheiden f={e.freigabe} onDone={onEntschieden} sofort={sofort} />}
          {e.disput && <DisputEntscheiden d={e.disput} onDone={onEntschieden} sofort={sofort} />}
          {e.pruefposten && <PruefpostenEntscheiden r={e.pruefposten} onDone={onEntschieden} />}
        </div>
      )}
    </li>
  );
}

/** One quiet line per source: its count, or its refusal in its own words. */
function Quellenzeile({ staende, agent }: { staende: Stand[]; agent: string | null }) {
  return (
    <ul
      aria-label="Quellen"
      className="m-0 flex list-none flex-wrap gap-x-4 gap-y-1 p-0 px-1 text-caption text-text-2"
    >
      {staende.map((s) => (
        <li key={s.quelle} className={s.fehler ? "text-critical" : undefined}>
          {s.label}:{" "}
          {s.fehler
            ? s.fehler
            : s.laedt
              ? "lädt …"
              : `${formatCount(s.eintraege.length)}${s.mehr ? "+" : ""}`}
        </li>
      ))}
      {agent && <li>Assistent: nicht erreichbar</li>}
    </ul>
  );
}

/**
 * Heute: everything that waits on a person in the Rolle worn, one list in
 * Frist bands. The agent's Hinweise sit under the item they concern; `⌘J`
 * asks the assistant about the item in focus (FR-006, FR-007, FR-031).
 */
export function HeuteView() {
  const { rolle } = useRolle();
  const staende = useQuellen(rolle);
  const { hinweise, fehler: agentFehler } = useHinweise();
  const now = useNow();
  const gemischt = useMemo(() => zusammenfuehren(staende, hinweise), [staende, hinweise]);
  const eintraege = useBenannt(gemischt.eintraege, rolle === "lieferant");
  const { zu } = gemischt;
  const [aktiv, setAktiv] = useState(0);
  const [offen, setOffen] = useState<string | null>(null);
  const [sofort, setSofort] = useState<string | null>(null);
  const client = useQueryClient();
  const zeilen = useRef<(HTMLButtonElement | null)[]>([]);
  const sichtbar = eintraege.slice(0, SICHTBAR);
  const laedt = staende.some((s) => s.laedt) && eintraege.length === 0;
  const vorschlaege = useVorschlaege({
    freigabe: sichtbar.flatMap((e) => (e.freigabe ? [e.freigabe.id] : [])),
    disput: sichtbar.flatMap((e) => (e.disput ? [e.disput.id] : [])),
  });
  const wartend = (e: Eintrag | undefined) =>
    e?.freigabe
      ? vorschlaege.freigabe.get(e.freigabe.id)
      : e?.disput
        ? vorschlaege.disput.get(e.disput.id)
        : undefined;
  const bereit = [...vorschlaege.freigabe.values(), ...vorschlaege.disput.values()].filter(
    (a) => a.vorschlag,
  ).length;

  const gruppen = BAENDER.map(({ band: b, titel }) => ({
    b,
    titel,
    zeilen: sichtbar.map((e, i) => ({ e, i })).filter(({ e }) => band(e.frist, now) === b),
  })).filter((g) => g.zeilen.length > 0);
  const zahl = (b: Band) => gruppen.find((g) => g.b === b)?.zeilen.length ?? 0;

  // J/K and the arrows step through every band as one list.
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      // The arrows move only from a row; J and K from anywhere but a field.
      const pfeil = ev.key === "ArrowDown" || ev.key === "ArrowUp";
      if (pfeil && !zeilen.current.some((z) => z === document.activeElement)) return;
      // U on a Freigabe or Disput row opens its decision with the proposal
      // taken in; once the decision is open, the form answers U itself.
      const fokussiert = sichtbar[aktiv];
      if (UEBERNEHMEN.match?.(ev) && fokussiert && offen !== fokussiert.id) {
        if (!wartend(fokussiert)?.vorschlag) return;
        ev.preventDefault();
        setSofort(fokussiert.id);
        setOffen(fokussiert.id);
        return;
      }
      const vor = ev.key === "ArrowDown" || NAECHSTE.match?.(ev);
      const zurueck = ev.key === "ArrowUp" || VORIGE.match?.(ev);
      if (!vor && !zurueck) return;
      ev.preventDefault();
      const n = Math.min(sichtbar.length, SICHTBAR);
      if (n === 0) return;
      const next = (aktiv + (vor ? 1 : -1) + n) % n;
      setAktiv(next);
      zeilen.current[next]?.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const fokus = sichtbar[aktiv];

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">Heute</h1>
        <p className="m-0 mt-1 text-callout text-text-2">
          {eintraege.length === 0
            ? "Was auf Sie wartet — die nächste Frist zuerst."
            : [
                zahl("ueberfaellig") && `${formatCount(zahl("ueberfaellig"))} überfällig`,
                zahl("heute") && `${formatCount(zahl("heute"))} heute fällig`,
                `${formatCount(eintraege.length)} insgesamt`,
                bereit > 0 && `${formatCount(bereit)} mit Vorschlag`,
              ]
                .filter(Boolean)
                .join(" · ")}
        </p>
      </header>
      {rolle === "lieferant" && <Lage />}

      {laedt && (
        <Panel className="flex flex-col gap-3 p-5">
          <span className="sr-only">Lädt …</span>
          <div className="skeleton h-4 w-2/3" />
          <div className="skeleton h-4 w-1/2" />
          <div className="skeleton h-4 w-3/5" />
        </Panel>
      )}

      {!laedt && eintraege.length === 0 && (
        <Panel>
          <EmptyState title="Alles erledigt.">Nichts wartet auf Sie.</EmptyState>
        </Panel>
      )}

      {gruppen.map((g) => (
        <section key={g.b} aria-labelledby={`band-${g.b}`} className="flex flex-col gap-2">
          <h2
            id={`band-${g.b}`}
            className={`m-0 flex items-baseline gap-2 px-1 text-callout font-semibold ${g.b === "ueberfaellig" ? "text-critical" : ""}`}
          >
            {g.titel}
            <span className="font-normal text-text-2">{formatCount(g.zeilen.length)}</span>
          </h2>
          <Panel className="overflow-hidden">
            <ol aria-label={g.titel} className="m-0 list-none p-0">
              {g.zeilen.map(({ e, i }) => (
                <Zeile
                  key={e.id}
                  e={e}
                  hinweise={zu.get(e.id) ?? []}
                  vorschlag={wartend(e)}
                  sofort={sofort === e.id}
                  aktiv={i === aktiv}
                  offen={offen === e.id}
                  index={i}
                  onFocus={setAktiv}
                  onOeffnen={(id) => {
                    setSofort(null);
                    setOffen(id);
                  }}
                  onEntschieden={() => {
                    setOffen(null);
                    void client.invalidateQueries();
                  }}
                  zeilen={zeilen}
                />
              ))}
            </ol>
          </Panel>
        </section>
      ))}

      {eintraege.length > SICHTBAR && (
        <p className="m-0 px-1 text-body text-text-2">
          Die ersten {formatCount(SICHTBAR)} von {formatCount(eintraege.length)} — der Rest folgt,
          sobald diese erledigt sind.
        </p>
      )}

      {fokus?.anker && !offen && (
        <section aria-label={`Fragen zu ${fokus.titel}`} className="flex flex-col gap-2">
          <h2 className="m-0 px-1 text-callout font-semibold">
            Fragen zu <span className={fokus.kennung ? "font-mono" : ""}>{fokus.titel}</span>
          </h2>
          <Fragen key={JSON.stringify(fokus.anker)} anker={fokus.anker} />
        </section>
      )}

      <Quellenzeile staende={staende} agent={agentFehler} />
      <p className="m-0 px-1 text-caption text-text-2">
        J K oder ↑↓ wählen · ↵ öffnet, Freigaben, Dispute und Prüfposten werden hier entschieden (A
        / R, U übernimmt den Vorschlag) · ⌘J fragt den Assistenten zum gewählten Eintrag
      </p>
    </div>
  );
}
