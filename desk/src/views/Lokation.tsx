import { useQuery } from "@tanstack/react-query";
import { Link, useParams, useSearch } from "@tanstack/react-router";
import { useEffect, useId, useRef } from "react";
import type uPlot from "uplot";
import { nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import { lesen, RestAbgelehnt } from "../data/api/client";
import type {
  AccountingdKontoauszug,
  AccountingdKontostand,
  EdmdAbdeckung,
  EdmdMesswerte,
  MarktdLokationsbuendel,
  MarktdMaloResponse,
  MarktdObjekttyp,
  MarktdVersorgungsStatusHistoryResponse,
} from "../data/api/schema";
import { ServiceUnavailable } from "../data/mcp";
import { Page } from "../design/Page";
import { Notice, Panel } from "../design/Panel";
import type { BusinessDate, Instant } from "../format/brands";
import {
  alsAchsenzeit,
  berlinAchsenzeit,
  berlinMitternacht,
  formatBusinessDate,
  formatInstant,
  formatInstantDay,
  formatTagKurz,
  heuteBerlin,
  tageVerschoben,
  tageZwischen,
} from "../format/date";
import { alsMesswert, formatCent, formatDecimal } from "../format/money";
import { eintrag, useT } from "../i18n";

// The windows the page looks at. A view's default, not a Frist: the services
// answer for the window asked, and nothing here decides from it.
const TAGE_MESSWERTE = 30;
const TAGE_LASTGANG = 7;

/** A panel's failure, in the service's words: unreachable, or its refusal. */
function Fehler({
  error,
  dienst,
}: {
  error: unknown;
  dienst: Parameters<typeof nichtErreichbar>[0];
}) {
  const t = useT();
  return (
    <Notice
      tone={error instanceof RestAbgelehnt ? "refusal" : "failure"}
      title={error instanceof ServiceUnavailable ? nichtErreichbar(dienst) : t.lokation.abgelehnt}
    >
      {error instanceof Error ? error.message : String(error)}
    </Notice>
  );
}

function PanelKopf({ id, titel, rechts }: { id: string; titel: string; rechts?: React.ReactNode }) {
  return (
    <div className="flex items-baseline gap-3 px-6 pt-5 pb-3">
      <h2 id={id} className="m-0 flex-1 text-headline">
        {titel}
      </h2>
      {rechts}
    </div>
  );
}

// ── Versorgung ──────────────────────────────────────────────────────────────

/** The status each band is drawn in: supplied, a fallback supply, or none. */
const BAND: Record<string, string> = {
  Beliefert: "bg-[color-mix(in_srgb,var(--accent)_16%,var(--surface))] border-accent text-text",
  Grundversorgung: "bg-info-fill border-info text-text",
  Ersatzversorgung: "bg-soon-fill border-soon text-text",
};

function Versorgung({ items }: { items: MarktdVersorgungsStatusHistoryResponse[] }) {
  const t = useT();
  const k = t.lokation;
  const id = useId();
  // marktd's history, the newest first as it pages; each state holds from its
  // valid_from to the next one's — half-open, as marktd keeps it.
  const chronologisch = [...items].reverse();
  return (
    <Panel className="overflow-hidden">
      <section aria-labelledby={id}>
        <PanelKopf id={id} titel={k.versorgung} />
        {chronologisch.length === 0 ? (
          <p className="m-0 px-6 pb-5 text-body text-text-2">{k.keineVersorgung}</p>
        ) : (
          <ol
            aria-label={k.versorgung}
            className="m-0 flex list-none gap-1 overflow-x-auto px-6 pb-5"
          >
            {chronologisch.map((v, i) => {
              const bis = chronologisch[i + 1]?.valid_from ?? null;
              // Drawn to scale: a band as wide as the days it held, the open
              // one up to now — for drawing only, no Frist is read off it.
              const tage = tageZwischen(v.valid_from, bis);
              const angekuendigt = v.zuordnungen.some((z) => z.status === "Angekuendigt");
              return (
                <li
                  key={v.id}
                  data-status={v.lieferstatus}
                  style={{ flexGrow: Math.max(tage, 1) }}
                  className={`flex min-w-32 basis-0 flex-col gap-1 rounded-[10px] border-l-4 px-3 py-2 ${
                    BAND[v.lieferstatus] ?? "border-separator bg-fill text-text-2"
                  } ${angekuendigt ? "border-dashed" : ""}`}
                >
                  <span className="text-headline">{eintrag(k.lieferstatus, v.lieferstatus)}</span>
                  <span className="text-caption tabular-nums">
                    {k.seitBis(formatInstantDay(v.valid_from), bis ? formatInstantDay(bis) : null)}
                  </span>
                  {v.zuordnungen.map((z) => (
                    <span
                      key={`${z.lf_mp_id}${z.tranche_id ?? ""}`}
                      className="text-caption text-text-2"
                    >
                      {k.lieferant(z.lf_mp_id)}
                      {z.status === "Angekuendigt" && ` · ${k.angekuendigt}`}
                    </span>
                  ))}
                </li>
              );
            })}
          </ol>
        )}
      </section>
    </Panel>
  );
}

// ── Messkonzept ─────────────────────────────────────────────────────────────

const REIHE: Record<MarktdObjekttyp, number> = { MALO: 0, MELO: 1, NELO: 2, TR: 2, SR: 2 };
const KNOTEN_B = 132;
const KNOTEN_H = 40;
const ABSTAND_X = 24;
const ABSTAND_Y = 64;

/**
 * The Lokationsbündel as marktd resolves it for a day: the Marktlokation on
 * top, its Messlokationen with their Zähler beneath, Netz-, technische and
 * steuerbare Ressourcen on the third row, an edge for every Zuordnung. Laid
 * out by row and order only — no layout library, nothing inferred.
 */
function Messkonzept({
  b,
  strukturen,
  knoten,
}: {
  b: MarktdLokationsbuendel;
  /** marktd's Lokationsbündelstrukturen: the BDEW wording of each code. */
  strukturen: Map<string, string>;
  /** The node the page was opened on (a MeLo), marked. */
  knoten: string | undefined;
}) {
  const t = useT();
  const k = t.lokation;
  const id = useId();
  const reihen = [0, 1, 2].map((r) => b.objekte.filter((o) => REIHE[o.typ] === r));
  const breit = Math.max(...reihen.map((r) => r.length), 1);
  const breite = breit * (KNOTEN_B + ABSTAND_X) + ABSTAND_X;
  const zaehlerZeilen = (melo: string) => b.zaehler.filter((z) => z.melo_id === melo);
  const hoehe = 3 * (KNOTEN_H + ABSTAND_Y) + 8;
  const ort = new Map<string, { x: number; y: number }>();
  reihen.forEach((reihe, r) => {
    const versatz = (breite - reihe.length * (KNOTEN_B + ABSTAND_X) + ABSTAND_X) / 2;
    reihe.forEach((o, i) => {
      ort.set(o.id, { x: versatz + i * (KNOTEN_B + ABSTAND_X), y: 8 + r * (KNOTEN_H + ABSTAND_Y) });
    });
  });
  return (
    <Panel className="overflow-hidden">
      <section aria-labelledby={id}>
        <PanelKopf
          id={id}
          titel={k.messkonzept}
          rechts={
            <span className="text-caption text-text-2">
              {k.standZu(formatBusinessDate(b.stichtag as BusinessDate))}
              {b.lokationsbuendelcodes.length > 0 &&
                ` · ${b.lokationsbuendelcodes.map((c) => (strukturen.get(c) ? `${c} ${strukturen.get(c)}` : c)).join(", ")}`}
            </span>
          }
        />
        {b.lokationsbuendelcodes.length > 1 && (
          <p className="m-0 px-6 pb-2 text-caption text-soon">{k.widerspruch}</p>
        )}
        <div className="flex justify-center-safe overflow-x-auto px-6 pb-5">
          <svg
            role="img"
            aria-labelledby={id}
            viewBox={`0 0 ${breite} ${hoehe}`}
            width={breite}
            height={hoehe}
            className="block h-auto shrink-0"
          >
            <title>{k.messkonzept}</title>
            {b.zuordnungen.map((e) => {
              const a = ort.get(e.von_id);
              const z = ort.get(e.nach_id);
              if (!a || !z) return null;
              return (
                <line
                  key={`${e.von_id}-${e.nach_id}`}
                  x1={a.x + KNOTEN_B / 2}
                  y1={a.y + KNOTEN_H}
                  x2={z.x + KNOTEN_B / 2}
                  y2={z.y}
                  className="stroke-separator"
                  strokeWidth={1.5}
                />
              );
            })}
            {b.objekte.map((o) => {
              const p = ort.get(o.id);
              if (!p) return null;
              const zaehler = o.typ === "MELO" ? zaehlerZeilen(o.id) : [];
              return (
                <g
                  key={o.id}
                  data-typ={o.typ}
                  {...(o.id === knoten ? { "aria-current": "location" as const } : {})}
                >
                  <rect
                    x={p.x}
                    y={p.y}
                    width={KNOTEN_B}
                    height={KNOTEN_H}
                    rx={10}
                    className={
                      o.typ === "MALO" || o.id === knoten
                        ? "fill-[color-mix(in_srgb,var(--accent)_14%,var(--surface))] stroke-accent"
                        : "fill-surface stroke-separator"
                    }
                    strokeWidth={o.id === knoten ? 3 : 1.5}
                  />
                  <text x={p.x + 10} y={p.y + 16} className="fill-text-2 text-caption">
                    {eintrag(k.objekttyp, o.typ)}
                  </text>
                  <text x={p.x + 10} y={p.y + 31} className="fill-text text-callout font-semibold">
                    {o.id.length > 15 ? `…${o.id.slice(-14)}` : o.id}
                  </text>
                  {zaehler.map((z, i) => (
                    <text
                      key={z.zaehler_id}
                      x={p.x + 10}
                      y={p.y + KNOTEN_H + 14 + i * 12}
                      className="fill-text-2 text-caption"
                    >
                      {k.zaehler(
                        z.zaehler_id.length > 14 ? `…${z.zaehler_id.slice(-13)}` : z.zaehler_id,
                      )}
                    </text>
                  ))}
                </g>
              );
            })}
          </svg>
        </div>
      </section>
    </Panel>
  );
}

// ── Messwerte ───────────────────────────────────────────────────────────────

const ZUSTAND: Record<string, string> = {
  gemessen: "bg-done",
  ersatzwert: "bg-soon",
  korrigiert: "bg-info",
  fehlt: "bg-critical",
};

/** One cell per day, as edmd states each day: measured, substituted, corrected or missing. */
function Abdeckung({ a }: { a: EdmdAbdeckung }) {
  const t = useT();
  const k = t.lokation;
  return (
    <div className="flex flex-col gap-2">
      <ol aria-label={k.abdeckung} className="m-0 flex list-none gap-[3px] p-0">
        {a.tage.map((tag) => (
          <li
            key={tag.tag}
            data-zustand={tag.zustand}
            title={`${formatTagKurz(tag.tag as BusinessDate)} · ${eintrag(k.tageszustand, tag.zustand)}`}
            className={`h-7 min-w-[6px] flex-1 rounded-[3px] ${ZUSTAND[tag.zustand] ?? "bg-fill-strong"} ${tag.zustand === "gemessen" ? "opacity-60" : ""}`}
          >
            <span className="sr-only">
              {formatTagKurz(tag.tag as BusinessDate)} {eintrag(k.tageszustand, tag.zustand)}
            </span>
          </li>
        ))}
      </ol>
      <p className="m-0 flex flex-wrap gap-x-4 gap-y-1 text-caption text-text-2">
        {(["gemessen", "ersatzwert", "korrigiert", "fehlt"] as const).map((z) => (
          <span key={z} className="inline-flex items-center gap-1.5">
            <span
              aria-hidden="true"
              className={`inline-block size-2.5 rounded-[2px] ${ZUSTAND[z]}`}
            />
            {k.tage(eintrag(k.tageszustand, z), `${a[z]}`)}
          </span>
        ))}
      </p>
    </div>
  );
}

/**
 * The Lastgang of the last days, in uPlot. The kWh values are edmd's decimal
 * strings, turned into numbers for drawing only (`alsMesswert`). A browser
 * without a canvas (a test) draws nothing and says so.
 */
function Lastgang({ m }: { m: EdmdMesswerte }) {
  const t = useT();
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el || m.readings.length === 0) return;
    const x = m.readings.map((r) => alsAchsenzeit(r.dtm_from as Instant));
    const y = m.readings.map((r) => alsMesswert(r.quantity_kwh));
    const farbe = getComputedStyle(el).getPropertyValue("--accent").trim() || "#0b6e66";
    let chart: uPlot | null = null;
    let weg = false;
    // uPlot is loaded with the chart, not with the page: it reads the window
    // as it loads, and only this panel draws with it.
    void Promise.all([import("uplot"), import("uplot/dist/uPlot.min.css")])
      .then(([{ default: UPlot }]) => {
        if (weg) return;
        chart = new UPlot(
          {
            width: el.clientWidth || 640,
            height: 180,
            legend: { show: false },
            cursor: { y: false },
            scales: { x: { time: true } },
            axes: [{ stroke: "currentColor" }, { stroke: "currentColor", size: 56 }],
            series: [{}, { stroke: farbe, width: 1.5, fill: `${farbe}22` }],
            tzDate: (ts) => berlinAchsenzeit(ts, UPlot.tzDate),
          },
          [x, y],
          el,
        );
      })
      .catch(() => {
        // No canvas or no window here (a test): the ribbon above still says it all.
      });
    return () => {
      weg = true;
      chart?.destroy();
    };
  }, [m]);
  if (m.readings.length === 0)
    return <p className="m-0 text-body text-text-2">{t.lokation.keineMesswerte}</p>;
  return (
    <div ref={ref} aria-label={t.lokation.lastgang} role="img" className="w-full text-text-2" />
  );
}

