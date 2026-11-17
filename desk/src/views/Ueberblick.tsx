import { useQuery } from "@tanstack/react-query";
import { DIENST } from "../app/dienste";
import { ROLLEN, useRolle } from "../app/rollen";
import { call, type Service, ServiceUnavailable } from "../data/mcp";
import { Page } from "../design/Page";
import { Tile, type TileState } from "../design/Tile";
import {
  formatAnteil,
  formatBewertungen,
  formatQuote,
  hilfreichText,
  MINDEST_URTEILE,
  vorschlagText,
} from "../format/bilanz";
import { formatCount } from "../format/count";
import { useT } from "../i18n";
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
  const t = useT();
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
      label={t.ueberblick.fristen}
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

/** The PID each Rolle's „Ohne Eingriff" tile reports: the process it answers most. */
const EINGRIFF_PID: Partial<Record<keyof typeof ROLLEN, number>> = {
  netz: 55001,
  lieferant: 55001,
  messstelle: 55039,
};

/**
 * How many of a PID's processes this month ended with no person touching
 * them — processd's count, and its share only where processd states one (it
 * gives none below 20 ended processes). The definition is processd's.
 */
function EingriffTile({ pid }: { pid: number }) {
  const t = useT();
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "processd", "get_kpi_report", pid],
    queryFn: () => call("processd", "get_kpi_report", { pid }),
    refetchInterval: 300_000,
  });
  const e = q.data?.eingriff;
  const state: TileState = e
    ? {
        kind: "wert",
        ...(e.anteile ? { wert: formatQuote(e.anteile.ohne_eingriff) } : {}),
        zeilen: [
          t.ueberblick.vonBeendet(formatCount(e.ohne_eingriff), formatCount(e.beendet)),
          e.definition,
        ],
      }
    : q.isError
      ? { kind: "failed", service: DIENST.processd }
      : { kind: "loading" };
  return (
    <Tile
      label={t.ueberblick.ohneEingriff(`${pid}`)}
      source={DIENST.processd}
      href="/heute"
      state={state}
    />
  );
}

/**
 * The § 40c EnWG billing deadlines, one tile per Lieferant billingd reports:
 * the periods past their deadline unbilled, and what was owed, issued on time
 * and issued late — billingd's counts, side by side, none added up here.
 */
function AbrechnungsfristenTiles() {
  const t = useT();
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "billingd", "summarize_billing_deadlines"],
    queryFn: () => call("billingd", "summarize_billing_deadlines", {}),
    refetchInterval: 300_000,
  });
  if (q.isError || q.isPending)
    return (
      <Tile
        label={t.ueberblick.abrechnungsfristen}
        source={DIENST.billingd}
        href="/heute"
        state={q.isError ? { kind: "failed", service: DIENST.billingd } : { kind: "loading" }}
      />
    );
  return (
    <>
      {(q.data.lieferanten ?? []).map((l) => (
        <Tile
          key={l.lf_mp_id}
          label={t.ueberblick.abrechnungsfristenVon(l.lf_mp_id)}
          source={DIENST.billingd}
          href="/heute"
          state={{
            kind: "wert",
            wert: formatCount(l.overdue),
            zeilen: [
              t.ueberblick.fristenStand(
                formatCount(l.owed),
                formatCount(l.issued_on_time),
                formatCount(l.issued_late),
              ),
            ],
          }}
        />
      ))}
    </>
  );
}

function AnmeldungenTile() {
  const t = useT();
  const { rolle } = useRolle();
  const anmeldungen = useQuery({
    queryKey: [rolle, "vertragd", "list_anmeldungen"],
    queryFn: () => call("vertragd", "list_anmeldungen", {}),
    refetchInterval: 60_000,
  });
  return (
    <Tile
      label={t.ueberblick.anmeldungen}
      source={DIENST.vertragd}
      href="/heute"
      state={tileState(anmeldungen, "vertragd", (d) => ({ n: d.gesamt }))}
    />
  );
}

