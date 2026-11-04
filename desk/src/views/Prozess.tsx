import { useQuery } from "@tanstack/react-query";
import { useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { Button as AriaButton, Form } from "react-aria-components";
import { keyKind } from "../app/CommandPalette";
import { nichtErreichbar } from "../app/dienste";
import { prozessTitel } from "../app/prozesse";
import { useRolle } from "../app/rollen";
import type { ProcessView } from "../data/gen/makod";
import { call, ServiceUnavailable, type ToolOutput, ToolRefusal } from "../data/mcp";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SearchField } from "../design/SearchField";
import { epochMs } from "../format/date";
import { Fragen } from "./Fragen";
import { Hinweise } from "./Hinweise";
import { Verlauf } from "./Verlauf";

type Processes = ToolOutput<"makod", "get_process">;

function Copy({ value, label }: { value: string; label: string }) {
  const [done, setDone] = useState(false);
  return (
    <AriaButton
      aria-label={`${label} kopieren`}
      onPress={() => {
        navigator.clipboard?.writeText(value).then(() => {
          setDone(true);
          setTimeout(() => setDone(false), 1500);
        });
      }}
      className="focus-ring inline-flex size-6 items-center justify-center rounded-[6px] text-text-2 data-[hovered]:bg-fill data-[hovered]:text-text"
    >
      <Icon name={done ? "check" : "copy"} size={14} />
    </AriaButton>
  );
}

function Fact({ term, value, copy }: { term: string; value: string; copy?: boolean }) {
  return (
    <div className="min-w-0">
      <dt className="text-caption font-medium text-text-2">{term}</dt>
      <dd className="m-0 mt-0.5 flex items-center gap-1">
        <span className="truncate font-mono text-body text-text" title={value}>
          {value}
        </span>
        {copy && <Copy value={value} label={term} />}
      </dd>
    </div>
  );
}

/** The earliest pending Frist of a process, or none. */
function firstDue(p: ProcessView): number {
  const due = p.deadlines.flatMap((d) => (d.due_at ? [epochMs(d.due_at)] : []));
  return due.length ? Math.min(...due) : Number.POSITIVE_INFINITY;
}

function ProcessCard({ process }: { process: ProcessView }) {
  const next = process.deadlines
    .flatMap((d) => (d.due_at ? [d.due_at] : []))
    .sort((a, b) => epochMs(a) - epochMs(b))[0];
  return (
    <Panel>
      <div className="flex items-start gap-3 px-6 pt-5 pb-4">
        <div className="min-w-0 flex-1">
          <h2 className="m-0 truncate text-title font-semibold">
            {prozessTitel(process.workflow_name)}
          </h2>
          <details className="group mt-1">
            <summary className="focus-ring inline-flex cursor-default list-none items-center gap-1 rounded-[6px] text-caption text-text-2 data-[hovered]:text-text [&::-webkit-details-marker]:hidden">
              <Icon
                name="chevron"
                size={12}
                className="transition-transform group-open:rotate-90"
              />
              Technische Angaben
            </summary>
            <dl className="m-0 mt-3 grid grid-cols-2 gap-4">
              <Fact term="Workflow" value={process.workflow_name} />
              <Fact term="Prozess-ID" value={process.process_id} copy />
              <Fact term="Stream" value={process.stream_id} copy />
            </dl>
          </details>
        </div>
        {next && <FristUhr due={next} />}
      </div>
      <Verlauf processId={process.process_id} deadlines={process.deadlines} />
    </Panel>
  );
}

function Result({ view }: { view: Processes }) {
  const ordered = [...view.processes].sort((a, b) => firstDue(a) - firstDue(b));
  const n = ordered.length;
  return (
    <div className="flex flex-col gap-4">
      <div className="flex items-baseline gap-3 px-1">
        <span className="font-mono text-title font-semibold">{view.business_key}</span>
        <Copy value={view.business_key} label="Geschäftsschlüssel" />
        <span className="text-callout text-text-2">{keyKind(view.business_key)}</span>
        <span className="flex-1" />
        <span className="text-callout text-text-2">{n === 1 ? "1 Prozess" : `${n} Prozesse`}</span>
      </div>
      <Hinweise nurFuer={[view.business_key]} />
      {keyKind(view.business_key) === "MaLo-ID" && <Fragen anker={{ malo: view.business_key }} />}
      {ordered.map((p) => (
        <ProcessCard key={p.stream_id} process={p} />
      ))}
    </div>
  );
}

function Loading() {
  return (
    <div role="status" aria-busy="true">
      <span className="sr-only">Lädt …</span>
      <Panel className="p-6">
        <div className="skeleton h-3 w-20" />
        <div className="skeleton mt-2 h-6 w-64" />
        <div className="mt-6 grid grid-cols-3 gap-4">
          <div className="skeleton h-9" />
          <div className="skeleton h-9" />
          <div className="skeleton h-9" />
        </div>
      </Panel>
    </div>
  );
}

function Failure({ error, businessKey }: { error: unknown; businessKey: string }) {
  if (error instanceof ToolRefusal) {
    return (
      <Notice tone="refusal" title={`Kein Prozess für „${businessKey}“`}>
        {error.message}
      </Notice>
    );
  }
  if (error instanceof ServiceUnavailable) {
    return (
      <Notice tone="failure" title={nichtErreichbar(error.service)}>
        {import.meta.env.DEV && (
          <>
            <code className="font-mono">just desk</code> startet sie mit dem Desk.{" "}
          </>
        )}
        ({error.message})
      </Notice>
    );
  }
  return (
    <Notice tone="failure" title="Die Antwort war unerwartet">
      {error instanceof Error ? error.message : String(error)}
    </Notice>
  );
}

/** Prozesse: open a running process by its business key. */
export function ProzessView() {
  const { key } = useSearch({ from: "/prozess" });
  const navigate = useNavigate({ from: "/prozess" });
  const { rolle } = useRolle();
  const [draft, setDraft] = useState(key ?? "");
  useEffect(() => setDraft(key ?? ""), [key]);

  const query = useQuery({
    queryKey: [rolle, "makod", "get_process", key],
    queryFn: () => call("makod", "get_process", { business_key: key ?? "" }),
    enabled: Boolean(key),
  });

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-6 px-8 py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">Prozesse</h1>
        <p className="m-0 mt-1 text-callout text-text-2">
          Prozesse über ihren Geschäftsschlüssel öffnen — MaLo-ID, MeLo-ID, Vorgangsnummer.
        </p>
      </header>

      <Form
        onSubmit={(e) => {
          e.preventDefault();
          const next = draft.trim();
          navigate({ search: next ? { key: next } : {} });
        }}
      >
        <SearchField
          aria-label="Geschäftsschlüssel"
          placeholder="MaLo-ID, MeLo-ID oder Vorgangsnummer"
          value={draft}
          onChange={setDraft}
          onClear={() => navigate({ search: {} })}
          autoFocus
        />
      </Form>

      {!key && (
        <EmptyState title="Welcher Prozess?">
          Geschäftsschlüssel eingeben und <Kbd>↵</Kbd> drücken — oder von überall <Kbd>⌘K</Kbd>.
        </EmptyState>
      )}
      {key && query.isPending && <Loading />}
      {key && query.isError && <Failure error={query.error} businessKey={key} />}
      {key && query.data && <Result view={query.data} />}
    </div>
  );
}
