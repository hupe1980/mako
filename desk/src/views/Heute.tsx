import { useQueryClient } from "@tanstack/react-query";
import { Link, useRouter, useSearch } from "@tanstack/react-router";
import { lazy, type RefObject, Suspense, useEffect, useMemo, useRef, useState } from "react";
import { Button as AriaButton } from "react-aria-components";
import { entscheidbar, fokussieren, gefunden } from "../app/aktionen";
import { disclosed, disclosure } from "../app/disclosure";
import { Entscheidungsstand } from "../app/Entscheidungsstand";
import { NAECHSTE, UEBERNEHMEN, VORIGE, WARUM } from "../app/keys";
import { useRolle } from "../app/rollen";
import { AKTION, type Answer, type Hinweis } from "../data/agent";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { Page } from "../design/Page";
import { EmptyState, Panel } from "../design/Panel";
import type { BusinessDate } from "../format/brands";
import { formatCount } from "../format/count";
import { epochMs, formatBusinessDate, formatInstantDay } from "../format/date";
import { BAENDER, band, useNow } from "../format/frist";
import { useT } from "../i18n";
import { type Eintrag, type QuelleId, type Stand, warumAnker } from "../inbox/model";
import { alsEintrag, useBenannt, useHinweise, useQuellen, useVorschlaege } from "../inbox/quellen";
import { Bewertung } from "./Bewertung";
import { GeparktEntscheiden } from "./Buchungen";
import { Fragen } from "./Fragen";
import { Entscheiden } from "./Freigaben";
import { DisputEntscheiden, GehaltenEntscheiden, PruefpostenEntscheiden } from "./Rechnungen";

// The Lieferant's Lage line loads with its Rolle.
const Lage = lazy(() => import("./Lage").then((m) => ({ default: m.Lage })));

// „Warum?" asks the assistant only when pressed: its trail loads then.
const Warum = lazy(() => import("./Warum").then((m) => ({ default: m.Warum })));

/** At most this many rows in the DOM, however many items wait. */
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
    // A Hinweis that names a process sits under that process's row only — a
    // MaLo can carry several, and a note about one breached Frist must not
    // land on another that is still running. One with no process goes by its MaLo.
    const prozesse = h.anker.filter((a) => a.art === "process").map((a) => a.wert);
    const traeger =
      prozesse.length > 0
        ? eintraege.find((e) => prozesse.some((p) => e.bezug.includes(p)))
        : eintraege.find((e) => h.anker.some((a) => e.bezug.includes(a.wert)));
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
  const t = useT();
  return (
    <p className="m-0 flex gap-2 border-l-2 border-ai pl-3 text-body text-text">
      <Icon name="sparkle" size={14} className="mt-0.5 shrink-0 text-ai" />
      <span className="min-w-0">
        <span className="font-semibold">{h.titel ?? h.skill ?? t.heute.assistent}: </span>
        {disclosed(h.text)}
      </span>
    </p>
  );
}

/**
 * The proposal waiting on a Freigabe, a Disput or a geparkte Buchung, one line under its row:
 * model-written, so set apart and carrying the disclosure; `U` takes it into
 * the decision.
 */
