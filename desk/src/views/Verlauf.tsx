import { useQuery } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useBelegZiel } from "../app/belege";
import { fristArt } from "../app/prozesse";
import { offeneFristen } from "../app/prozessStand";
import { useRolle } from "../app/rollen";
import type { DeadlineView, HistoryEntry } from "../data/gen/makod";
import { call } from "../data/mcp";
import { FristUhr } from "../design/FristUhr";
import { Notice } from "../design/Panel";
import { antwortTeile } from "../format/antwort";
import { formatCount } from "../format/count";
import { formatInstant } from "../format/date";
import { eintrag, useT } from "../i18n";

function Message({ entry }: { entry: HistoryEntry }) {
  const t = useT();
  const eingang = entry.richtung === "eingang";
  const antwort = antwortTeile(entry);
  return (
    <div
      className={`rounded-[12px] border px-3 py-2 ${
        eingang
          ? "border-separator bg-surface"
          : "border-[color-mix(in_srgb,var(--accent)_35%,transparent)] bg-[color-mix(in_srgb,var(--accent)_8%,transparent)]"
      }`}
    >
      <p className="m-0 flex items-baseline gap-2 text-body">
        <span className={`font-semibold ${eingang ? "text-text" : "text-accent"}`}>
          {eingang ? t.verlauf.eingang : t.verlauf.ausgang}
        </span>
        {entry.pid && <span className="text-text">PID {entry.pid}</span>}
      </p>
      {antwort && (
        <p className="m-0 mt-1 text-body text-text">
          <span className="font-semibold">{antwort.code}</span>
          {" — "}
          <span className={entry.bedeutung ? undefined : "text-text-2"}>{antwort.text}</span>
        </p>
      )}
      {entry.akteur && (
        // Who caused it, as makod states the class; a proposal was involved
        // only in an approval, so only those carry the assistant's colour.
        <p
          className={`m-0 mt-0.5 text-caption ${entry.akteur === "freigabe" || entry.akteur === "freigabe-geaendert" ? "text-ai" : "text-text-2"}`}
        >
          {eintrag(t.verlauf.akteur, entry.akteur)}
        </p>
      )}
      <p className="m-0 mt-0.5 text-caption text-text-2">
        {entry.recorded_at ? t.zeit.uhr(formatInstant(entry.recorded_at)) : t.verlauf.ohneZeitpunkt}
        {entry.message_ref && (
          <>
            {" · "}
            <span>{entry.message_ref}</span>
          </>
        )}
      </p>
    </div>
  );
}

function Dot({ tone }: { tone: "message" | "step" | "frist" }) {
  const look =
    tone === "message"
      ? "size-3 bg-accent ring-4 ring-[color-mix(in_srgb,var(--accent)_18%,transparent)]"
      : tone === "frist"
        ? "size-3 border-2 border-soon bg-surface"
        : "size-2 bg-separator";
  return <span aria-hidden="true" className={`block rounded-full ${look}`} />;
}

/**
 * The thread's rows: each message alone, and every run of steps between two
 * messages folded into one — a step is the workflow's bookkeeping, and its
 * identifier is shown on request, never translated by guessing at its name.
 */
function zeilen(eintraege: HistoryEntry[]): (HistoryEntry | HistoryEntry[])[] {
  const out: (HistoryEntry | HistoryEntry[])[] = [];
  for (const e of eintraege) {
    const last = out[out.length - 1];
    if (e.richtung) out.push(e);
    else if (Array.isArray(last)) last.push(e);
    else out.push([e]);
  }
  return out;
}

function Schritte({ steps }: { steps: HistoryEntry[] }) {
  const t = useT();
  return (
    <details className="group text-caption text-text-2">
      <summary className="focus-ring cursor-default list-none rounded-[6px] [&::-webkit-details-marker]:hidden">
        {t.verlauf.schritte(steps.length, formatCount(steps.length))}
      </summary>
      <ul className="m-0 mt-1 list-none p-0">
        {steps.map((s) => (
          <li key={s.sequence} className="font-mono">
            {s.event_type}
          </li>
        ))}
      </ul>
    </details>
  );
}

