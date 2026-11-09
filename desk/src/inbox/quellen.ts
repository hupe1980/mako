// The inbox sources, one hook each, as the plan's Heute table assigns them to
// the Rollen. A source reads one typed tool and maps its rows; it is enabled
// only for the Rollen that read it, so a hidden one costs no request.

import { useQuery } from "@tanstack/react-query";
import { nichtErreichbar } from "../app/dienste";
import { ROLLEN, type Rolle, useRolle } from "../app/rollen";
import { type Answer, type Hinweis, hinweise, type VorschlagArt, vorschlaege } from "../data/agent";
import { abgelehntWeil, buchungsart } from "../data/buchung";
import { call, ServiceUnavailable } from "../data/mcp";
import type { Decimal, Instant } from "../format/brands";
import { formatBusinessDate, formatInstant } from "../format/date";
import { formatCent, formatEuro } from "../format/money";
import { katalog } from "../i18n";
import { ANMELDUNG_PIDS } from "../views/Lage";
import type { Eintrag, QuelleId, Stand } from "./model";

/** What each Rolle's Heute reads; every Rolle also reads the agent's Hinweise. */
export const QUELLEN: Record<Rolle, QuelleId[]> = {
  netz: ["freigaben", "ueberfaellig", "korrekturbedarf"],
  lieferant: [
    "anmeldungen",
    "freigaben",
    "ueberfaellig",
    "dispute",
    "pruefposten",
    "geparkt",
    "nicht_uebermittelt",
  ],
  messstelle: ["freigaben", "ueberfaellig", "ableseauftraege"],
  energieservice: ["freigaben", "ueberfaellig"],
};

/** The sources' words, in the Sprache in force. */
const wort = () => katalog().quellen;

/** A page: what one source shows at most per refresh. */
const SEITE = 50;
/** While Heute is on screen, how often it reads again (refetch pauses in a hidden tab). */
const TAKT = 15_000;

function fehlerText(error: unknown): string {
  if (error instanceof ServiceUnavailable) return nichtErreichbar(error.service);
  return error instanceof Error ? error.message : String(error);
}

function stand(
  quelle: QuelleId,
  label: string,
  q: { isPending: boolean; error: unknown },
  eintraege: Eintrag[],
  mehr: boolean,
): Stand {
  return {
    quelle,
    label,
    eintraege,
    laedt: q.isPending,
    fehler: q.error ? fehlerText(q.error) : null,
    mehr,
  };
}

function useFreigaben(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "processd", "list_pending_approvals", "inbox"],
    queryFn: () =>
      call("processd", "list_pending_approvals", {
        marktrolle: ROLLEN[rolle].marktrolle,
        limit: SEITE,
      }),
    enabled: aktiv,
    refetchInterval: TAKT,
  });
  const eintraege: Eintrag[] = (q.data?.freigaben ?? []).map((f) => ({
    id: `freigabe:${f.id}`,
    quelle: "freigaben",
    titel: f.malo_id ?? wort().prozess(f.process_id),
    kennung: true,
    detail: `PID ${f.pid} · ${f.art === "gehalten" ? wort().antwortFestgehalten : wort().antwortOffen}`,
    grund: f.grund,
    frist: f.expires_at,
    bezug: [f.process_id, ...(f.malo_id ? [f.malo_id] : [])],
    anker: { freigabe: f.id },
    ziel: { to: "/freigaben", search: { id: f.id } },
    freigabe: f,
  }));
  return stand("freigaben", wort().freigaben, q, eintraege, q.data?.saturated ?? false);
}

function useUeberfaellig(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "processd", "list_overdue_processes"],
    queryFn: () => call("processd", "list_overdue_processes", {}),
    enabled: aktiv,
    refetchInterval: TAKT,
  });
  const eintraege: Eintrag[] = (q.data?.overdue ?? []).map((p) => ({
    id: `prozess:${p.process_id}`,
    quelle: "ueberfaellig",
    titel: p.malo_id ?? p.process_id,
    kennung: true,
    detail: `PID ${p.pid} · ${p.state}`,
    grund: p.deadline_source ?? p.family,
    frist: p.deadline_at ?? null,
    bezug: [p.process_id, ...(p.malo_id ? [p.malo_id] : [])],
    anker: p.malo_id ? { malo: p.malo_id } : { process: p.process_id },
    ziel: { to: "/prozess", search: { key: p.malo_id ?? p.process_id } },
  }));
  return stand("ueberfaellig", wort().ueberfaellig, q, eintraege, q.data?.saturated ?? false);
}

/**
 * The Lieferant's open Anmeldungen in processd's Frist order; their customers'
 * names come with every other item's, from `useBenannt`.
 */
