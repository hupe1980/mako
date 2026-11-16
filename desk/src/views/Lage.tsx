import { useQuery } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import { call, type Service, ServiceUnavailable } from "../data/mcp";
import { formatCount } from "../format/count";
import { formatBerlinTime } from "../format/date";
import { useT } from "../i18n";

/** The Lieferant's Anmeldungen: Strom (verbrauchend, erzeugend) and Gas. */
export const ANMELDUNG_PIDS = [55001, 55077, 44001];

/** One phrase of the line, and where it leads — a list on this page or another view. */
function Zeile({
  children,
  ziel,
  tone = "plain",
}: {
  children: ReactNode;
  ziel: string;
  tone?: "plain" | "breach";
}) {
  const router = useRouter();
  return (
    <li>
      <button
        type="button"
        onClick={() => {
          if (ziel.startsWith("#")) {
            const el = document.getElementById(ziel.slice(1));
            el?.scrollIntoView({ behavior: "smooth", block: "start" });
            el?.closest("section")?.querySelector<HTMLElement>("[role=grid]")?.focus();
          } else router.navigate({ to: ziel });
        }}
        className={`focus-ring cursor-default rounded-[6px] border-0 bg-transparent p-0 text-left text-body outline-none hover:underline ${
          tone === "breach" ? "text-critical" : "text-accent"
        }`}
      >
        {children}
      </button>
    </li>
  );
}

function Ausfall({ error, service }: { error: Error; service: Service }) {
  return (
    <li className="text-body text-text-2">
      {nichtErreichbar(error instanceof ServiceUnavailable ? error.service : service)}
    </li>
  );
}

/**
 * The Lage: what the list below does not say — the service's totals beyond its page,
 * each exactly as its service counts it, each opening the list it counts. One
 * line, and none at all when it would only repeat the list. A function of the
 * counts, no model, so it carries no disclosure.
 */
export function Lage() {
  const t = useT();
  const l = t.lage;
  const { rolle } = useRolle();
  const offen = useQuery({
    queryKey: [rolle, "processd", "count_open_processes", ANMELDUNG_PIDS],
    queryFn: () => call("processd", "count_open_processes", { pids: ANMELDUNG_PIDS }),
    refetchInterval: 60_000,
  });
  const ueberfaellig = useQuery({
    queryKey: [rolle, "processd", "list_overdue_processes"],
    queryFn: () => call("processd", "list_overdue_processes", {}),
    refetchInterval: 60_000,
  });
  const ohne = useQuery({
    queryKey: [rolle, "vertragd", "list_anmeldungen", "ohne_prozess", "gesamt"],
    queryFn: () => call("vertragd", "list_anmeldungen", { ohne_prozess: true, limit: 1 }),
    refetchInterval: 60_000,
  });

  const o = offen.data;
  const u = ueberfaellig.data;
  const g = ohne.data?.gesamt;
  const zeilen = [
    o && o.heute_faellig > 0 && (
      <Zeile key="h" ziel="#wartend">
        {l.heuteFaellig(o.heute_faellig, formatCount(o.heute_faellig))}
        {o.heute_frueheste &&
          `, ${l.frueheste(o.heute_faellig === 1, formatBerlinTime(o.heute_frueheste))}`}
      </Zeile>
    ),
    offen.isError && <Ausfall key="o" error={offen.error} service="processd" />,
    u && u.count > 0 && (
      <Zeile key="u" ziel="/fristen" tone="breach">
        {l.ueberschritten(u.count === 1 && !u.saturated, formatCount(u.count, u.saturated))}
      </Zeile>
    ),
    ueberfaellig.isError && <Ausfall key="ue" error={ueberfaellig.error} service="processd" />,
    o && o.offen > 0 && (
      <Zeile key="w" ziel="#wartend">
        {l.wartend(o.offen, formatCount(o.offen))}
      </Zeile>
    ),
    g !== undefined && g > 0 && (
      <Zeile key="g" ziel="#ohne">
        {l.ohneProzess(g, formatCount(g))}
      </Zeile>
    ),
    ohne.isError && <Ausfall key="oe" error={ohne.error} service="vertragd" />,
  ].filter(Boolean);
  if (zeilen.length === 0) return null;

  return (
    <section aria-label={l.titel}>
      <ul className="m-0 flex list-none flex-wrap items-baseline gap-x-5 gap-y-1 p-0 text-body text-text-2">
        {zeilen}
      </ul>
    </section>
  );
}