/** One row of the thread: Eingang left, Ausgang right, everything else centred. */
function Row({ left, mid, right }: { left?: ReactNode; mid: ReactNode; right?: ReactNode }) {
  return (
    <li className="grid grid-cols-[1fr_28px_1fr] items-center gap-3 py-1.5">
      <div className="flex justify-end">{left}</div>
      <div className="flex justify-center">{mid}</div>
      <div>{right}</div>
    </li>
  );
}

/** makod's history of one process — one read, shared by every part of the page. */
export function useHistorie(processId: string) {
  const { rolle } = useRolle();
  return useQuery({
    queryKey: [rolle, "makod", "get_process_history", processId],
    queryFn: () => call("makod", "get_process_history", { process_id: processId }),
  });
}

/**
 * The Verlauf of one process: its messages as a thread — what arrived on the
 * left, what was sent on the right, the steps between quietly centred — and
 * the Fristen still pending at its end. Direction and PID are what the
 * workflow stated (makod `get_process_history`); nothing is read off a name.
 */
export function Verlauf({
  processId,
  deadlines,
  bis = null,
  vorspann,
}: {
  processId: string;
  deadlines: DeadlineView[];
  /** Show the process as of this event's sequence; null for now. */
  bis?: number | null;
  /** Under the heading, above the thread: the Stand control. */
  vorspann?: ReactNode;
}) {
  const t = useT();
  const history = useHistorie(processId);
  // An answer citing this Verlauf marks it while the citation has focus.
  const beleg = useBelegZiel(`makod/get_process_history/${processId}`);
  // An earlier Stand knows nothing of the deadlines pending now.
  const pending = bis === null ? offeneFristen(deadlines) : [];
  const eintraege = (history.data?.eintraege ?? []).filter(
    (e) => bis === null || e.sequence <= bis,
  );

  return (
    <section
      {...beleg}
      aria-labelledby={`verlauf-${processId}`}
      className="border-t border-separator px-6 py-5"
    >
      <div className="mb-3 flex items-baseline gap-3">
        <h2 id={`verlauf-${processId}`} className="m-0 text-headline">
          {t.verlauf.titel}
        </h2>
        <span className="flex-1" />
        <span className="text-caption text-text-2">{t.verlauf.legende}</span>
      </div>
      {vorspann}
      {history.isError && (
        <Notice tone="failure" title={t.verlauf.fehler}>
          {history.error.message}
        </Notice>
      )}
      {history.isPending && <div className="skeleton h-24" />}
      {history.data && (
        <ol className="relative m-0 list-none p-0 before:absolute before:top-2 before:bottom-2 before:left-1/2 before:w-px before:-translate-x-1/2 before:bg-separator">
          {zeilen(eintraege).map((z) =>
            Array.isArray(z) ? (
              <Row
                key={`s${z[0]?.sequence}`}
                left={
                  <span className="text-caption text-text-2">
                    {z[0]?.recorded_at ? formatInstant(z[0].recorded_at) : ""}
                  </span>
                }
                mid={<Dot tone="step" />}
                right={<Schritte steps={z} />}
              />
            ) : (
              <Row
                key={z.sequence}
                left={z.richtung === "eingang" ? <Message entry={z} /> : undefined}
                mid={<Dot tone="message" />}
                right={z.richtung === "ausgang" ? <Message entry={z} /> : undefined}
              />
            ),
          )}
          {pending.map((d) => (
            <Row
              key={d.deadline_id}
              left={
                <span className="text-caption text-text-2" title={d.label}>
                  {fristArt(d.art)}
                </span>
              }
              mid={<Dot tone="frist" />}
              right={<FristUhr due={d.due_at} />}
            />
          ))}
        </ol>
      )}
    </section>
  );
}
