import { type ReactNode, useState } from "react";
import { Switch } from "react-aria-components";
import { DIENST, dienstVon } from "../app/dienste";
import type { Bilanz, Faehigkeit } from "../data/agent";
import { Icon } from "../design/Icon";
import { EmptyState, Notice, Panel } from "../design/Panel";
import {
  entschieden,
  hilfreichText,
  vorschlagAufschluesselung,
  vorschlagText,
} from "../format/bilanz";
import { formatCount } from "../format/count";
import { useKatalog } from "./Bilanz";

function Merkmal({ ok, children }: { ok: boolean; children: ReactNode }) {
  return (
    <span
      className={`inline-flex h-6 items-center gap-1 rounded-full px-2.5 text-caption font-medium ${
        ok ? "bg-done-fill text-done" : "bg-fill text-text-2"
      }`}
    >
      {ok && <Icon name="check" size={12} />}
      {children}
    </span>
  );
}

/** A skill's reads grouped by the service they reach, each with its tools' reasons. */
function dienste(liest: Faehigkeit["liest"]): { dienst: string; wozu: string[] }[] {
  const by = new Map<string, string[]>();
  for (const l of liest) {
    const dienst = dienstVon(l.werkzeug);
    by.set(dienst, [...(by.get(dienst) ?? []), l.wozu ?? l.werkzeug]);
  }
  return [...by].map(([dienst, wozu]) => ({ dienst, wozu }));
}

/** `anthropic/claude-sonnet-5` → `claude-sonnet-5 · anthropic`, as configured. */
function modell(m: string | null): string {
  if (!m) return "regelbasiert, kein Modell";
  const [anbieter, name] = m.split("/");
  return name ? `${name} · ${anbieter}` : m;
}

/**
 * What the skill's runs came to in agentd's window: how many ran, how often a
 * person found the answer „hilfreich", what became of its proposals — the
 * record a person weighs a proposal by, as agentd counted it.
 */
function Record({ b }: { b: Bilanz }) {
  const n = entschieden(b.vorschlag);
  return (
    <section aria-label={`Bilanz der letzten ${formatCount(b.tage)} Tage`}>
      <h3 className="m-0 mb-1.5 text-caption font-semibold tracking-wide text-text-2 uppercase">
        In {formatCount(b.tage)} Tagen
      </h3>
      <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-body">
        <dt className="text-text-2">Läufe</dt>
        <dd className="m-0 text-text tabular-nums">
          {formatCount(b.laeufe)}
          {b.offen > 0 && <span className="text-text-2"> · {formatCount(b.offen)} laufen</span>}
        </dd>
        <dt className="text-text-2">Hilfreich</dt>
        <dd className="m-0 text-text">{hilfreichText(b.hilfreich)}</dd>
        <dt className="text-text-2">Vorschläge</dt>
        <dd className="m-0 text-text">
          {vorschlagText(b.vorschlag)}
          {n > 0 && (
            <span className="block text-caption text-text-2">
              {vorschlagAufschluesselung(b.vorschlag)}
            </span>
          )}
        </dd>
      </dl>
    </section>
  );
}

