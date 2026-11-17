import { useQuery } from "@tanstack/react-query";
import { Link, useNavigate, useSearch } from "@tanstack/react-router";
import { useEffect, useRef, useState } from "react";
import { Form } from "react-aria-components";
import { keyKind } from "../app/CommandPalette";
import { nichtErreichbar } from "../app/dienste";
import { prozessTitel } from "../app/prozesse";
import { useRolle } from "../app/rollen";
import type { ProcessView } from "../data/gen/makod";
import { call, ServiceUnavailable, type ToolOutput, ToolRefusal } from "../data/mcp";
import { Button } from "../design/Button";
import { FristUhr } from "../design/FristUhr";
import { Kbd } from "../design/Kbd";
import { Page } from "../design/Page";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { Refusal } from "../design/Refusal";
import { SearchField } from "../design/SearchField";
import { epochMs } from "../format/date";
import { useT } from "../i18n";
import { Fragen } from "./Fragen";
import { Hinweise } from "./Hinweise";
import { ProzessAkte } from "./ProzessAkte";

type Processes = ToolOutput<"makod", "get_process">;

/** The recipe that starts the services beside the Desk in development. */
const DEV_BEFEHL = "just dev";

/** The earliest pending Frist of a process, or none. */
function firstDue(p: ProcessView): number {
  const due = p.deadlines.flatMap((d) => (d.due_at ? [epochMs(d.due_at)] : []));
  return due.length ? Math.min(...due) : Number.POSITIVE_INFINITY;
}

/** Several processes under one key: one link each, the earliest Frist first. */
function Auswahl({
  businessKey,
  ordered,
  selected,
}: {
  businessKey: string;
  ordered: ProcessView[];
  selected: string;
}) {
  const t = useT();
  const n = ordered.length;
  return (
    <nav aria-label={t.prozessAnsicht.unter(n, `${n}`, businessKey)}>
      <ul className="m-0 flex list-none flex-wrap gap-2 p-0">
        {ordered.map((p) => {
          const next = p.deadlines
            .flatMap((d) => (d.due_at ? [d.due_at] : []))
            .sort((a, b) => epochMs(a) - epochMs(b))[0];
          const current = p.process_id === selected;
          return (
            <li key={p.stream_id}>
              <Link
                to="/prozess"
                search={{ key: businessKey, p: p.process_id }}
                {...(current ? { "aria-current": "page" as const } : {})}
                className={`focus-ring inline-flex h-8 items-center gap-2 rounded-full border px-3 text-body no-underline ${
                  current
                    ? "border-accent bg-[color-mix(in_srgb,var(--accent)_10%,transparent)] text-text"
                    : "border-separator text-text hover:bg-fill"
                }`}
              >
                <span>{prozessTitel(p.workflow_name)}</span>
                {next && <FristUhr due={next} />}
              </Link>
            </li>
          );
        })}
      </ul>
    </nav>
  );
}

function Result({ view, selected }: { view: Processes; selected: string | undefined }) {
  const ordered = [...view.processes].sort((a, b) => firstDue(a) - firstDue(b));
  const process = ordered.find((p) => p.process_id === selected) ?? ordered[0];
  return (
    <div className="flex flex-col gap-4">
      {ordered.length > 1 && process && (
        <Auswahl businessKey={view.business_key} ordered={ordered} selected={process.process_id} />
      )}
      {process && (
        <ProzessAkte key={process.process_id} businessKey={view.business_key} process={process} />
      )}
      {/* The assistant follows the work, never above it. */}
      <Hinweise nurFuer={[view.business_key]} />
      {keyKind(view.business_key) === "MaLo-ID" && <Fragen anker={{ malo: view.business_key }} />}
    </div>
  );
}

function Loading() {
  const t = useT();
  return (
    <div role="status" aria-busy="true">
      <span className="sr-only">{t.prozessAnsicht.laedt}</span>
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

function Failure({
  error,
  businessKey,
  onNeu,
}: {
  error: unknown;
  businessKey: string;
  onNeu: () => void;
}) {
  const t = useT();
  const k = t.prozessAnsicht;
  if (error instanceof ToolRefusal) {
    return (
      <Refusal
        title={k.keiner(businessKey)}
        raw={error.message}
        action={
          <Button variant="primary" onPress={onNeu}>
            {t.refusal.newSearch}
          </Button>
        }
      />
    );
  }
  if (error instanceof ServiceUnavailable) {
    return (
      <Notice tone="failure" title={nichtErreichbar(error.service)}>
        {import.meta.env.DEV && (
          <>
            <code className="font-mono">{DEV_BEFEHL}</code> {k.devStart}{" "}
          </>
        )}
        ({error.message})
      </Notice>
    );
  }
  return (
    <Notice tone="failure" title={k.unerwartet}>
      {error instanceof Error ? error.message : String(error)}
    </Notice>
  );
}

/** Prozesse: open a running process by its business key. */
export function ProzessView() {
  const t = useT();
  const k = t.prozessAnsicht;
  const { key, p } = useSearch({ from: "/prozess" });
  const navigate = useNavigate({ from: "/prozess" });
  const { rolle } = useRolle();
  const [draft, setDraft] = useState(key ?? "");
  const feld = useRef<HTMLInputElement>(null);
  useEffect(() => setDraft(key ?? ""), [key]);

  const query = useQuery({
    queryKey: [rolle, "makod", "get_process", key],
    queryFn: () => call("makod", "get_process", { business_key: key ?? "" }),
    enabled: Boolean(key),
  });

  // A resolved key is a record: the process brings its own title and crumbs.
  if (key && query.data && query.data.processes.length > 0) {
    return (
      <Page width="work">
        <Result view={query.data} selected={p} />
      </Page>
    );
  }

  return (
    <Page width="reading" title={k.titel} subtitle={k.untertitel}>
      <Form
        onSubmit={(e) => {
          e.preventDefault();
          const next = draft.trim();
          navigate({ search: next ? { key: next } : {} });
        }}
      >
        <SearchField
          inputRef={feld}
          aria-label={k.schluessel}
          placeholder={k.platzhalter}
          value={draft}
          onChange={setDraft}
          onClear={() => navigate({ search: {} })}
          autoFocus
        />
      </Form>

      {!key && (
        <EmptyState title={k.welcher}>
          {k.eingebenVor} <Kbd>↵</Kbd> {k.eingebenNach} <Kbd>⌘K</Kbd>.
        </EmptyState>
      )}
      {key && query.isPending && <Loading />}
      {key && query.isError && (
        <Failure
          error={query.error}
          businessKey={key}
          onNeu={() => {
            navigate({ search: {} });
            setDraft("");
            feld.current?.focus();
          }}
        />
      )}
      {key && query.data && query.data.processes.length === 0 && (
        <EmptyState title={k.keiner(key)} />
      )}
    </Page>
  );
}
