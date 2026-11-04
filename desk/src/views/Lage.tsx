import { useQuery } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import { call, type Service, ServiceUnavailable } from "../data/mcp";
import { Icon } from "../design/Icon";
import { Panel } from "../design/Panel";
import { formatCount } from "../format/count";
import { formatBerlinTime } from "../format/date";

/** The Lieferant's Anmeldungen: Strom (verbrauchend, erzeugend) and Gas. */
export const ANMELDUNG_PIDS = [55001, 55077, 44001];

function zahl(n: number, eins: string, mehr: string, atLeast = false): string {
  return `${formatCount(n, atLeast)} ${n === 1 && !atLeast ? eins : mehr}`;
}

/** One line: the sentence, and where it leads — a list on this page or another view. */
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
        className="focus-ring group flex w-full cursor-default items-center gap-3 rounded-[8px] px-3 py-2 text-left text-callout outline-none hover:bg-fill"
      >
        <span
          aria-hidden
          className={`size-1.5 shrink-0 rounded-full ${tone === "breach" ? "bg-critical" : "bg-accent"}`}
        />
        <span className={`flex-1 ${tone === "breach" ? "text-critical" : "text-text"}`}>
          {children}
        </span>
        <Icon
          name="chevron"
          size={14}
          className="text-text-2 opacity-0 transition-opacity group-hover:opacity-100"
        />
      </button>
    </li>
  );
}

function Ausfall({ error, service }: { error: Error; service: Service }) {
  return (
    <li className="px-3 py-2 text-callout text-text-2">
      {nichtErreichbar(error instanceof ServiceUnavailable ? error.service : service)}
    </li>
  );
}

/**
 * The Lage: the day in a few lines, each figure exactly as its service counts
 * it — a function of the counts, no model, so it carries no disclosure. Each
 * line opens the list it counts.
 */
export function Lage() {
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

  const loading = offen.isPending || ueberfaellig.isPending || ohne.isPending;
  const o = offen.data;
  const u = ueberfaellig.data;
  const g = ohne.data?.gesamt;
  const ruhig = o && u && g !== undefined && o.heute_faellig === 0 && u.count === 0 && g === 0;

  return (
    <section aria-labelledby="lage">
      <Panel className="px-3 py-3">
        <div className="flex items-baseline gap-3 px-3 pb-1">
          <h2 id="lage" className="m-0 text-callout font-semibold">
            Lage
          </h2>
          <span className="flex-1" />
          <span className="text-caption text-text-2">
            gezählt von {DIENST.processd} und {DIENST.vertragd}
          </span>
        </div>
        {loading && <div className="skeleton mx-3 my-2 h-16" />}
        {!loading && (
          <ul className="m-0 list-none p-0">
            {offen.isError && <Ausfall error={offen.error} service="processd" />}
            {o && o.heute_faellig > 0 && (
              <Zeile ziel="#wartend">
                {zahl(o.heute_faellig, "Frist läuft", "Fristen laufen")} heute ab
                {o.heute_frueheste && (
                  <span className="text-text-2">
                    {" "}
                    · {o.heute_faellig === 1 ? "um" : "die früheste um"}{" "}
                    {formatBerlinTime(o.heute_frueheste)}
                  </span>
                )}
              </Zeile>
            )}
            {u && u.count > 0 && (
              <Zeile ziel="/fristen" tone="breach">
                {zahl(u.count, "Frist ist", "Fristen sind", u.saturated)} überschritten
              </Zeile>
            )}
            {ueberfaellig.isError && <Ausfall error={ueberfaellig.error} service="processd" />}
            {o && o.offen > 0 && (
              <Zeile ziel="#wartend">
                {zahl(o.offen, "Anmeldung wartet", "Anmeldungen warten")} auf den Netzbetreiber
              </Zeile>
            )}
            {g !== undefined && g > 0 && (
              <Zeile ziel="#ohne">
                {zahl(g, "Anmeldung ist", "Anmeldungen sind")} noch nicht übermittelt
              </Zeile>
            )}
            {ohne.isError && <Ausfall error={ohne.error} service="vertragd" />}
            {ruhig && (
              <li className="px-3 py-2 text-callout text-text-2">
                Nichts läuft heute ab, nichts ist überschritten.
              </li>
            )}
          </ul>
        )}
      </Panel>
    </section>
  );
}