// ── Konto ───────────────────────────────────────────────────────────────────

function Konto({
  stand,
  auszug,
}: {
  stand: AccountingdKontostand;
  auszug: AccountingdKontoauszug;
}) {
  const t = useT();
  const k = t.lokation;
  const id = useId();
  return (
    <Panel className="overflow-hidden">
      <section aria-labelledby={id}>
        <PanelKopf
          id={id}
          titel={k.konto}
          rechts={
            <span
              className={`text-title2 font-semibold tabular-nums ${stand.status === "overdue" ? "text-critical" : stand.status === "credit" ? "text-done" : "text-text"}`}
            >
              {formatCent(stand.balance_ct)}
            </span>
          }
        />
        <p className="m-0 px-6 pb-3 text-caption text-text-2">
          {eintrag(k.saldo, stand.status)} · {k.abschlag(formatCent(stand.abschlag_ct))}
        </p>
        <ol aria-label={k.kontoauszug} className="m-0 list-none border-t border-separator p-0">
          {auszug.lines.map((l) => (
            <li
              key={l.entry_id}
              className="grid grid-cols-[6rem_minmax(0,1fr)_7rem_7rem] items-baseline gap-3 border-b border-separator px-6 py-2 text-body last:border-b-0"
            >
              <span className="text-caption text-text-2 tabular-nums">
                {formatBusinessDate(l.booking_date as BusinessDate)}
              </span>
              <span className="min-w-0 truncate">
                {l.entry_type ? eintrag(t.buchungsarten.art, l.entry_type) : k.buchung}
                {l.beleg && <span className="ml-2 text-caption text-text-2">{l.beleg}</span>}
              </span>
              <span className="text-right tabular-nums">{formatCent(l.signed_ct)}</span>
              <span className="text-right text-text-2 tabular-nums">
                {formatCent(l.running_ct)}
              </span>
            </li>
          ))}
        </ol>
      </section>
    </Panel>
  );
}