function Karte({ f }: { f: Faehigkeit }) {
  return (
    <Panel className={`flex flex-col gap-3 p-5 ${f.aktiv ? "" : "opacity-70"}`}>
      <header>
        <h2 className="m-0 text-callout font-semibold text-text">{f.titel ?? f.name}</h2>
        {f.zweck && <p className="m-0 mt-1 text-body text-text-2">{f.zweck}</p>}
      </header>
      <div className="flex flex-wrap gap-1.5">
        <Merkmal ok={f.aktiv}>{f.aktiv ? "aktiv" : "nicht aktiviert"}</Merkmal>
        {f.nur_lesend && <Merkmal ok>liest nur</Merkmal>}
        <Merkmal ok={false}>
          {f.ausloeser.length === 0
            ? "startet auf Ihre Frage"
            : `startet bei ${formatCount(f.ausloeser.length)} Ereignisart${f.ausloeser.length === 1 ? "" : "en"}`}
        </Merkmal>
      </div>
      <dl className="m-0 grid grid-cols-[auto_1fr] gap-x-4 gap-y-1.5 text-body">
        <dt className="text-text-2">Modell</dt>
        <dd className="m-0 font-mono text-text">{modell(f.modell)}</dd>
        <dt className="text-text-2">Liest</dt>
        <dd className="m-0 flex flex-wrap gap-1.5">
          {f.liest.length === 0 ? (
            <span className="text-text-2">nichts</span>
          ) : (
            dienste(f.liest).map(({ dienst, wozu }) => (
              <span
                key={dienst}
                title={wozu.join("\n")}
                className="inline-flex h-6 items-center gap-1 rounded-full border border-separator px-2 text-caption text-text"
              >
                {dienst}
                {wozu.length > 1 && <span className="text-text-2">{formatCount(wozu.length)}</span>}
              </span>
            ))
          )}
        </dd>
        {f.mit_freigabe > 0 && (
          <>
            <dt className="text-text-2">Freigabe</dt>
            <dd className="m-0 text-text">
              {formatCount(f.mit_freigabe)} Werkzeug{f.mit_freigabe === 1 ? "" : "e"} nur nach
              Freigabe
            </dd>
          </>
        )}
      </dl>
      {f.bilanz && (
        <div className="border-t border-separator pt-3">
          <Record b={f.bilanz} />
        </div>
      )}
      <p className="m-0 mt-auto font-mono text-caption text-text-2">
        {f.name} · v{f.version ?? "?"}
      </p>
    </Panel>
  );
}

/**
 * Assistent: the Beipackzettel of every skill — what it is for, what it may
 * read, where its model runs, whether it can change anything, and how its
 * last 30 days went — read off the manifests agentd runs and the record it
 * keeps, so the page cannot disagree with them.
 */
export function AssistentView() {
  const [nurAktive, setNurAktive] = useState(true);
  const list = useKatalog();
  const shown = (list.data ?? []).filter((f) => !nurAktive || f.aktiv);
  const alleLesen = (list.data ?? []).every((f) => f.nur_lesend);

  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-6 px-8 py-10">
      <header className="flex items-end gap-4">
        <div className="flex-1">
          <h1 className="m-0 text-large font-bold tracking-tight">Assistent</h1>
          <p className="m-0 mt-1 text-callout text-text-2">
            Was jede Fähigkeit liest, womit sie denkt, was sie tun darf und wie oft sie half — aus
            ihrer geprüften Beschreibung und dem, was der {DIENST.agentd} über ihre Läufe festhält.
          </p>
        </div>
        <Switch
          isSelected={nurAktive}
          onChange={setNurAktive}
          className="group flex cursor-default items-center gap-2 text-body text-text"
        >
          <span className="flex h-5 w-9 items-center rounded-full bg-fill-strong p-0.5 transition-colors group-data-[selected]:bg-accent">
            <span className="size-4 rounded-full bg-surface shadow transition-transform group-data-[selected]:translate-x-4" />
          </span>
          Nur aktive
        </Switch>
      </header>

      {list.data && alleLesen && (
        <Notice tone="info" title="Der Assistent liest nur.">
          Keine Fähigkeit kann eine Marktnachricht senden, buchen oder Stammdaten ändern — was er
          vorschlägt, entscheidet und sendet ein Mensch.
        </Notice>
      )}
      {list.isError && (
        <Notice tone="failure" title="Assistent ist nicht erreichbar">
          {list.error.message}
        </Notice>
      )}
      {list.data && shown.length === 0 && (
        <EmptyState title="Keine Fähigkeit ist aktiviert.">
          Der Betrieb aktiviert sie in der Konfiguration des Assistenten.
        </EmptyState>
      )}
      <div className="grid grid-cols-[repeat(auto-fill,minmax(300px,1fr))] gap-3">
        {shown.map((f) => (
          <Karte key={f.name} f={f} />
        ))}
      </div>
    </div>
  );
}