function useAnmeldungen(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "processd", "list_open_processes", ANMELDUNG_PIDS],
    queryFn: () => call("processd", "list_open_processes", { pids: ANMELDUNG_PIDS, limit: SEITE }),
    enabled: aktiv,
    refetchInterval: TAKT,
  });
  const eintraege: Eintrag[] = (q.data?.prozesse ?? []).map((p) => {
    const id = p.malo_id ?? p.process_id;
    return {
      id: `prozess:${p.process_id}`,
      quelle: "anmeldungen",
      titel: id,
      kennung: true,
      detail: `PID ${p.pid}`,
      grund: wort().anmeldungWartet,
      frist: p.deadline_at ?? null,
      bezug: [p.process_id, ...(p.malo_id ? [p.malo_id] : [])],
      anker: p.malo_id ? { malo: p.malo_id } : { process: p.process_id },
      ziel: { to: "/prozess", search: { key: id } },
    };
  });
  return stand("anmeldungen", wort().anmeldungen, q, eintraege, Boolean(q.data?.weiter));
}

/** Angemeldet in vertragd, but no makod process carries it yet — so no Frist. */
function useNichtUebermittelt(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "vertragd", "list_anmeldungen", "ohne_prozess"],
    queryFn: () => call("vertragd", "list_anmeldungen", { ohne_prozess: true, limit: SEITE }),
    enabled: aktiv,
    refetchInterval: 60_000,
  });
  const eintraege: Eintrag[] = (q.data?.anmeldungen ?? []).map((a) => ({
    id: `anmeldung:${a.kunden_id}:${a.malo_id}`,
    quelle: "nicht_uebermittelt",
    titel: a.name ?? a.malo_id,
    kennung: !a.name,
    detail: `${a.malo_id} · ${a.sparte} · ${wort().vertrag(a.vertrags_nr)}`,
    grund: wort().nichtUebermitteltGrund,
    frist: null,
    bezug: [a.malo_id],
    anker: { kunde: a.kunden_id },
    ziel: { to: "/kunde/$id", params: { id: a.kunden_id } },
  }));
  const gesamt = q.data?.gesamt ?? 0;
  return stand(
    "nicht_uebermittelt",
    wort().nichtUebermittelt,
    q,
    eintraege,
    gesamt > eintraege.length,
  );
}

/** Negative Prüfmitteilungen no corrected Summenzeitreihe has answered yet. */
function useKorrekturbedarf(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "netzbilanzd", "list_korrekturbedarf"],
    queryFn: () => call("netzbilanzd", "list_korrekturbedarf", {}),
    enabled: aktiv,
    refetchInterval: TAKT,
  });
  const eintraege: Eintrag[] = (q.data?.korrekturbedarf ?? []).map((k) => ({
    id: `korrektur:${k.pruefmitteilung_id}`,
    quelle: "korrekturbedarf",
    titel: k.bilanzierungsgebiet_id,
    kennung: true,
    detail: `${formatBusinessDate(k.period_from)} – ${formatBusinessDate(k.period_to)}`,
    grund: wort().korrekturGrund,
    frist: null,
    bezug: [k.bilanzierungsgebiet_id],
    anker: null,
    ziel: null,
  }));
  return stand("korrekturbedarf", wort().korrekturbedarf, q, eintraege, false);
}

/** Reading orders past their ausfuehrt_bis, the Frist as edmd states it. */
function useAbleseauftraege(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "edmd", "list_overdue_reading_orders"],
    queryFn: () => call("edmd", "list_overdue_reading_orders", { limit: SEITE }),
    enabled: aktiv,
    refetchInterval: TAKT,
  });
  const eintraege: Eintrag[] = (q.data?.auftraege ?? []).map((a) => ({
    id: `ablesung:${a.id}`,
    quelle: "ableseauftraege",
    titel: a.malo_id,
    kennung: true,
    detail: `${wort().faellig(formatBusinessDate(a.ausfuehrt_bis))} · ${a.status}`,
    grund: a.fehlschlag_grund ? `${a.anlass} — ${a.fehlschlag_grund}` : wort().stehtAus(a.anlass),
    frist: a.frist,
    bezug: [a.malo_id],
    anker: { malo: a.malo_id },
    ziel: { to: "/prozess", search: { key: a.malo_id } },
  }));
  return stand("ableseauftraege", wort().ableseauftraege, q, eintraege, q.data?.saturated ?? false);
}

/**
 * INVOIC in dispute, due by the Zahlungsziel the invoice stated, each titled
 * by who sent it — every Rechnungssteller of the page named in one marktd
 * read, never one per row.
 */
