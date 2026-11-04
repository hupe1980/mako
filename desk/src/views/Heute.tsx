import { useInfiniteQuery, useQueries, useQuery } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { GridList, GridListItem } from "react-aria-components";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import type { MaloKunde } from "../data/gen/vertragd";
import { call, ServiceUnavailable } from "../data/mcp";
import { Button } from "../design/Button";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { formatCount } from "../format/count";
import { Hinweise } from "./Hinweise";
import { monogram } from "./kundenSuche";
import { ANMELDUNG_PIDS, Lage } from "./Lage";

const PAGE = 50;

const ROW =
  "focus-ring flex cursor-default items-center gap-4 border-b border-separator px-5 py-3 outline-none last:border-b-0 data-[focused]:bg-fill data-[hovered]:bg-fill";

function Person({ name, sub }: { name: string | null | undefined; sub: ReactNode }) {
  return (
    <>
      <span className="flex size-9 shrink-0 items-center justify-center rounded-full bg-[color-mix(in_srgb,var(--accent)_14%,transparent)] text-body font-semibold text-accent">
        {monogram(name)}
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-callout font-medium text-text">
          {name ?? "Ohne Namen"}
        </span>
        <span className="block text-body text-text-2">{sub}</span>
      </span>
    </>
  );
}

/**
 * Waiting on the Netzbetreiber, the earliest Frist first. The order is
 * processd's — a keyset page at a time — so the most urgent Anmeldung is on
 * top however many are open; each page is named in one vertragd call.
 */
function Wartend() {
  const router = useRouter();
  const { rolle } = useRolle();
  const pages = useInfiniteQuery({
    queryKey: [rolle, "processd", "list_open_processes", ANMELDUNG_PIDS],
    queryFn: ({ pageParam }) =>
      call("processd", "list_open_processes", {
        pids: ANMELDUNG_PIDS,
        limit: PAGE,
        ...(pageParam ? { nach: pageParam } : {}),
      }),
    initialPageParam: "",
    getNextPageParam: (last) => last.weiter ?? undefined,
    refetchInterval: 30_000,
  });
  const loaded = pages.data?.pages ?? [];
  const namen = useQueries({
    queries: loaded.map((p) => {
      const malos = p.prozesse.flatMap((r) => (r.malo_id ? [r.malo_id] : []));
      return {
        queryKey: [rolle, "vertragd", "kunden_zu_malos", malos],
        queryFn: () => call("vertragd", "kunden_zu_malos", { malos }),
        enabled: malos.length > 0,
        staleTime: 60_000,
      };
    }),
  });
  const kunde = new Map<string, MaloKunde>();
  for (const n of namen) for (const k of n.data?.kunden ?? []) kunde.set(k.malo_id, k);
  const rows = loaded.flatMap((p) => p.prozesse);

  return (
    <section aria-labelledby="wartend" className="flex flex-col gap-2">
      <h2 id="wartend" className="m-0 px-1 text-callout font-semibold">
        Warten auf Antwort
      </h2>
      {pages.isError && (
        <Notice
          tone="failure"
          title={
            pages.error instanceof ServiceUnavailable
              ? nichtErreichbar(pages.error.service)
              : "Die offenen Anmeldungen ließen sich nicht lesen"
          }
        >
          {pages.error.message}
        </Notice>
      )}
      {pages.data && rows.length === 0 && (
        <Panel>
          <EmptyState title="Alles beantwortet.">
            Keine Anmeldung wartet auf den Netzbetreiber.
          </EmptyState>
        </Panel>
      )}
      {rows.length > 0 && (
        <Panel className="overflow-hidden">
          <GridList
            aria-label="Warten auf Antwort"
            onAction={(key) => {
              const k = kunde.get(String(key));
              if (k) router.navigate({ to: "/kunde/$id", params: { id: k.kunden_id } });
              else router.navigate({ to: "/prozess", search: { key: String(key) } });
            }}
            className="outline-none"
          >
            {rows.map((p) => {
              const k = p.malo_id ? kunde.get(p.malo_id) : undefined;
              const id = p.malo_id ?? p.process_id;
              return (
                <GridListItem key={p.process_id} id={id} textValue={k?.name ?? id} className={ROW}>
                  <Person
                    name={k?.name}
                    sub={
                      <>
                        <span className="font-mono">{id}</span> · PID{" "}
                        <span className="font-mono">{p.pid}</span>
                        {k?.kunden_nr && (
                          <>
                            {" · "}
                            <span className="font-mono">{k.kunden_nr}</span>
                          </>
                        )}
                      </>
                    }
                  />
                  {p.deadline_at ? (
                    <FristUhr due={p.deadline_at} />
                  ) : (
                    <span className="text-body text-text-2">keine Frist</span>
                  )}
                  <Icon name="chevron" size={16} className="text-text-2" />
                </GridListItem>
              );
            })}
          </GridList>
          {pages.hasNextPage && (
            <div className="border-t border-separator px-5 py-3">
              <Button
                variant="ghost"
                isDisabled={pages.isFetchingNextPage}
                onPress={() => pages.fetchNextPage()}
              >
                {pages.isFetchingNextPage ? "Lädt …" : `Nächste ${PAGE}`}
              </Button>
            </div>
          )}
        </Panel>
      )}
      <p className="m-0 px-1 text-caption text-text-2">
        Reihenfolge und Fristen aus der {DIENST.processd} · ↑↓ wählen, ↵ öffnet den Kunden
      </p>
    </section>
  );
}

