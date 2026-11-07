import { useQuery } from "@tanstack/react-query";
import { DIENST } from "../app/dienste";
import { ROLLEN, useRolle } from "../app/rollen";
import { call, type Service, ServiceUnavailable } from "../data/mcp";
import { Tile, type TileState } from "../design/Tile";
import { useDeadLetters } from "./DeadLetters";
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

function DeadLettersTile() {
  const q = useDeadLetters();
  // `gesamt` is every dead letter the service holds, not the page it lists.
  // It marks no breach, so the tile stays uncoloured.
  return (
    <Tile
      label="Dead Letters"
      source={DIENST.makod}
      href="/dead-letters"
      state={tileState(q, "makod", (d) => ({ n: d.gesamt }))}
    />
  );
}

// The tiles below count what a Heute source lists, with the same read Heute
// makes (same key, same arguments), so the number is the rows the click
// shows — a page's count, a floor where the service says it is saturated.
const SEITE = 50;

function PruefpostenTile() {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "billingd", "list_review_queue"],
    queryFn: () => call("billingd", "list_review_queue", { limit: SEITE }),
    refetchInterval: 60_000,
  });
  return (
    <Tile
      label="Rechnungsprüfung"
      source={DIENST.billingd}
      href="/heute"
      state={tileState(q, "billingd", (d) => ({ n: d.count, atLeast: d.saturated }))}
    />
  );
}

function DisputeTile() {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "invoicd", "list_disputes"],
    queryFn: () => call("invoicd", "list_disputes", {}),
    refetchInterval: 60_000,
  });
  return (
    <Tile
      label="Rechnungsdispute"
      source={DIENST.invoicd}
      href="/heute"
      state={tileState(q, "invoicd", (d) => ({ n: d.count, atLeast: d.saturated }))}
    />
  );
}

function AbleseauftraegeTile() {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "edmd", "list_overdue_reading_orders"],
    queryFn: () => call("edmd", "list_overdue_reading_orders", { limit: SEITE }),
    refetchInterval: 60_000,
  });
  // Every row is past the ausfuehrt_bis edmd states, so a non-empty answer is
  // the service's own breach.
  return (
    <Tile
      label="Ableseaufträge überfällig"
      source={DIENST.edmd}
      href="/heute"
      state={tileState(q, "edmd", (d) => ({
        n: d.count,
        atLeast: d.saturated,
        breach: d.count > 0,
      }))}
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
        {rolle === "netz" && <DeadLettersTile />}
        {rolle === "lieferant" && (
          <>
            <AnmeldungenTile />
            <PruefpostenTile />
            <DisputeTile />
          </>
        )}
        {rolle === "messstelle" && <AbleseauftraegeTile />}
      </div>
    </div>
  );
}