/**
 * A Marktlokation's Konto as accountingd keeps it — the balance, then the
 * statement with its running balance, the newest first. The Lokation page
 * and the Kunde view (one per Lieferstelle) both show it.
 */
export function KontoPanel({ malo }: { malo: string }) {
  const { rolle } = useRolle();
  const id = malo;
  const stand = useQuery({
    queryKey: [rolle, "rest", "accountingd", "balance", id],
    queryFn: () =>
      lesen("accountingd", "/svc/accountingd/api/v1/accounts/{malo_id}/balance", {
        path: { malo_id: id },
      }),
  });
  const auszug = useQuery({
    queryKey: [rolle, "rest", "accountingd", "ledger", id],
    queryFn: () =>
      lesen("accountingd", "/svc/accountingd/api/v1/accounts/{malo_id}/ledger", {
        path: { malo_id: id },
        query: { limit: 20 },
      }),
  });
  if (stand.isError || auszug.isError)
    return <Fehler error={stand.error ?? auszug.error} dienst="accountingd" />;
  if (!stand.data || !auszug.data) return <div className="skeleton h-32" />;
  return <Konto stand={stand.data} auszug={auszug.data} />;
}

/**
 * The Lokation page: the hub a MaLo opens onto, read over the services' REST
 * API — the Versorgung from marktd's history, the Messkonzept from its
 * Lokationsbündel, the Messwerte from edmd (completeness first, then the
 * Lastgang), the Konto from accountingd. Each panel answers or refuses on its
 * own; one unreachable service never empties the page.
 */