function VorschlagZeile({ a, befund }: { a: Answer; befund?: string | undefined }) {
  const t = useT();
  const v = a.vorschlag;
  if (!v) {
    if (a.status !== "running") return null;
    return (
      <p className="m-0 flex items-center gap-2 text-caption text-ai">
        <Icon name="sparkle" size={12} className="animate-pulse" />
        {t.heute.bereitetVor}
      </p>
    );
  }
  return (
    <p className="m-0 flex gap-2 border-l-2 border-ai pl-3 text-body text-text">
      <Icon name="sparkle" size={14} className="mt-0.5 shrink-0 text-ai" />
      <span className="min-w-0 flex-1">
        <span className="font-semibold">
          {disclosure(a.disclosure)} {t.heute.vorschlag(AKTION[v.aktion])}
          {v.antwortcode && <span className="ml-1">{v.antwortcode}</span>}
          {v.aktion === "neu_buchen" && v.buchungsdatum && (
            <span className="ml-1">
              {t.heute.am(formatBusinessDate(v.buchungsdatum as BusinessDate))}
            </span>
          )}
          {befund && <span className="ml-1 font-normal">{t.heute.wegen(befund)}</span>}
          {" — "}
        </span>
        <span>{v.begruendung}</span>
        {!v.belegt && (
          <span className="ml-2 inline-flex items-center gap-1 align-middle text-caption font-semibold text-info">
            <Icon name="warn" size={11} />
            {t.heute.nichtBelegt}
          </span>
        )}
      </span>
      {/* The key says what it does: U takes the proposal into the decision. */}
      <span className="flex shrink-0 items-center gap-1.5 self-center text-callout text-text-2">
        <Kbd>U</Kbd>
        {t.heute.takeIn}
      </span>
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
  warum,
  onWarum,
  index,
  onFocus,
  onOeffnen,
  onEntschieden,
  zeilen,
}: {
  e: Eintrag;
  hinweise: Hinweis[];
  /** The proposal waiting on a Freigabe, Disput or geparkte Buchung row. */
  vorschlag: Answer | undefined;
  /** `U` opened the decision: take the proposal into it. */
  sofort: boolean;
  aktiv: boolean;
  /** The item's decision is open under it. */
  offen: boolean;
  /** „Warum?" is answered under it. */
  warum: boolean;
  /** Ask „Warum?" — absent where the item does not offer it. */
  onWarum: (() => void) | null;
  index: number;
  onFocus: (i: number) => void;
  onOeffnen: (id: string | null) => void;
  onEntschieden: () => void;
  zeilen: RefObject<(HTMLButtonElement | null)[]>;
}) {
  const t = useT();
  const router = useRouter();
  const ai = e.quelle === "hinweise";
  const ding = entscheidbar(e);
  const anker = warumAnker(e);
  return (
    <li data-quelle={e.quelle} className="border-b border-separator last:border-b-0">
      <div
        data-aktiv={aktiv}
        className="group relative flex items-start hover:bg-fill data-[aktiv=true]:bg-[color-mix(in_srgb,var(--accent)_9%,transparent)]"
      >
        {/* The row in hand: a bar in the accent, not only a fill. */}
        <span
          aria-hidden="true"
          className="absolute inset-y-1.5 left-0 w-[3px] rounded-r-full bg-accent opacity-0 group-data-[aktiv=true]:opacity-100"
        />
        <button
          type="button"
          ref={(el) => {
            zeilen.current[index] = el;
          }}
          tabIndex={aktiv ? 0 : -1}
          aria-expanded={ding ? offen : undefined}
          onFocus={() => onFocus(index)}
          onClick={() => {
            // A Freigabe, a Disput, a Prüfposten and a geparkte Buchung are decided here;
            // everything else opens its view.
            if (ding) onOeffnen(offen ? null : e.id);
            else if (e.ziel) router.navigate(e.ziel);
          }}
          className="focus-ring flex min-w-0 flex-1 cursor-default items-start gap-4 border-0 bg-transparent py-2.5 pl-5 text-left outline-none"
        >
          {/* Task first, the subject under it — Mail's sender and subject. */}
          <span className="min-w-0 flex-1">
            <span className="flex items-center gap-1.5 text-headline text-text">
              {ai && <Icon name="sparkle" size={12} className="shrink-0 text-ai" />}
              <span className="truncate">{ai ? e.titel : e.grund}</span>
            </span>
            <span className="mt-0.5 block text-body text-text-2">
              {ai ? e.grund : [e.titel, e.detail].filter(Boolean).join(" · ")}
            </span>
          </span>
          {/* The trailing column has one width in every row, so the Fristen align. */}
          <span className="flex w-32 shrink-0 flex-col items-end gap-1 text-right">
            {e.frist ? (
              <FristUhr due={e.frist} />
            ) : (
              <span className="text-body text-text-2">{t.heute.keineFrist}</span>
            )}
            {ding && <Entscheidungsstand ding={ding} fehlschlag={!offen} />}
            {ding && (
              <span className="text-callout text-accent">
                {offen ? t.heute.schliessen : t.heute.entscheiden}
              </span>
            )}
          </span>
        </button>
        {/* Every row keeps the slot, so „Warum?" never moves the Frist column. */}
        <span className="mt-2 mr-3 ml-2 flex w-20 shrink-0 justify-end">
          {onWarum && (
            <AriaButton
              onPress={onWarum}
              excludeFromTabOrder={!aktiv}
              aria-expanded={warum}
              aria-keyshortcuts="?"
              className={`focus-ring inline-flex h-6 items-center gap-1 rounded-full px-2 text-caption text-ai transition-opacity data-[hovered]:bg-ai-fill ${warum ? "bg-ai-fill opacity-100" : "opacity-0 group-hover:opacity-100 group-data-[aktiv=true]:opacity-100 data-[focus-visible]:opacity-100"}`}
            >
              <Icon name="sparkle" size={11} />
              {t.heute.warum}
            </AriaButton>
          )}
        </span>
      </div>
      {warum && anker && (
        <div className="px-5 pb-3">
          <Suspense fallback={<div className="skeleton h-12" />}>
            <Warum anker={anker} />
          </Suspense>
        </div>
      )}
      {vorschlag && !offen && (
        <div className="px-5 pb-3">
          <VorschlagZeile
            a={vorschlag}
            befund={
              e.disput?.befunde.find((b) => b.art === vorschlag.vorschlag?.befund)?.bezeichnung
            }
          />
        </div>
      )}
      {hinweise.length > 0 && (
        <div className="flex flex-col gap-2 px-5 pb-3">
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
      {offen && ding && (
        <div className="flex flex-col gap-3 border-t border-separator bg-fill/40 px-5 py-4">
          {e.freigabe && <Entscheiden f={e.freigabe} onDone={onEntschieden} sofort={sofort} />}
          {e.disput && <DisputEntscheiden d={e.disput} onDone={onEntschieden} sofort={sofort} />}
          {e.pruefposten && <PruefpostenEntscheiden r={e.pruefposten} onDone={onEntschieden} />}
          {e.gehalten && <GehaltenEntscheiden g={e.gehalten} onDone={onEntschieden} />}
          {e.geparkt && (
            <GeparktEntscheiden b={e.geparkt} onDone={onEntschieden} vorschlag={vorschlag} />
          )}
        </div>
      )}
    </li>
  );
}

/** Where a source's row opens: its own view where the Desk has one, else Heute narrowed to it. */
function zielVon(quelle: QuelleId) {
  if (quelle === "freigaben") return { to: "/freigaben" as const, search: {} };
  if (quelle === "zahlungen" || quelle === "guthaben")
    return { to: "/zahlungen" as const, search: {} };
  return { to: "/heute" as const, search: { quelle } };
}

/**
 * The Arbeitsvorrat: one row per source the Rolle reads, with its count and
 * the oldest item it states — a zero stays a row, and a source that did not
 * answer says so instead of a number. The oldest is named only while the page
 * is the whole list; a capped page says that instead.
 */
function Arbeitsvorrat({ staende, agent }: { staende: Stand[]; agent: string | null }) {
  const t = useT();
  const k = t.heute;
  return (
    <section aria-label={k.arbeitsvorrat}>
      <Panel className="overflow-hidden">
        <ul aria-label={k.arbeitsvorrat} className="m-0 list-none p-0">
          {staende.map((s) => {
            const seit = s.eintraege.flatMap((e) => (e.seit ? [e.seit] : []));
            const aelteste = seit.sort((a, b) => epochMs(a) - epochMs(b))[0];
            const n = s.eintraege.length;
            const zeile = s.fehler
              ? s.fehler
              : s.laedt
                ? k.quelleLaedt
                : n === 0
                  ? k.nichtsOffen
                  : s.mehr
                    ? k.mehrAlsSeite
                    : aelteste
                      ? k.aeltesteVom(formatInstantDay(aelteste))
                      : null;
            return (
              <li key={s.quelle} className="border-b border-separator last:border-b-0">
                <Link
                  {...zielVon(s.quelle)}
                  className="focus-ring flex items-center gap-4 px-5 py-2.5 text-text no-underline hover:bg-fill"
                >
                  <span className="min-w-0 flex-1">
                    <span className="block text-headline">{s.label}</span>
                    {zeile && (
                      <span
                        className={`block text-caption ${s.fehler ? "text-critical" : "text-text-2"}`}
                      >
                        {zeile}
                      </span>
                    )}
                  </span>
                  {!s.fehler && !s.laedt && (
                    <span
                      className={`text-title2 font-semibold tabular-nums ${n === 0 ? "text-text-2" : "text-text"}`}
                    >
                      {formatCount(n, s.mehr)}
                    </span>
                  )}
                  {s.fehler && <Icon name="warn" size={16} className="text-critical" />}
                  <Icon name="chevron" size={14} className="text-text-2" />
                </Link>
              </li>
            );
          })}
        </ul>
      </Panel>
      {agent && <p className="m-0 mt-2 px-1 text-caption text-text-2">{k.assistentFehlt}</p>}
    </section>
  );
}

/**
 * Heute: everything that waits on a person in the Rolle worn, one list in
 * Frist bands. The agent's Hinweise sit under the item they concern; `?`
 * asks the assistant why the selected item waits and answers beside it, `⌘J`
 * asks it anything about the item in focus.
 */
export function HeuteView() {
  const t = useT();
  const k = t.heute;
  const { rolle } = useRolle();
  const { quelle: nur } = useSearch({ from: "/heute" });
  const staende = useQuellen(rolle);
  const { hinweise, fehler: agentFehler } = useHinweise();
  const now = useNow();
  const gemischt = useMemo(() => zusammenfuehren(staende, hinweise), [staende, hinweise]);
  const eintraege = useBenannt(gemischt.eintraege, rolle === "lieferant");
  const { zu } = gemischt;
  const [aktiv, setAktiv] = useState(0);
  const [offen, setOffen] = useState<string | null>(null);
  const [sofort, setSofort] = useState<string | null>(null);
  const [warum, setWarum] = useState<string | null>(null);
  // A row the person has focused is what ⌘K opens on; before that, none is.
  const [gefokust, setGefokust] = useState(false);
  const client = useQueryClient();
  const zeilen = useRef<(HTMLButtonElement | null)[]>([]);
  const sichtbar = eintraege.filter((e) => !nur || e.quelle === nur).slice(0, SICHTBAR);
  const laedt = staende.some((s) => s.laedt) && eintraege.length === 0;
  const vorschlaege = useVorschlaege({
    freigabe: sichtbar.flatMap((e) => (e.freigabe ? [e.freigabe.id] : [])),
    disput: sichtbar.flatMap((e) => (e.disput ? [e.disput.id] : [])),
    buchung: sichtbar.flatMap((e) => (e.geparkt ? [e.geparkt.key] : [])),
  });
  const wartend = (e: Eintrag | undefined) =>
    e?.freigabe
      ? vorschlaege.freigabe.get(e.freigabe.id)
      : e?.disput
        ? vorschlaege.disput.get(e.disput.id)
        : e?.geparkt
          ? vorschlaege.buchung.get(e.geparkt.key)
          : undefined;
  // „Warum?" needs the assistant: while agentd is unreachable it is not offered.
  const fragbar = (e: Eintrag) => !agentFehler && warumAnker(e) !== null;

  const gruppen = BAENDER.map(({ band: b, titel }) => ({
    b,
    titel,
    zeilen: sichtbar.map((e, i) => ({ e, i })).filter(({ e }) => band(e.frist, now) === b),
  })).filter((g) => g.zeilen.length > 0);

  // J/K and the arrows step through every band as one list.
  useEffect(() => {
    const onKey = (ev: KeyboardEvent) => {
      // The arrows move only from a row; J and K from anywhere but a field.
      const pfeil = ev.key === "ArrowDown" || ev.key === "ArrowUp";
      if (pfeil && !zeilen.current.some((z) => z === document.activeElement)) return;
      // U on a Freigabe or Disput row opens its decision with the proposal
      // taken in; once the decision is open, the form answers U itself.
      const fokussiert = sichtbar[aktiv];
      // ? asks why the selected item waits, and asks it away again.
      if (WARUM.match?.(ev) && fokussiert && fragbar(fokussiert)) {
        ev.preventDefault();
        setWarum(warum === fokussiert.id ? null : fokussiert.id);
        return;
      }
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
  const imBlick = gefokust && fokus ? gefunden(fokus) : null;
  useEffect(() => fokussieren(imBlick));
  useEffect(() => () => fokussieren(null), []);

  return (
    <Page width="reading" title={k.titel}>
      {rolle === "lieferant" && (
        <Suspense fallback={null}>
          <Lage />
        </Suspense>
      )}

      <Arbeitsvorrat staende={staende} agent={agentFehler} />
      {nur && (
        <p className="m-0 flex items-center gap-3 px-1 text-body text-text-2">
          <span>{k.nurQuelle(staende.find((s) => s.quelle === nur)?.label ?? nur)}</span>
          <Link
            to="/heute"
            search={{}}
            className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
          >
            {k.alleQuellen}
          </Link>
        </p>
      )}

      {laedt && (
        <Panel className="flex flex-col gap-3 p-5">
          <span className="sr-only">{k.laedt}</span>
          <div className="skeleton h-4 w-2/3" />
          <div className="skeleton h-4 w-1/2" />
          <div className="skeleton h-4 w-3/5" />
        </Panel>
      )}

      {!laedt && eintraege.length === 0 && (
        <Panel>
          <EmptyState glyph="check" title={k.erledigt}>
            {k.erledigtText}
          </EmptyState>
        </Panel>
      )}

      {gruppen.map((g) => (
        <section key={g.b} aria-labelledby={`band-${g.b}`} className="flex flex-col gap-2">
          <h2
            id={`band-${g.b}`}
            className={`sticky top-(--topbar) z-[1] m-0 -mx-1 flex items-baseline gap-2 bg-bg/90 px-2 py-1.5 text-headline backdrop-blur-sm ${g.b === "ueberfaellig" ? "text-critical" : ""}`}
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
                  warum={warum === e.id && fragbar(e)}
                  onWarum={fragbar(e) ? () => setWarum(warum === e.id ? null : e.id) : null}
                  index={i}
                  onFocus={(i) => {
                    setAktiv(i);
                    setGefokust(true);
                  }}
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
          {k.ersten(formatCount(SICHTBAR), formatCount(eintraege.length))}
        </p>
      )}

      {fokus?.anker && !offen && (
        <section aria-label={k.fragenZu(fokus.titel)} className="flex flex-col gap-2">
          <h2 className="m-0 px-1 text-headline">
            {k.fragenZuKopf} <span>{fokus.titel}</span>
          </h2>
          <Fragen key={JSON.stringify(fokus.anker)} anker={fokus.anker} />
        </section>
      )}

      <p className="m-0 px-1 text-caption text-text-2">{k.hilfe}</p>
    </Page>
  );
}
