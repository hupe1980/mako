import { useQuery } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { Button, Dialog, DialogTrigger, Heading, Popover } from "react-aria-components";
import { nichtErreichbar } from "../app/dienste";
import { exportLink } from "../app/gegenpartei";
import { useRolle } from "../app/rollen";
import type { AkteProzess } from "../data/gen/processd";
import { call, ServiceUnavailable } from "../data/mcp";
import { Icon } from "../design/Icon";
import { Page } from "../design/Page";
import { Notice, Panel } from "../design/Panel";
import { Refusal } from "../design/Refusal";
import { SourceChip } from "../design/SourceChip";
import { type Spalte, Table } from "../design/Table";
import { formatProzent } from "../format/bilanz";
import type { BusinessDate, Instant } from "../format/brands";
import { formatCount } from "../format/count";
import { formatBusinessDate, formatInstant } from "../format/date";
import { eintrag, useT } from "../i18n";

/** processd states these instants as RFC 3339 strings without a schema format yet. */
const zeit = (v: string | null) => (v ? formatInstant(v as Instant) : "");

function Zahl({
  wert,
  was,
  ton,
}: {
  wert: string;
  was: string;
  ton?: "critical" | "soon" | undefined;
}) {
  return (
    <li className="flex flex-col gap-1 border-r border-separator px-5 py-4 last:border-r-0">
      <span
        className={`text-title1 font-semibold tabular-nums ${ton === "critical" ? "text-critical" : ton === "soon" ? "text-soon" : "text-text"}`}
      >
        {wert}
      </span>
      <span className="text-caption text-text-2">{was}</span>
    </li>
  );
}

/**
 * A Gegenpartei's record: how its answers arrived against their Fristen over
 * a period, what it refused and with which code, and every process behind a
 * figure — processd's counts, a share only where processd states one (it
 * gives none below its threshold), exportable as evidence (CSV).
 */