/** Angemeldet in vertragd, but no makod process carries it yet — so no Frist. */
function NichtUebermittelt() {
  const router = useRouter();
  const { rolle } = useRolle();
  const ohne = useQuery({
    queryKey: [rolle, "vertragd", "list_anmeldungen", "ohne_prozess"],
    queryFn: () => call("vertragd", "list_anmeldungen", { ohne_prozess: true, limit: PAGE }),
    refetchInterval: 60_000,
  });
  if (!ohne.data || ohne.data.gesamt === 0) return null;
  const rows = ohne.data.anmeldungen;
  return (
    <section aria-labelledby="ohne" className="flex flex-col gap-2">
      <h2 id="ohne" className="m-0 px-1 text-callout font-semibold">
        Noch nicht übermittelt{" "}
        <span className="font-normal text-text-2">{formatCount(ohne.data.gesamt)}</span>
      </h2>
      <Panel className="overflow-hidden">
        <GridList
          aria-label="Noch nicht übermittelt"
          onAction={(key) =>
            router.navigate({ to: "/kunde/$id", params: { id: String(key).split("|")[0] ?? "" } })
          }
          className="outline-none"
        >
          {rows.map((a) => (
            <GridListItem
              key={`${a.kunden_id}|${a.malo_id}`}
              id={`${a.kunden_id}|${a.malo_id}`}
              textValue={a.name ?? a.malo_id}
              className={ROW}
            >
              <Person
                name={a.name}
                sub={
                  <>
                    <span className="font-mono">{a.malo_id}</span> · {a.sparte} · Vertrag{" "}
                    <span className="font-mono">{a.vertrags_nr}</span>
                  </>
                }
              />
              <Icon name="chevron" size={16} className="text-text-2" />
            </GridListItem>
          ))}
        </GridList>
        {ohne.data.gesamt > rows.length && (
          <p className="m-0 border-t border-separator px-5 py-3 text-body text-text-2">
            Die ersten {formatCount(rows.length)} von {formatCount(ohne.data.gesamt)}, nach Namen.
          </p>
        )}
      </Panel>
    </section>
  );
}

/** Heute for the Lieferant: what waits on an answer, the next Frist first. */
export function HeuteView() {
  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">Heute</h1>
        <p className="m-0 mt-1 text-callout text-text-2">
          Was auf eine Antwort wartet — die nächste Frist zuerst.
        </p>
      </header>
      <Lage />
      <Hinweise />
      <Wartend />
      <NichtUebermittelt />
    </div>
  );
}
