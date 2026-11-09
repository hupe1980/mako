import { useQuery } from "@tanstack/react-query";
import { useParams } from "@tanstack/react-router";
import { Link } from "react-aria-components";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import type { Lieferstelle } from "../data/gen/vertragd";
import { call, ServiceUnavailable, ToolRefusal } from "../data/mcp";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { Notice, Panel } from "../design/Panel";
import type { Instant } from "../format/brands";
import { formatCount } from "../format/count";
import { formatBusinessDate } from "../format/date";
import { eintrag, katalog, useT } from "../i18n";
import { Fragen } from "./Fragen";
import { Hinweise } from "./Hinweise";
import { monogram } from "./kundenSuche";

type Tone = "calm" | "soon" | "done" | "critical" | "muted";

/** The words and tone for a Vertragskomponente's state — presentation only. */
function supplyState(l: Lieferstelle): { text: string; tone: Tone } {
  const ab = formatBusinessDate(l.lieferbeginn);
  const bis = l.lieferende ? formatBusinessDate(l.lieferende) : null;
  const k = katalog().kundeAnsicht;
  switch (l.status) {
    case "ANGEMELDET":
      return { text: k.anmeldungLaeuft, tone: "soon" };
    case "BESTAETIGT":
      return { text: k.bestaetigtAb(ab), tone: "calm" };
    case "AKTIV":
      return { text: bis ? k.beliefertBis(bis) : k.beliefertSeit(ab), tone: "done" };
    case "BEENDET":
      return { text: bis ? k.beendetZum(bis) : k.beendet, tone: "muted" };
    case "ABGELEHNT":
      return { text: k.abgelehnt, tone: "critical" };
    case "STORNIERT":
      return { text: k.storniert, tone: "muted" };
    case "ANGELEGT":
      return { text: k.angelegt, tone: "calm" };
    default:
      return { text: l.status, tone: "calm" };
  }
}

const TONE: Record<Tone, string> = {
  calm: "bg-fill text-text",
  soon: "bg-soon-fill text-soon",
  done: "bg-done-fill text-done",
  critical: "bg-critical-fill text-critical",
  muted: "bg-fill text-text-2",
};

function StelleCapsule({ stelle }: { stelle: Lieferstelle }) {
  const t = useT();
  const { text, tone } = supplyState(stelle);
  return (
    <Link
      href={`/prozess?key=${stelle.malo_id}`}
      aria-label={t.kundeAnsicht.stelleOeffnen(stelle.malo_id, text)}
      className={`focus-ring inline-flex h-8 items-center gap-2 rounded-full px-3 text-body no-underline data-[hovered]:brightness-95 ${TONE[tone]}`}
    >
      <span className="font-mono font-medium">{stelle.malo_id}</span>
      <span className="opacity-80">{text}</span>
    </Link>
  );
}

/** Each listed Lieferstelle's earliest open Frist, from one processd call. */
function useFristen(malos: string[]): Map<string, Instant> {
  const { rolle } = useRolle();
  const open = useQuery({
    queryKey: [rolle, "processd", "list_open_processes", "malos", malos],
    queryFn: () => call("processd", "list_open_processes", { malos, limit: 200 }),
    enabled: malos.length > 0,
  });
  // processd returns the earliest Frist first, so the first row per MaLo is it.
  const next = new Map<string, Instant>();
  for (const p of open.data?.prozesse ?? []) {
    if (p.malo_id && p.deadline_at && !next.has(p.malo_id)) next.set(p.malo_id, p.deadline_at);
  }
  return next;
}