export function GegenparteiView() {
  const t = useT();
  const k = t.gegenpartei;
  const { mpId, rolle: marktrolle } = useParams({ from: "/gegenpartei/$mpId/$rolle" });
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "processd", "get_counterparty_record", mpId, marktrolle],
    queryFn: () => call("processd", "get_counterparty_record", { mp_id: mpId, rolle: marktrolle }),
  });
  const namen = useQuery({
    queryKey: [rolle, "marktd", "partner_namen", [mpId]],
    queryFn: () => call("marktd", "partner_namen", { mp_ids: [mpId] }),
    staleTime: 300_000,
  });
  const name = namen.data?.partner?.find((p) => p.mp_id === mpId)?.name ?? mpId;
  const a = q.data;
  // The export's address is built by `exportLink`, from parts it encodes.
  const csv = a ? exportLink(mpId, marktrolle, a.von, a.bis) : undefined;
  const spalten: Spalte<AkteProzess>[] = [
    {
      id: "pid",
      titel: k.pid,
      rowHeader: true,
      zelle: (p) => (
        <Link
          to="/prozess"
          search={{ key: p.process_id }}
          className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
        >
          {p.pid}
        </Link>
      ),
    },
    { id: "kategorie", titel: k.kategorie, zelle: (p) => eintrag(k.kategorien, p.kategorie) },
    { id: "frist", titel: k.frist, zelle: (p) => zeit(p.frist) },
    { id: "beantwortet", titel: k.beantwortet, zelle: (p) => zeit(p.beantwortet_um) },
    {
      id: "verspaetung",
      titel: k.verspaetung,
      className: "text-right tabular-nums",
      zelle: (p) =>
        p.verspaetung_minuten === null ? "" : k.minuten(formatCount(p.verspaetung_minuten)),
    },
    {
      id: "antwort",
      titel: k.antwort,
      zelle: (p) =>
        p.antwort_code ? (
          <span className={p.abgelehnt ? "text-critical" : undefined}>
            {p.antwort_code}
            {p.antwort_codeliste && (
              <span className="ml-1 text-caption text-text-2">{p.antwort_codeliste}</span>
            )}
          </span>
        ) : (
          ""
        ),
    },
  ];
  return (
    <Page
      width="work"
      title={
        <span className="flex flex-wrap items-baseline gap-x-3">
          {name}
          <span className="text-title2 font-normal text-text-2">{k.alsRolle(marktrolle)}</span>
        </span>
      }
      subtitle={
        a
          ? k.zeitraum(
              mpId,
              formatBusinessDate(a.von as BusinessDate),
              formatBusinessDate(a.bis as BusinessDate),
            )
          : undefined
      }
      trailing={
        a && (
          <span className="flex items-center gap-2">
            <DialogTrigger>
              <Button className="focus-ring inline-flex h-7 items-center gap-1 rounded-control px-3 text-body text-text-2 data-[hovered]:bg-fill">
                <Icon name="doc" size={14} />
                {k.begriffe}
              </Button>
              <Popover
                offset={6}
                className="w-96 rounded-panel bg-surface-raised p-4 shadow-popover outline-none"
              >
                <Dialog className="outline-none">
                  <Heading slot="title" className="m-0 text-headline">
                    {k.begriffe}
                  </Heading>
                  <dl className="m-0 mt-2 flex flex-col gap-2">
                    {a.definitionen.map((d) => (
                      <div key={d.begriff}>
                        <dt className="text-body font-semibold">
                          {eintrag(k.kategorien, d.begriff)}
                        </dt>
                        <dd className="m-0 text-body text-text-2">{d.definition}</dd>
                      </div>
                    ))}
                  </dl>
                </Dialog>
              </Popover>
            </DialogTrigger>
            <a
              href={csv}
              download
              className="focus-ring inline-flex h-7 items-center gap-1.5 rounded-control bg-accent px-3 text-body text-on-accent no-underline"
            >
              <Icon name="download" size={14} />
              {k.exportieren}
            </a>
          </span>
        )
      }
    >
      {q.isPending && <div className="skeleton h-40" />}
      {q.isError &&
        (q.error instanceof ServiceUnavailable ? (
          <Notice tone="failure" title={nichtErreichbar("processd")}>
            {q.error.message}
          </Notice>
        ) : (
          <Refusal title={k.keine} raw={q.error.message} />
        ))}
      {a && (
        <>
          <Panel className="overflow-hidden">
            <ul
              aria-label={k.kennzahlen}
              className="m-0 grid list-none grid-cols-2 p-0 md:grid-cols-5"
            >
              <Zahl
                wert={
                  a.quote_fristgerecht === null
                    ? k.vonGezaehlt(formatCount(a.fristgerecht), formatCount(a.gezaehlt))
                    : formatProzent(a.quote_fristgerecht)
                }
                was={
                  a.quote_fristgerecht === null ? k.fristgerecht : k.quote(formatCount(a.gezaehlt))
                }
              />
              <Zahl
                wert={formatCount(a.verspaetet)}
                was={k.kategorien.verspaetet}
                ton={a.verspaetet ? "soon" : undefined}
              />
              <Zahl
                wert={formatCount(a.unbeantwortet)}
                was={k.kategorien.unbeantwortet}
                ton={a.unbeantwortet ? "critical" : undefined}
              />
              <Zahl wert={formatCount(a.offen)} was={k.kategorien.offen} />
              <Zahl
                wert={
                  a.verspaetung.median_minuten === null
                    ? "—"
                    : k.minuten(formatCount(a.verspaetung.median_minuten))
                }
                was={k.median}
              />
            </ul>
          </Panel>
          <Panel className="px-6 py-5">
            <h2 className="m-0 mb-2 text-headline">{k.ablehnungen}</h2>
            {a.ablehnungen.length === 0 ? (
              <p className="m-0 text-body text-text-2">{k.keineAblehnung}</p>
            ) : (
              <ul aria-label={k.ablehnungen} className="m-0 flex list-none flex-col gap-2 p-0">
                {a.ablehnungen.map((x) => (
                  <li
                    key={`${x.codeliste}/${x.code}`}
                    className="flex flex-wrap items-baseline gap-3 text-body"
                  >
                    <span className="font-semibold">{x.code ?? "—"}</span>
                    <span className="min-w-0 flex-1">{x.bedeutung ?? k.ohneWortlaut}</span>
                    {x.codeliste && <SourceChip quelle={{ dokument: x.codeliste }} />}
                    <span className="text-text-2 tabular-nums">{k.mal(formatCount(x.anzahl))}</span>
                  </li>
                ))}
              </ul>
            )}
          </Panel>
          <Panel className="overflow-hidden">
            <Table
              label={k.prozesse}
              spalten={spalten}
              zeilen={a.prozesse}
              zeilenId={(p) => p.process_id}
            />
          </Panel>
          {(a.ausserhalb.ohne_frist > 0 || a.ausserhalb.schuldner_unbestimmt > 0) && (
            <p className="m-0 px-1 text-caption text-text-2">
              {k.ausserhalb(
                formatCount(a.ausserhalb.ohne_frist),
                formatCount(a.ausserhalb.schuldner_unbestimmt),
              )}
            </p>
          )}
        </>
      )}
    </Page>
  );
}

