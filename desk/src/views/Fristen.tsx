import { useQuery } from "@tanstack/react-query";
import { Link } from "react-aria-components";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { zustand } from "../app/prozesse";
import { useRolle } from "../app/rollen";
import { call, ServiceUnavailable } from "../data/mcp";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SourceChip } from "../design/SourceChip";
import { formatCount } from "../format/count";

/** Fristen: every open process past its Antwortfrist, as processd's read model has it. */
export function FristenView() {
  const { rolle } = useRolle();
  const overdue = useQuery({
    queryKey: [rolle, "processd", "list_overdue_processes"],
    queryFn: () => call("processd", "list_overdue_processes", {}),
    refetchInterval: 60_000,
  });
  const rows = overdue.data?.overdue ?? [];

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">Fristen überschritten</h1>
        <p className="m-0 mt-1 text-callout text-text-2">
          Offene Prozesse nach ihrer Antwortfrist — die älteste zuerst, jede mit ihrer Fundstelle.
        </p>
      </header>

      {overdue.isError && (
        <Notice
          tone="failure"
          title={
            overdue.error instanceof ServiceUnavailable
              ? nichtErreichbar(overdue.error.service)
              : "Die Fristen ließen sich nicht lesen"
          }
        >
          {overdue.error.message}
        </Notice>
      )}

      {overdue.data && rows.length === 0 && (
        <Panel>
          <EmptyState title="Keine Frist überschritten.">
            Jeder offene Prozess liegt in seiner Antwortfrist.
          </EmptyState>
        </Panel>
      )}

      {rows.length > 0 && (
        <Panel className="overflow-hidden">
          <ul className="m-0 list-none p-0">
            {rows.map((p) => (
              <li
                key={p.process_id}
                className="flex items-center gap-4 border-b border-separator px-5 py-3 last:border-b-0"
              >
                <span className="min-w-0 flex-1">
                  {p.malo_id ? (
                    <Link
                      href={`/prozess?key=${p.malo_id}`}
                      className="focus-ring block font-mono text-callout font-medium text-text no-underline data-[hovered]:underline"
                    >
                      {p.malo_id}
                    </Link>
                  ) : (
                    <span className="block font-mono text-callout text-text-2">{p.process_id}</span>
                  )}
                  <span className="mt-0.5 flex flex-wrap items-center gap-x-2 gap-y-1 text-body text-text-2">
                    <span>
                      PID <span className="font-mono">{p.pid}</span> · {zustand(p.state)}
                    </span>
                    {p.deadline_source && <SourceChip quelle={{ dokument: p.deadline_source }} />}
                  </span>
                </span>
                {p.deadline_at && <FristUhr due={p.deadline_at} />}
                {p.malo_id && <Icon name="chevron" size={16} className="text-text-2" />}
              </li>
            ))}
          </ul>
          {overdue.data?.saturated && (
            <p className="m-0 border-t border-separator px-5 py-3 text-body text-text-2">
              Mindestens {formatCount(overdue.data.limit)} — mehr liefert die {DIENST.processd}{" "}
              nicht auf einmal.
            </p>
          )}
        </Panel>
      )}
    </div>
  );
}