function useDispute(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "invoicd", "list_disputes"],
    queryFn: () => call("invoicd", "list_disputes", {}),
    enabled: aktiv,
    refetchInterval: TAKT,
  });
  const mpIds = [...new Set((q.data?.disputes ?? []).map((d) => d.sender_mp_id))].sort();
  const namen = useQuery({
    queryKey: [rolle, "marktd", "partner_namen", mpIds],
    queryFn: () => call("marktd", "partner_namen", { mp_ids: mpIds }),
    enabled: aktiv && mpIds.length > 0,
    staleTime: 300_000,
  });
  const name = new Map((namen.data?.partner ?? []).map((p) => [p.mp_id, p.name]));
  const eintraege: Eintrag[] = (q.data?.disputes ?? []).map((d) => ({
    id: `prozess:${d.process_id}`,
    quelle: "dispute",
    titel:
      name.get(d.sender_mp_id) ??
      (d.rechnungsnummer ? wort().rechnung(d.rechnungsnummer) : wort().rechnungImDisput),
    kennung: false,
    detail: [
      name.has(d.sender_mp_id) && d.rechnungsnummer ? wort().rechnung(d.rechnungsnummer) : null,
      d.sender_mp_id,
      `PID ${d.pid}`,
      d.abgesendet ? wort().ablehnungVersandt : wort().ablehnungNichtVersandt,
    ]
      .filter(Boolean)
      .join(" · "),
    grund: d.befunde[0]?.bezeichnung ?? wort().disputGrund,
    frist: d.frist ?? null,
    bezug: [d.process_id],
    anker: { disput: d.id },
    ziel: { to: "/prozess", search: { key: d.process_id } },
    disput: d,
  }));
  return stand("dispute", wort().dispute, q, eintraege, q.data?.saturated ?? false);
}

/** Billing records the risk check held for a person, highest risk first. */
function usePruefposten(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "billingd", "list_review_queue"],
    queryFn: () => call("billingd", "list_review_queue", { limit: SEITE }),
    enabled: aktiv,
    refetchInterval: TAKT,
  });
  const eintraege: Eintrag[] = (q.data?.records ?? []).map((r) => ({
    id: `pruefung:${r.id}`,
    quelle: "pruefposten",
    titel: r.malo_id,
    kennung: true,
    detail: `${r.rechnungsnummer} · ${formatBusinessDate(r.period_from)} – ${formatBusinessDate(r.period_to)} · ${formatEuro(r.total_brutto_eur as Decimal)}`,
    grund:
      r.risk_findings[0]?.message ??
      (r.risk_band === "HELD" ? wort().angehalten : wort().zurPruefung),
    frist: null,
    bezug: [r.malo_id],
    anker: { malo: r.malo_id },
    ziel: null,
    pruefposten: r,
  }));
  return stand("pruefposten", wort().pruefposten, q, eintraege, q.data?.saturated ?? false);
}

/**
 * Bookings the ledger refused for good, oldest first — each titled by its
 * MaLo, which `useBenannt` turns into the customer's name.
 */
function useGeparkt(aktiv: boolean): Stand {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "accountingd", "list_parked_bookings"],
    queryFn: () => call("accountingd", "list_parked_bookings", { limit: SEITE }),
    enabled: aktiv,
    refetchInterval: TAKT,
  });
  const eintraege: Eintrag[] = (q.data?.buchungen ?? []).map((b) => ({
    id: `buchung:${b.key}`,
    quelle: "geparkt",
    titel: b.malo_id,
    kennung: true,
    detail: [
      b.beleg ? wort().beleg(b.beleg) : null,
      wort().buchungsdatum(formatBusinessDate(b.buchungsdatum)),
      wort().geparktSeit(formatInstant(b.seit)),
    ]
      .filter(Boolean)
      .join(" · "),
    grund: `${buchungsart(b.art)} ${formatCent(b.betrag_ct)} — ${abgelehntWeil(b.code)}`,
    frist: null,
    bezug: [b.malo_id],
    anker: { buchung: b.key },
    ziel: null,
    geparkt: b,
  }));
  return stand("geparkt", wort().geparkt, q, eintraege, q.data?.saturated ?? false);
}

/** The agent's open Hinweise, for every Rolle; agentd absent is no failure of Heute. */
export function useHinweise(): { hinweise: Hinweis[]; fehler: string | null } {
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "agentd", "hinweise"],
    queryFn: hinweise,
    refetchInterval: 60_000,
    retry: false,
  });
  return { hinweise: q.data ?? [], fehler: q.error ? fehlerText(q.error) : null };
}

/** Each kind's waiting proposals, by entry id. */
export type Wartend = Record<VorschlagArt, Map<string, Answer>>;

