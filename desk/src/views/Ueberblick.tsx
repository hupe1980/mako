import { useQuery } from "@tanstack/react-query";
import { DIENST } from "../app/dienste";
import { ROLLEN, useRolle } from "../app/rollen";
import { call, type Service, ServiceUnavailable } from "../data/mcp";
import { Tile, type TileState } from "../design/Tile";
import {
  formatAnteil,
  formatBewertungen,
  hilfreichText,
  MINDEST_URTEILE,
  vorschlagText,
} from "../format/bilanz";
import { formatCount } from "../format/count";
import { DESK_ASSISTANT, useKatalog } from "./Bilanz";
import { useDeadLetters } from "./DeadLetters";
import { Hinweise } from "./Hinweise";

/** A tile's state from its query: the service's number, or why there is none. */
export function tileState<T>(
  q: { data?: T; isError: boolean; error: Error | null },
  service: Service,
  read: (data: NonNullable<T>) => Omit<Extract<TileState, { kind: "count" }>, "kind">,
): TileState {
  if (q.data) {
    const count = read(q.data);
    // An answer without the figure is no figure: the tile says the service did
    // not give one, never a number the Desk made up (`NaN`, a zero).
    return Number.isFinite(count.n)
      ? { kind: "count", ...count }
      : { kind: "failed", service: DIENST[service] };
  }
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

function GeparktTile() {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "accountingd", "list_parked_bookings"],
    queryFn: () => call("accountingd", "list_parked_bookings", { limit: SEITE }),
    refetchInterval: 60_000,
  });
  // `count` is every parked booking the service holds, not the page it
  // lists, so it is exact even when the page is full; each one is an amount
  // the ledger does not carry yet, so any is a breach.
  return (
    <Tile
      label="Geparkte Buchungen"
      source={DIENST.accountingd}
      href="/heute"
      state={tileState(q, "accountingd", (d) => ({ n: d.count, breach: d.count > 0 }))}
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
 * The assistant's record as agentd's catalog states it: how often a person
 * found its answers „hilfreich" — a share only from five verdicts, the count
 * below — and what became of its proposals. It opens the Beipackzettel.
 */
function AssistentTile() {
  const q = useKatalog();
  const b = q.data?.find((f) => f.name === DESK_ASSISTANT)?.bilanz;
  const state = ((): TileState => {
    if (q.isError) return { kind: "failed", service: DIENST.agentd };
    if (!q.data) return { kind: "loading" };
    if (!b) return { kind: "wert", zeilen: ["Nicht aktiviert"] };
    const fenster = `in ${formatCount(b.tage)} Tagen`;
    const n = b.hilfreich.ja + b.hilfreich.nein;
    const vorschlag = vorschlagText(b.vorschlag);
    return n >= MINDEST_URTEILE
      ? {
          kind: "wert",
          wert: formatAnteil(b.hilfreich.ja, n),
          zeilen: [`hilfreich · ${formatBewertungen(n)} ${fenster}`, vorschlag],
        }
      : { kind: "wert", zeilen: [`${hilfreichText(b.hilfreich)} ${fenster}`, vorschlag] };
  })();
  return (
    <Tile
      label="Assistent"
      source={DIENST.agentd}
      href="/assistent"
      state={state}
      ziel="Beipackzettel öffnen"
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
            <GeparktTile />
          </>
        )}
        {rolle === "messstelle" && <AbleseauftraegeTile />}
        <AssistentTile />
      </div>
    </div>
  );
}
