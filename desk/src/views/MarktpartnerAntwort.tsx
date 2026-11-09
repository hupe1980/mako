import { useQuery } from "@tanstack/react-query";
import { useRolle } from "../app/rollen";
import { call, ToolRefusal } from "../data/mcp";
import { SourceChip } from "../design/SourceChip";
import { antwortTeile } from "../format/antwort";
import { useT } from "../i18n";

/**
 * The Antwortcode the counterparty answered a process with, and the BDEW's
 * wording of it in the list the answer named — both as processd's read model
 * states them. A process nobody has answered yet shows nothing; a code
 * without a wording stands with its list, never with a guessed meaning.
 */
export function MarktpartnerAntwort({ processId }: { processId: string }) {
  const t = useT();
  const { rolle } = useRolle();
  const query = useQuery({
    queryKey: [rolle, "processd", "get_process", processId],
    queryFn: () => call("processd", "get_process", { process_id: processId }),
  });

  if (query.isError) {
    // No projection for this process: the read model has not seen it, which
    // says nothing about an answer.
    if (query.error instanceof ToolRefusal) return null;
    return <p className="m-0 px-6 pb-4 text-caption text-text-2">{t.prozess.antwort.fehler}</p>;
  }
  const p = query.data;
  const antwort = p
    ? antwortTeile({
        antwortcode: p.antwort_code,
        codeliste: p.antwort_codeliste,
        bedeutung: p.bedeutung,
      })
    : null;
  if (!p || !antwort) return null;
  return (
    <div className="px-6 pb-4">
      <p className="m-0 text-caption font-medium text-text-2">{t.prozess.antwort.titel}</p>
      <p className="m-0 mt-0.5 flex flex-wrap items-baseline gap-x-2 text-body text-text">
        <span>
          <span className="font-mono font-semibold">{antwort.code}</span>
          {" — "}
          <span className={p.bedeutung ? undefined : "text-text-2"}>{antwort.text}</span>
        </span>
        {p.bedeutung && p.antwort_codeliste && (
          <SourceChip quelle={{ dokument: p.antwort_codeliste }} />
        )}
      </p>
    </div>
  );
}