/**
 * The proposals waiting on the Freigaben, Dispute and geparkte Buchungen on
 * screen, in one read. Polled while one is still being prepared or not yet
 * admitted; quiet once each has its answer. A failure shows no proposal — the
 * decision stands on its own.
 */
export function useVorschlaege(ids: Partial<Record<VorschlagArt, string[]>>): Wartend {
  const { rolle } = useRolle();
  const gefragt = (Object.entries(ids) as [VorschlagArt, string[]][]).filter(
    ([, v]) => v.length > 0,
  );
  const q = useQuery({
    queryKey: [rolle, "agentd", "vorschlaege", gefragt],
    queryFn: () => vorschlaege(ids),
    enabled: gefragt.length > 0,
    retry: false,
    refetchInterval: (query) => {
      const d = query.state.data;
      if (!d) return false;
      const offen = gefragt.some(([art, v]) =>
        v.some((id) => !d[art]?.[id] || d[art]?.[id]?.status === "running"),
      );
      return offen ? 15_000 : false;
    },
  });
  const m: Wartend = { freigabe: new Map(), disput: new Map(), buchung: new Map() };
  for (const art of ["freigabe", "disput", "buchung"] as const)
    for (const [id, a] of Object.entries(q.data?.[art] ?? {})) if (a) m[art].set(id, a);
  return m;
}

/** A Hinweis no item carries is an item of its own, due when it says. */
export function alsEintrag(h: Hinweis): Eintrag {
  const ziel = h.anker.find((a) => a.art !== "process")?.wert ?? h.anker[0]?.wert;
  const malo = h.anker.find((a) => a.art === "malo")?.wert;
  const prozess = h.anker.find((a) => a.art === "process")?.wert;
  return {
    id: `hinweis:${h.task}`,
    quelle: "hinweise",
    titel: h.titel ?? h.skill ?? wort().hinweis,
    kennung: false,
    detail: h.anker.map((a) => a.wert).join(" · "),
    grund: h.text,
    frist: (h.faellig as Instant | null) ?? null,
    bezug: h.anker.map((a) => a.wert),
    anker: malo ? { malo } : prozess ? { process: prozess } : null,
    ziel: { to: "/prozess", search: { key: ziel ?? "" } },
  };
}

/**
 * The Lieferant reads a MaLo by its customer: every item still titled by its
 * MaLo-ID is named in one vertragd read for the whole list, never one per row.
 */
export function useBenannt(eintraege: Eintrag[], aktiv: boolean): Eintrag[] {
  const { rolle } = useRolle();
  const malos = [
    ...new Set(eintraege.flatMap((e) => (e.kennung && /^\d{11}$/.test(e.titel) ? [e.titel] : []))),
  ].sort();
  const q = useQuery({
    queryKey: [rolle, "vertragd", "kunden_zu_malos", malos],
    queryFn: () => call("vertragd", "kunden_zu_malos", { malos }),
    enabled: aktiv && malos.length > 0,
    staleTime: 60_000,
  });
  const kunde = new Map((q.data?.kunden ?? []).map((k) => [k.malo_id, k]));
  if (kunde.size === 0) return eintraege;
  return eintraege.map((e) => {
    const k = e.kennung ? kunde.get(e.titel) : undefined;
    if (!k?.name) return e;
    return {
      ...e,
      titel: k.name,
      kennung: false,
      detail: [e.titel, e.detail, k.kunden_nr].filter(Boolean).join(" · "),
      // A decision keeps its own Anker; the assistant reads the customer from it.
      anker:
        e.anker && ("freigabe" in e.anker || "disput" in e.anker || "buchung" in e.anker)
          ? e.anker
          : { kunde: k.kunden_id },
      ziel:
        e.ziel?.to === "/freigaben" ? e.ziel : { to: "/kunde/$id", params: { id: k.kunden_id } },
    };
  });
}

/**
 * Every source of the active Rolle, in the order its table names them — or
 * only those in `nur`, the rest asking nothing.
 */
export function useQuellen(rolle: Rolle, nur?: readonly QuelleId[]): Stand[] {
  const liest = new Set(QUELLEN[rolle].filter((q) => !nur || nur.includes(q)));
  const alle = [
    useAnmeldungen(liest.has("anmeldungen")),
    useFreigaben(liest.has("freigaben")),
    useUeberfaellig(liest.has("ueberfaellig")),
    useNichtUebermittelt(liest.has("nicht_uebermittelt")),
    useKorrekturbedarf(liest.has("korrekturbedarf")),
    useAbleseauftraege(liest.has("ableseauftraege")),
    useDispute(liest.has("dispute")),
    usePruefposten(liest.has("pruefposten")),
    useGeparkt(liest.has("geparkt")),
  ];
  return [...liest].flatMap((id) => alle.filter((s) => s.quelle === id));
}