function DeadLettersTile() {
  const t = useT();
  const q = useDeadLetters();
  // `gesamt` is every dead letter the service holds, not the page it lists.
  // It marks no breach, so the tile stays uncoloured.
  return (
    <Tile
      label={t.ueberblick.deadLetters}
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
  const t = useT();
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "billingd", "list_review_queue"],
    queryFn: () => call("billingd", "list_review_queue", { limit: SEITE }),
    refetchInterval: 60_000,
  });
  return (
    <Tile
      label={t.ueberblick.pruefung}
      source={DIENST.billingd}
      href="/heute"
      state={tileState(q, "billingd", (d) => ({ n: d.count, atLeast: d.saturated }))}
    />
  );
}

function GeparktTile() {
  const t = useT();
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
      label={t.ueberblick.geparkt}
      source={DIENST.accountingd}
      href="/heute"
      state={tileState(q, "accountingd", (d) => ({ n: d.count, breach: d.count > 0 }))}
    />
  );
}

function DisputeTile() {
  const t = useT();
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "invoicd", "list_disputes"],
    queryFn: () => call("invoicd", "list_disputes", {}),
    refetchInterval: 60_000,
  });
  return (
    <Tile
      label={t.ueberblick.dispute}
      source={DIENST.invoicd}
      href="/heute"
      state={tileState(q, "invoicd", (d) => ({ n: d.count, atLeast: d.saturated }))}
    />
  );
}

function AbleseauftraegeTile() {
  const t = useT();
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
      label={t.ueberblick.ablesung}
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
  const t = useT();
  const q = useKatalog();
  const b = q.data?.find((f) => f.name === DESK_ASSISTANT)?.bilanz;
  const state = ((): TileState => {
    if (q.isError) return { kind: "failed", service: DIENST.agentd };
    if (!q.data) return { kind: "loading" };
    if (!b) return { kind: "wert", zeilen: [t.ueberblick.nichtAktiviert] };
    const fenster = t.ueberblick.fenster(formatCount(b.tage));
    const n = b.hilfreich.ja + b.hilfreich.nein;
    const vorschlag = vorschlagText(b.vorschlag);
    return n >= MINDEST_URTEILE
      ? {
          kind: "wert",
          wert: formatAnteil(b.hilfreich.ja, n),
          zeilen: [t.ueberblick.hilfreich(formatBewertungen(n), fenster), vorschlag],
        }
      : { kind: "wert", zeilen: [`${hilfreichText(b.hilfreich)} ${fenster}`, vorschlag] };
  })();
  return (
    <Tile
      label={t.ueberblick.assistent}
      source={DIENST.agentd}
      href="/assistent"
      state={state}
      ziel={t.ueberblick.beipackzettel}
    />
  );
}

/**
 * Überblick: a few numbers for whoever answers for the Rolle — each one a
 * service's own figure, each opening the list it counts.
 */
export function UeberblickView() {
  const t = useT();
  const { rolle } = useRolle();
  return (
    <Page
      width="work"
      title={t.ueberblick.titel}
      subtitle={t.ueberblick.untertitel(ROLLEN[rolle].label)}
    >
      <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-3">
        <FristenTile />
        {EINGRIFF_PID[rolle] && <EingriffTile pid={EINGRIFF_PID[rolle]} />}
        {rolle === "netz" && <DeadLettersTile />}
        {rolle === "lieferant" && (
          <>
            <AnmeldungenTile />
            <PruefpostenTile />
            <DisputeTile />
            <GeparktTile />
            <AbrechnungsfristenTiles />
          </>
        )}
        {rolle === "messstelle" && <AbleseauftraegeTile />}
        <AssistentTile />
      </div>
      {/* The assistant's notes follow the figures, folded to one line. */}
      <Hinweise />
    </Page>
  );
}
