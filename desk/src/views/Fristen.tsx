import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { Link } from "react-aria-components";
import { family, zustand } from "../app/codes";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import type { ProcessRow } from "../data/gen/processd";
import { call, ServiceUnavailable } from "../data/mcp";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { Angabe, AngabenListe, ListDetail } from "../design/ListDetail";
import { ListRow } from "../design/ListRow";
import { Page } from "../design/Page";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SourceChip } from "../design/SourceChip";
import { formatCount } from "../format/count";
import { formatInstant } from "../format/date";
import { useT } from "../i18n";
import { Verlauf } from "./Verlauf";

/** What a row is called: its MaLo, or the process where it has none. */
const name = (p: ProcessRow) => p.malo_id ?? p.process_id;

/** The list pane's head: compact, so the list starts near the top. */
function Kopf() {
  const t = useT();
  return <h1 className="m-0 text-title2 font-semibold">{t.fristen.titel}</h1>;
}

/** The selected process: what it is, its Frist, and its Verlauf. */
function Detail({ p }: { p: ProcessRow }) {
  const t = useT();
  return (
    <article>
      <header className="flex flex-col gap-2 px-6 pt-6 pb-5">
        {/* Task first, as in Heute; what it is about on the line under it. */}
        <h2 className="m-0 text-title2 font-semibold">
          {t.quellen.fristOverdue(family(p.family))}
        </h2>
        <p className="m-0 flex flex-wrap items-center gap-x-3 gap-y-1 text-body text-text-2">
          <span>
            {name(p)} · {t.fristen.zeile(String(p.pid), zustand(p.state))}
          </span>
          {p.deadline_at && <FristUhr due={p.deadline_at} />}
        </p>
        {p.malo_id && (
          <Link
            href={`/prozess?key=${p.malo_id}`}
            className="focus-ring inline-flex w-fit items-center gap-1 rounded-[6px] text-body text-accent no-underline data-[hovered]:underline"
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
  return (
    <AngabenListe>
      <Angabe name={f.malo}>{p.malo_id}</Angabe>
      <Angabe name={f.pid}>{p.pid}</Angabe>
      <Angabe name={f.zustand}>{zustand(p.state)}</Angabe>
      <Angabe name={f.frist}>{p.deadline_at && t.zeit.uhr(formatInstant(p.deadline_at))}</Angabe>
      <Angabe name={f.fundstelle}>
        {p.deadline_source && <SourceChip quelle={{ dokument: p.deadline_source }} />}
      </Angabe>
      <Angabe name={f.begonnen}>{p.started_at && t.zeit.uhr(formatInstant(p.started_at))}</Angabe>
      <Angabe name={f.marktpartner}>{p.partner_mp_id}</Angabe>
      <Angabe name={f.familie}>{family(p.family)}</Angabe>
      <Angabe name={f.prozess}>{p.process_id}</Angabe>
    </AngabenListe>
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
      <Page width="reading" title={t.fristen.titel} subtitle={t.fristen.untertitel}>
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
        {overdue.isPending && <div className="skeleton h-24" />}
        {overdue.data && (
          <Panel>
            <EmptyState title={t.fristen.leer}>{t.fristen.leerText}</EmptyState>
          </Panel>
        )}
      </Page>
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
          title={name(p)}
          subtitle={t.fristen.zeile(String(p.pid), zustand(p.state))}
          trailing={p.deadline_at && <FristUhr due={p.deadline_at} compact />}
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
