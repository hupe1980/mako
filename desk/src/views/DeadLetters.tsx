import { useQuery } from "@tanstack/react-query";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import { call, ServiceUnavailable } from "../data/mcp";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { type Spalte, Table } from "../design/Table";
import { formatCount } from "../format/count";
import { formatInstant } from "../format/date";
import { useT } from "../i18n";

/** The read the Dead-Letters tile and this list share — one request for both. */
export function useDeadLetters() {
  const { rolle } = useRolle();
  return useQuery({
    queryKey: [rolle, "makod", "list_dead_letters"],
    queryFn: () => call("makod", "list_dead_letters", {}),
    refetchInterval: 60_000,
  });
}

/** One dead letter of the page; the page is ordered by the service and replaced whole. */
type Zeile = NonNullable<ReturnType<typeof useDeadLetters>["data"]>["dead_letters"][number];

/**
 * Dead Letters: the messages the Marktkommunikation could not take or could
 * not deliver, the most recent first — each one waits on a person. A table,
 * in the service's order: the Desk sorts no page.
 */
export function DeadLettersView() {
  const t = useT();
  const q = useDeadLetters();
  const rows = q.data?.dead_letters ?? [];
  const d = t.deadLetters;

  const spalten: Spalte<Zeile>[] = [
    {
      id: "zeitpunkt",
      titel: d.spalteZeitpunkt,
      className: "w-44 whitespace-nowrap tabular-nums text-text-2",
      zelle: (z) => (z.rejected_at ? formatInstant(z.rejected_at) : d.zeitpunktUnlesbar),
    },
    {
      id: "was",
      titel: d.spalteWas,
      rowHeader: true,
      className: "text-callout text-text",
      zelle: (z) => z.reason_detail,
    },
    {
      id: "kategorie",
      titel: d.spalteKategorie,
      className: "font-mono text-text-2",
      zelle: (z) => z.reason_label,
    },
  ];

  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-6 px-4 py-6 md:px-8 md:py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">{d.titel}</h1>
        <p className="m-0 mt-1 text-callout text-text-2">{d.untertitel}</p>
      </header>

      {q.isError && (
        <Notice
          tone="failure"
          title={
            q.error instanceof ServiceUnavailable ? nichtErreichbar(q.error.service) : d.fehler
          }
        >
          {q.error.message}
        </Notice>
      )}

      {q.data && rows.length === 0 && (
        <Panel>
          <EmptyState title={d.leer}>{d.leerText}</EmptyState>
        </Panel>
      )}

      {rows.length > 0 && (
        <Panel className="overflow-x-auto">
          <Table
            label={d.tabelle}
            spalten={spalten}
            zeilen={rows}
            // A dead letter carries no id of its own; the page is replaced
            // whole on each read, so its position names it.
            zeilenId={(_, i) => `${i}`}
          />
          {q.data && q.data.gesamt > rows.length && (
            <p className="m-0 border-t border-separator px-5 py-3 text-body text-text-2">
              {d.juengste(formatCount(rows.length), formatCount(q.data.gesamt), DIENST.makod)}
            </p>
          )}
        </Panel>
      )}
    </div>
  );
}
