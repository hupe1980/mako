import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import type { ReactNode } from "react";
import { Link } from "react-aria-components";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { zustand } from "../app/prozesse";
import { useRolle } from "../app/rollen";
import type { ProcessRow } from "../data/gen/processd";
import { call, ServiceUnavailable } from "../data/mcp";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { Angabe, ListDetail } from "../design/ListDetail";
import { ListRow } from "../design/ListRow";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SourceChip } from "../design/SourceChip";
import { formatCount } from "../format/count";
import { formatInstant } from "../format/date";
import { useT } from "../i18n";
import { Verlauf } from "./Verlauf";

/** What a row is called: its MaLo, or the process where it has none. */
const name = (p: ProcessRow) => p.malo_id ?? p.process_id;

function Kopf({ children }: { children?: ReactNode }) {
  const t = useT();
  return (
    <header className="flex flex-col gap-4">
      <div>
        <h1 className="m-0 text-large font-bold tracking-tight">{t.fristen.titel}</h1>
        <p className="m-0 mt-1 text-callout text-text-2">{t.fristen.untertitel}</p>
      </div>
      {children}
    </header>
  );
}

/** The selected process: what it is, its Frist, and its Verlauf. */
function Detail({ p }: { p: ProcessRow }) {
  const t = useT();
  return (
    <article>
      <header className="flex flex-col gap-2 px-6 pt-6 pb-5">
        <h2 className="m-0 font-mono text-title font-semibold tracking-tight">{name(p)}</h2>
        <p className="m-0 flex flex-wrap items-center gap-x-3 gap-y-1 text-body text-text-2">
          <span>{t.fristen.zeile(String(p.pid), zustand(p.state))}</span>
          {p.deadline_at && <FristUhr due={p.deadline_at} />}
        </p>
        {p.malo_id && (
          <Link
            href={`/prozess?key=${p.malo_id}`}
            className="focus-ring inline-flex w-fit items-center gap-1 rounded-[6px] text-body font-medium text-accent no-underline data-[hovered]:underline"
          >
            {t.fristen.prozessOeffnen}
            <Icon name="chevron" size={14} />
          </Link>
        )}
      </header>
      <Verlauf processId={p.process_id} deadlines={[]} />
    </article>
  );
}

/** Every fact processd returned for the row — the identifiers live here, not in the title. */
function Angaben({ p }: { p: ProcessRow }) {
  const t = useT();
  const f = t.fristen.feld;
  const ohne = t.fristen.ohne;
  return (
    <dl className="m-0">
      <Angabe name={f.malo}>
        <span className="font-mono">{p.malo_id ?? ohne}</span>
      </Angabe>
      <Angabe name={f.pid}>
        <span className="font-mono">{p.pid}</span>
      </Angabe>
      <Angabe name={f.zustand}>{zustand(p.state)}</Angabe>
      <Angabe name={f.frist}>
        {p.deadline_at ? t.zeit.uhr(formatInstant(p.deadline_at)) : ohne}
      </Angabe>
      <Angabe name={f.fundstelle}>
        {p.deadline_source ? <SourceChip quelle={{ dokument: p.deadline_source }} /> : ohne}
      </Angabe>
      <Angabe name={f.begonnen}>
        {p.started_at ? t.zeit.uhr(formatInstant(p.started_at)) : ohne}
      </Angabe>
      <Angabe name={f.marktpartner}>
        <span className="font-mono">{p.partner_mp_id ?? ohne}</span>
      </Angabe>
      <Angabe name={f.familie}>
        <span className="font-mono">{p.family}</span>
      </Angabe>
      <Angabe name={f.prozess}>
        <span className="font-mono break-all">{p.process_id}</span>
      </Angabe>
    </dl>
  );
}

/**
 * Fristen: every open process past its Antwortfrist, as processd's read model
 * has it — the list on the left, the selected process with its Verlauf in the
 * middle, its Angaben on the right. The selection is in the address, so a
 * link names the process it shows.
 */
export function FristenView() {
  const t = useT();
  const { rolle } = useRolle();
  const { id } = useSearch({ from: "/fristen" });
  const navigate = useNavigate({ from: "/fristen" });
  const overdue = useQuery({
    queryKey: [rolle, "processd", "list_overdue_processes"],
    queryFn: () => call("processd", "list_overdue_processes", {}),
    refetchInterval: 60_000,
  });
  const rows = overdue.data?.overdue ?? [];
  const gewaehlt = rows.find((p) => p.process_id === id) ?? null;

  if (rows.length === 0) {
    return (
      <div className="mx-auto flex max-w-3xl flex-col gap-6 px-4 py-6 md:px-8 md:py-10">
        <Kopf>
          {overdue.isError && (
            <Notice
              tone="failure"
              title={
                overdue.error instanceof ServiceUnavailable
                  ? nichtErreichbar(overdue.error.service)
                  : t.fristen.fehler
              }
            >
              {overdue.error.message}
            </Notice>
          )}
        </Kopf>
        {overdue.isPending && <div className="skeleton h-24" />}
        {overdue.data && (
          <Panel>
            <EmptyState title={t.fristen.leer}>{t.fristen.leerText}</EmptyState>
          </Panel>
        )}
      </div>
    );
  }

  return (
    <ListDetail
      label={t.fristen.liste}
      kopf={<Kopf />}
      selected={gewaehlt?.process_id ?? null}
      onSelect={(next) => navigate({ search: { id: next }, replace: true })}
      liste={rows.map((p) => (
        <ListRow
          key={p.process_id}
          id={p.process_id}
          textValue={name(p)}
          title={<span className="font-mono">{name(p)}</span>}
          subtitle={t.fristen.zeile(String(p.pid), zustand(p.state))}
          trailing={p.deadline_at && <FristUhr due={p.deadline_at} />}
        />
      ))}
      fuss={
        overdue.data?.saturated && (
          <p className="m-0 border-t border-separator px-5 py-3 text-body text-text-2">
            {t.fristen.mindestens(formatCount(overdue.data.limit), DIENST.processd)}
          </p>
        )
      }
      detail={gewaehlt && <Detail p={gewaehlt} />}
      angaben={gewaehlt && <Angaben p={gewaehlt} />}
    />
  );
}
