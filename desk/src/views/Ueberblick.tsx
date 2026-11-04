import { useQuery } from "@tanstack/react-query";
import { DIENST } from "../app/dienste";
import { ROLLEN, useRolle } from "../app/rollen";
import { call, type Service, ServiceUnavailable } from "../data/mcp";
import { Tile, type TileState } from "../design/Tile";
import { Hinweise } from "./Hinweise";

/** A tile's state from its query: the service's number, or why there is none. */
function tileState<T>(
  q: { data?: T; isError: boolean; error: Error | null },
  service: Service,
  read: (data: NonNullable<T>) => Omit<Extract<TileState, { kind: "count" }>, "kind">,
): TileState {
  if (q.data) return { kind: "count", ...read(q.data) };
  if (q.isError)
    return {
      kind: "failed",
      service: DIENST[q.error instanceof ServiceUnavailable ? q.error.service : service],
    };
  return { kind: "loading" };
}

function FristenTile() {
  const { rolle } = useRolle();
  const overdue = useQuery({
    queryKey: [rolle, "processd", "list_overdue_processes"],
    queryFn: () => call("processd", "list_overdue_processes", {}),
    refetchInterval: 60_000,
  });
  // Every row is a process the read model found past its Antwortfrist, so a
  // non-empty answer is the service's own breach — not a threshold set here.
  return (
    <Tile
      label="Fristen überschritten"
      source={DIENST.processd}
      href="/fristen"
      state={tileState(overdue, "processd", (d) => ({
        n: d.count,
        atLeast: d.saturated,
        breach: d.count > 0,
      }))}
    />
  );
}

function AnmeldungenTile() {
  const { rolle } = useRolle();
  const anmeldungen = useQuery({
    queryKey: [rolle, "vertragd", "list_anmeldungen"],
    queryFn: () => call("vertragd", "list_anmeldungen", {}),
    refetchInterval: 60_000,
  });
  return (
    <Tile
      label="Anmeldungen offen"
      source={DIENST.vertragd}
      href="/heute"
      state={tileState(anmeldungen, "vertragd", (d) => ({ n: d.gesamt }))}
    />
  );
}

/**
 * Überblick: a few numbers for whoever answers for the Rolle — each one a
 * service's own figure, each opening the list it counts.
 */
export function UeberblickView() {
  const { rolle } = useRolle();
  return (
    <div className="mx-auto flex max-w-4xl flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">Überblick</h1>
        <p className="m-0 mt-1 text-callout text-text-2">
          {ROLLEN[rolle].label} — jede Zahl so, wie ihr Dienst sie meldet; ein Klick öffnet die
          Liste.
        </p>
      </header>
      <Hinweise />
      <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
        <FristenTile />
        {rolle === "lieferant" && <AnmeldungenTile />}
      </div>
    </div>
  );
}