export function LokationView() {
  const t = useT();
  const k = t.lokation;
  const { id } = useParams({ from: "/lokation/$id" });
  const { knoten } = useSearch({ from: "/lokation/$id" });
  const { rolle } = useRolle();
  const heute = heuteBerlin();
  const malo = useQuery({
    queryKey: [rolle, "rest", "marktd", "malo", id],
    queryFn: () => lesen("marktd", "/svc/marktd/api/v1/malos/{id}", { path: { id } }),
  });
  const versorgung = useQuery({
    queryKey: [rolle, "rest", "marktd", "versorgung", id],
    queryFn: () =>
      lesen("marktd", "/svc/marktd/api/v1/versorgung/{malo_id}/history", { path: { malo_id: id } }),
  });
  const buendel = useQuery({
    queryKey: [rolle, "rest", "marktd", "lokationsbuendel", id, heute],
    queryFn: () =>
      lesen("marktd", "/svc/marktd/api/v1/malos/{id}/lokationsbuendel", {
        path: { id },
        query: { stichtag: heute },
      }),
  });
  const abdeckung = useQuery({
    queryKey: [rolle, "rest", "edmd", "coverage", id, heute],
    queryFn: () =>
      lesen("edmd", "/svc/edmd/api/v1/coverage/{malo_id}", {
        path: { malo_id: id },
        query: { from: tageVerschoben(heute, -(TAGE_MESSWERTE - 1)), to: heute },
      }),
  });
  const lastgang = useQuery({
    queryKey: [rolle, "rest", "edmd", "timeseries", id, heute],
    queryFn: () =>
      lesen("edmd", "/svc/edmd/api/v1/timeseries/{malo_id}", {
        path: { malo_id: id },
        query: {
          from: berlinMitternacht(tageVerschoben(heute, -TAGE_LASTGANG)),
          to: berlinMitternacht(tageVerschoben(heute, 1)),
        },
      }),
  });
  // The generating side of the bündel: einsd's plants on this MaLo, each opening its Anlage page.
  const erzeugung = useQuery({
    queryKey: [rolle, "rest", "einsd", "anlagen", id],
    queryFn: () => lesen("einsd", "/svc/einsd/api/v1/anlagen", { query: { malo_id: id } }),
  });
  const strukturen = useQuery({
    queryKey: [rolle, "rest", "marktd", "lokationsbuendelstrukturen"],
    queryFn: () => lesen("marktd", "/svc/marktd/api/v1/lokationsbuendelstrukturen", {}),
    staleTime: Number.POSITIVE_INFINITY,
  });
  const qualitaet = useQuery({
    queryKey: [rolle, "rest", "edmd", "quality", id, heute],
    queryFn: () =>
      lesen("edmd", "/svc/edmd/api/v1/quality/{malo_id}", {
        path: { malo_id: id },
        query: {
          from: berlinMitternacht(tageVerschoben(heute, -(TAGE_MESSWERTE - 1))),
          to: berlinMitternacht(tageVerschoben(heute, 1)),
        },
      }),
  });
  const korrekturen = useQuery({
    queryKey: [rolle, "rest", "edmd", "corrections", id],
    queryFn: () =>
      lesen("edmd", "/svc/edmd/api/v1/corrections/{malo_id}", {
        path: { malo_id: id },
        query: { limit: 10 },
      }),
  });
  const m: MarktdMaloResponse | undefined = malo.data;
  const aktuell = versorgung.data?.items[0];
  const messwerteId = useId();
  const fakten = m
    ? [
        eintrag(t.values.sparte, m.sparte),
        m.netzebene,
        m.bilanzierungsmethode,
        m.energierichtung ? eintrag(k.energierichtung, m.energierichtung) : null,
        m.lokationsbuendel_objektcode,
      ].filter((f): f is string => Boolean(f))
    : [];

  return (
    <Page width="work">
      <Panel className="px-6 pt-5 pb-5">
        <nav aria-label={t.prozessAnsicht.pfad} className="text-caption text-text-2">
          <Link
            to="/prozess"
            search={{ key: id }}
            className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
          >
            {t.prozessAnsicht.titel}
          </Link>
          <span aria-hidden="true" className="mx-1.5">
            /
          </span>
          <span>{id}</span>
        </nav>
        <h1 className="m-0 mt-1 text-large-title font-semibold">
          {k.titel(id)}
          {aktuell && (
            <span
              className={`ml-3 inline-flex h-6 items-center rounded-full px-2.5 align-middle text-body font-semibold ${
                aktuell.lieferstatus === "Beliefert"
                  ? "bg-done-fill text-done"
                  : aktuell.lieferstatus === "Unbeliefert"
                    ? "bg-critical-fill text-critical"
                    : "bg-info-fill text-info"
              }`}
            >
              {eintrag(k.lieferstatus, aktuell.lieferstatus)}
            </span>
          )}
        </h1>
        {fakten.length > 0 && (
          <ul
            aria-label={t.prozessAnsicht.eckdaten}
            className="m-0 mt-1.5 flex list-none flex-wrap gap-x-2 p-0 text-body"
          >
            {fakten.map((f, i) => (
              <li key={f}>
                {i > 0 && (
                  <span aria-hidden="true" className="mr-2 text-text-2">
                    ·
                  </span>
                )}
                {f}
              </li>
            ))}
          </ul>
        )}
        {malo.isError && <Fehler error={malo.error} dienst="marktd" />}
      </Panel>

      {versorgung.data && <Versorgung items={versorgung.data.items} />}
      {versorgung.isError && <Fehler error={versorgung.error} dienst="marktd" />}

      <div className="grid grid-cols-1 gap-5 xl:grid-cols-2">
        {buendel.data && (
          <Messkonzept
            b={buendel.data}
            strukturen={
              new Map((strukturen.data?.strukturen ?? []).map((x) => [x.code, x.bezeichnung]))
            }
            knoten={knoten}
          />
        )}
        {buendel.isError && <Fehler error={buendel.error} dienst="marktd" />}

        <Panel className="overflow-hidden">
          <section aria-labelledby={messwerteId}>
            <PanelKopf
              id={messwerteId}
              titel={k.messwerte}
              rechts={
                <span className="text-caption text-text-2">
                  {k.letzteTage(`${TAGE_MESSWERTE}`)}
                </span>
              }
            />
            <div className="flex flex-col gap-5 px-6 pb-5">
              {abdeckung.data && <Abdeckung a={abdeckung.data} />}
              {abdeckung.isError && <Fehler error={abdeckung.error} dienst="edmd" />}
              {lastgang.data && <Lastgang m={lastgang.data} />}
              {lastgang.isError && <Fehler error={lastgang.error} dienst="edmd" />}
              {qualitaet.data?.grade && (
                <p className="m-0 text-caption text-text-2">
                  {k.qualitaet(
                    eintrag(k.note, qualitaet.data.grade),
                    qualitaet.data.coverage_pct,
                    `${qualitaet.data.gaps_detected}`,
                  )}
                </p>
              )}
              {(korrekturen.data?.corrections?.length ?? 0) > 0 && korrekturen.data && (
                <ol aria-label={k.korrekturen} className="m-0 flex list-none flex-col gap-1 p-0">
                  {korrekturen.data.corrections.map((c) => (
                    <li key={c.correction_id} className="text-caption text-text">
                      {k.korrektur(
                        formatInstant(c.dtm_from),
                        c.original_kwh,
                        c.corrected_kwh,
                        c.reason,
                      )}
                    </li>
                  ))}
                </ol>
              )}
            </div>
          </section>
        </Panel>
      </div>

      {(erzeugung.data ?? []).length > 0 && (
        <Panel className="px-6 py-5">
          <h2 className="m-0 mb-2 text-headline">{k.erzeugung}</h2>
          <ul aria-label={k.erzeugung} className="m-0 flex list-none flex-col gap-1 p-0">
            {(erzeugung.data ?? []).map((a) => (
              <li key={a.tr_id} className="text-body">
                <Link
                  to="/anlage/$trId"
                  params={{ trId: a.tr_id }}
                  className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
                >
                  {t.anlage.titel(
                    eintrag(t.anlage.erzeugungsart, a.erzeugungsart),
                    formatDecimal(a.leistung_kwp),
                  )}
                </Link>
                <span className="ml-2 text-caption text-text-2">{a.tr_id}</span>
              </li>
            ))}
          </ul>
        </Panel>
      )}
      {rolle === "lieferant" && <KontoPanel malo={id} />}
    </Page>
  );
}