/**
 * Every Gegenpartei processd counted over the period, as it lists them —
 * each row opens its record. processd orders the list; the Desk does not.
 */
export function GegenparteienView() {
  const t = useT();
  const k = t.gegenpartei;
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "processd", "list_counterparties"],
    queryFn: () => call("processd", "list_counterparties", {}),
  });
  const ids = [...new Set((q.data?.gegenparteien ?? []).map((g) => g.mp_id))].sort();
  const namen = useQuery({
    queryKey: [rolle, "marktd", "partner_namen", ids],
    queryFn: () => call("marktd", "partner_namen", { mp_ids: ids }),
    enabled: ids.length > 0,
    staleTime: 300_000,
  });
  const name = new Map((namen.data?.partner ?? []).map((p) => [p.mp_id, p.name]));
  type Zeile = NonNullable<typeof q.data>["gegenparteien"][number];
  const zahl = (n: number) => formatCount(n);
  const spalten: Spalte<Zeile>[] = [
    {
      id: "name",
      titel: k.gegenpartei,
      rowHeader: true,
      zelle: (g) => (
        <Link
          to="/gegenpartei/$mpId/$rolle"
          params={{ mpId: g.mp_id, rolle: g.rolle }}
          className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
        >
          {name.get(g.mp_id) ?? g.mp_id}
        </Link>
      ),
    },
    { id: "rolle", titel: k.rolle, zelle: (g) => g.rolle },
    {
      id: "gezaehlt",
      titel: k.gezaehlt,
      className: "text-right tabular-nums",
      zelle: (g) => zahl(g.gezaehlt),
    },
    {
      id: "verspaetet",
      titel: k.kategorien.verspaetet,
      className: "text-right tabular-nums",
      zelle: (g) => zahl(g.verspaetet),
    },
    {
      id: "unbeantwortet",
      titel: k.kategorien.unbeantwortet,
      className: "text-right tabular-nums",
      zelle: (g) => zahl(g.unbeantwortet),
    },
    {
      id: "ablehnungen",
      titel: k.ablehnungen,
      className: "text-right tabular-nums",
      zelle: (g) => zahl(g.ablehnungen),
    },
  ];
  return (
    <Page
      width="work"
      title={k.liste}
      subtitle={
        q.data
          ? k.listeZeitraum(
              formatBusinessDate(q.data.von as BusinessDate),
              formatBusinessDate(q.data.bis as BusinessDate),
            )
          : undefined
      }
    >
      {q.isPending && <div className="skeleton h-40" />}
      {q.isError && (
        <Notice tone="failure" title={nichtErreichbar("processd")}>
          {q.error.message}
        </Notice>
      )}
      {q.data && (
        <Panel className="overflow-hidden">
          <Table
            label={k.liste}
            spalten={spalten}
            zeilen={q.data.gegenparteien}
            zeilenId={(g) => `${g.mp_id}/${g.rolle}`}
          />
        </Panel>
      )}
    </Page>
  );
}