function Stellen({ stellen, gesamt }: { stellen: Lieferstelle[]; gesamt: number }) {
  const t = useT();
  const k = t.kundeAnsicht;
  const fristen = useFristen([...new Set(stellen.map((s) => s.malo_id))]);
  return (
    <section aria-labelledby="stellen" className="border-t border-separator px-6 py-5">
      <h2 id="stellen" className="m-0 mb-3 text-callout font-semibold">
        {k.lieferstellen}{" "}
        <span className="font-normal text-text-2">{gesamt ? formatCount(gesamt) : ""}</span>
      </h2>
      {stellen.length === 0 ? (
        <p className="m-0 text-body text-text-2">{k.keineStelle}</p>
      ) : (
        <ul className="m-0 flex list-none flex-col p-0">
          {stellen.map((s) => (
            <li
              key={`${s.malo_id}-${s.vertrags_nr}-${s.lieferbeginn}`}
              className="flex items-center gap-4 border-b border-separator py-2.5 last:border-b-0"
            >
              <span className="min-w-0 flex-1">
                <span className="block text-body text-text">
                  {s.standort ?? s.sparte.toLowerCase()}
                </span>
                <span className="block text-caption text-text-2">
                  {s.sparte} · {k.vertrag} <span className="font-mono">{s.vertrags_nr}</span>
                </span>
              </span>
              {fristen.get(s.malo_id) && <FristUhr due={fristen.get(s.malo_id) as Instant} />}
              <StelleCapsule stelle={s} />
            </li>
          ))}
        </ul>
      )}
      {gesamt > stellen.length && (
        <p className="m-0 mt-3 text-body text-text-2">
          {k.neuesten(formatCount(stellen.length), formatCount(gesamt))} <Kbd>⌘K</Kbd>.
        </p>
      )}
    </section>
  );
}

/** Kunde: one customer as Marktkommunikation and Abrechnung see it. */
export function KundeView() {
  const t = useT();
  const k = t.kundeAnsicht;
  const { id } = useParams({ from: "/kunde/$id" });
  const { rolle } = useRolle();
  const kunde = useQuery({
    queryKey: [rolle, "vertragd", "get_kunde", id],
    queryFn: () => call("vertragd", "get_kunde", { kunden_id: id }),
  });

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-4 py-6 md:px-8 md:py-10">
      {kunde.isPending && (
        <div role="status" aria-busy="true">
          <span className="sr-only">{k.laedt}</span>
          <Panel className="p-6">
            <div className="flex items-center gap-4">
              <div className="skeleton size-14 rounded-full" />
              <div className="flex-1">
                <div className="skeleton h-6 w-56" />
                <div className="skeleton mt-2 h-4 w-32" />
              </div>
            </div>
          </Panel>
        </div>
      )}
      {kunde.isError && (
        <Notice
          tone={kunde.error instanceof ToolRefusal ? "refusal" : "failure"}
          title={
            kunde.error instanceof ToolRefusal
              ? k.nichtGefunden
              : kunde.error instanceof ServiceUnavailable
                ? nichtErreichbar(kunde.error.service)
                : k.unerwartet
          }
        >
          {kunde.error.message}
        </Notice>
      )}
      {kunde.data && (
        <Panel>
          <div className="flex items-center gap-5 px-6 pt-6 pb-5">
            <span className="squircle flex size-16 shrink-0 items-center justify-center rounded-[18px] bg-[color-mix(in_srgb,var(--accent)_16%,transparent)] text-title font-semibold text-accent">
              {monogram(kunde.data.name)}
            </span>
            <div className="min-w-0 flex-1">
              <h1 className="m-0 truncate text-large font-bold tracking-tight">
                {kunde.data.name ?? k.ohneNamen}
              </h1>
              <p className="m-0 mt-1 text-callout text-text-2">
                {kunde.data.kunden_nr && (
                  <>
                    <span className="font-mono">{kunde.data.kunden_nr}</span>
                    {" · "}
                  </>
                )}
                {eintrag(k.kundentyp, kunde.data.kundentyp)}
                {kunde.data.haushaltskunde && ` · ${k.haushaltskunde}`}
              </p>
            </div>
          </div>
          <Stellen stellen={kunde.data.lieferstellen} gesamt={kunde.data.lieferstellen_gesamt} />
        </Panel>
      )}
      {kunde.data && <Hinweise nurFuer={kunde.data.lieferstellen.map((s) => s.malo_id)} />}
      {kunde.data && <Fragen anker={{ kunde: id }} />}
      <p className="m-0 flex items-center gap-1.5 text-caption text-text-2">
        <Icon name="doc" size={12} /> {k.quelle(DIENST.vertragd)}
      </p>
    </div>
  );
}
