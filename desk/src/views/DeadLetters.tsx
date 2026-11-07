import { useQuery } from "@tanstack/react-query";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import { call, ServiceUnavailable } from "../data/mcp";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { formatCount } from "../format/count";
import { formatInstant } from "../format/date";

/** The read the Dead-Letters tile and this list share — one request for both. */
export function useDeadLetters() {
  const { rolle } = useRolle();
  return useQuery({
    queryKey: [rolle, "makod", "list_dead_letters"],
    queryFn: () => call("makod", "list_dead_letters", {}),
    refetchInterval: 60_000,
  });
}

/**
 * Dead Letters: the messages the Marktkommunikation could not take or could
 * not deliver, the most recent first — each one waits on a person.
 */
export function DeadLettersView() {
  const q = useDeadLetters();
  const rows = q.data?.dead_letters ?? [];

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">Dead Letters</h1>
        <p className="m-0 mt-1 text-callout text-text-2">
          Nachrichten, die kein Prozess annahm oder deren Zustellung endgültig scheiterte — die
          jüngste zuerst.
        </p>
      </header>

      {q.isError && (
        <Notice
          tone="failure"
          title={
            q.error instanceof ServiceUnavailable
              ? nichtErreichbar(q.error.service)
              : "Die Dead Letters ließen sich nicht lesen"
          }
        >
          {q.error.message}
        </Notice>
      )}

      {q.data && rows.length === 0 && (
        <Panel>
          <EmptyState title="Keine Dead Letters.">
            Jede Nachricht wurde angenommen oder zugestellt.
          </EmptyState>
        </Panel>
      )}

      {rows.length > 0 && (
        <Panel className="overflow-hidden">
          <ul className="m-0 list-none p-0">
            {rows.map((d, i) => (
              <li
                // A dead letter carries no id of its own; the page is
                // ordered by the service and replaced whole on each read.
                // biome-ignore lint/suspicious/noArrayIndexKey: see above
                key={i}
                className="flex items-start gap-4 border-b border-separator px-5 py-3 last:border-b-0"
              >
                <span className="min-w-0 flex-1">
                  <span className="block text-callout text-text">{d.reason_detail}</span>
                  <span className="mt-0.5 block font-mono text-body text-text-2">
                    {d.reason_label}
                  </span>
                </span>
                <span className="shrink-0 text-body tabular-nums text-text-2">
                  {d.rejected_at ? formatInstant(d.rejected_at) : "Zeitpunkt unlesbar"}
                </span>
              </li>
            ))}
          </ul>
          {q.data && q.data.gesamt > rows.length && (
            <p className="m-0 border-t border-separator px-5 py-3 text-body text-text-2">
              Die jüngsten {formatCount(rows.length)} von {formatCount(q.data.gesamt)} — mehr
              liefert die {DIENST.makod} nicht auf einmal.
            </p>
          )}
        </Panel>
      )}
    </div>
  );
}
