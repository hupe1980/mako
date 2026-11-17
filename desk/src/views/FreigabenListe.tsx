// The Freigaben view: the queue processd holds for a person, each opened
// with its decision (`Entscheiden`, in Freigaben.tsx, which Heute and the
// Prozess page share). Loaded when first opened, so the first paint does not
// carry the list or the Verlauf.

import { useQueryClient } from "@tanstack/react-query";
import { useRouter, useSearch } from "@tanstack/react-router";
import { useEffect, useState } from "react";
import { Button as AriaButton, GridList, GridListItem } from "react-aria-components";
import { fokussieren } from "../app/aktionen";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { Entscheidungsstand } from "../app/Entscheidungsstand";
import { NAECHSTE, VORIGE } from "../app/keys";
import type { Freigabe } from "../data/gen/processd";
import { ServiceUnavailable } from "../data/mcp";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { Page } from "../design/Page";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { formatCount } from "../format/count";
import { eintrag, useT } from "../i18n";
import { useVorschlaege } from "../inbox/quellen";
import { Entscheiden, ROW, useFreigaben } from "./Freigaben";
import { Verlauf } from "./Verlauf";

function Detail({ f }: { f: Freigabe }) {
  const t = useT();
  const k = t.freigabenAnsicht;
  const client = useQueryClient();
  const router = useRouter();
  const [erledigt, setErledigt] = useState(false);
  if (erledigt) {
    return (
      <Panel>
        <EmptyState title={k.entschieden}>
          {k.entschiedenText(DIENST.processd, DIENST.makod)}
        </EmptyState>
      </Panel>
    );
  }
  return (
    <div className="flex flex-col gap-4">
      <Panel className="flex flex-col gap-3 p-5">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <p className="m-0 text-caption font-semibold text-text-2">
              {eintrag(k.art, f.art)} · PID <span>{f.pid}</span>
            </p>
            <h2 className="m-0 mt-1 text-title2 font-semibold">{f.grund}</h2>
          </div>
          <span className="flex flex-col items-end gap-1 text-caption text-text-2">
            {k.entscheidenBis}
            <span className="flex items-center gap-2">
              <Entscheidungsstand ding={{ art: "freigabe", f }} fehlschlag={false} />
              <FristUhr due={f.expires_at} />
            </span>
          </span>
        </div>
        {f.malo_id && (
          <AriaButton
            onPress={() => router.navigate({ to: "/prozess", search: { key: f.malo_id ?? "" } })}
            className="focus-ring inline-flex h-7 w-fit items-center gap-1 rounded-full border border-separator px-3 text-body text-text data-[hovered]:bg-fill"
          >
            MaLo <span>{f.malo_id}</span>
            <Icon name="chevron" size={12} />
          </AriaButton>
        )}
      </Panel>
      <Entscheiden
        f={f}
        onDone={() => {
          setErledigt(true);
          void client.invalidateQueries({ queryKey: ["processd", "list_pending_approvals"] });
          void client.invalidateQueries();
        }}
      />
      <Panel className="overflow-hidden">
        <Verlauf processId={f.process_id} deadlines={[]} />
      </Panel>
    </div>
  );
}

/**
 * Freigaben: the decisions processd held for a person — the earliest Frist
 * first, as processd orders them. Each opens with its facts, the codes its
 * Entscheidungsbaum allows, the process's Verlauf and the assistant; a
 * decision is held for undo before it is sent, the market message included.
 */
export function FreigabenView() {
  const t = useT();
  const k = t.freigabenAnsicht;
  const router = useRouter();
  const { id } = useSearch({ from: "/freigaben" });
  const list = useFreigaben();
  const rows = list.data?.freigaben ?? [];
  const selected = rows.find((f) => f.id === id) ?? rows[0];
  const vorschlaege = useVorschlaege({ freigabe: rows.slice(0, 100).map((f) => f.id) }).freigabe;

  // The selected Freigabe is what ⌘K opens on.
  useEffect(() => {
    fokussieren(
      selected
        ? {
            titel: selected.malo_id ?? k.prozess(selected.process_id),
            kennung: true,
            grund: selected.grund,
            frist: selected.expires_at,
            ding: { art: "freigabe", f: selected },
          }
        : null,
    );
  });
  useEffect(() => () => fokussieren(null), []);

  // J and K walk the queue in processd's order.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const step = NAECHSTE.match?.(e) ? 1 : VORIGE.match?.(e) ? -1 : 0;
      if (!step || rows.length === 0) return;
      e.preventDefault();
      const at = Math.max(
        0,
        rows.findIndex((f) => f.id === selected?.id),
      );
      const next = rows[Math.min(rows.length - 1, Math.max(0, at + step))];
      if (next) router.navigate({ to: "/freigaben", search: { id: next.id } });
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [rows, selected?.id, router]);

  return (
    <Page width="work" title={k.titel} subtitle={k.untertitel(DIENST.processd)}>
      {list.isError && (
        <Notice
          tone="failure"
          title={
            list.error instanceof ServiceUnavailable
              ? nichtErreichbar(list.error.service)
              : k.fehler
          }
        >
          {list.error.message}
        </Notice>
      )}
      {list.isPending && <div className="skeleton h-40" />}
      {list.data && rows.length === 0 && (
        <Panel>
          <EmptyState title={k.leer}>{k.leerText(DIENST.processd)}</EmptyState>
        </Panel>
      )}
      {selected && (
        <div className="grid grid-cols-1 gap-6 lg:grid-cols-[minmax(0,22rem)_minmax(0,1fr)]">
          <section
            aria-label={k.offene}
            className="flex flex-col gap-2 lg:sticky lg:top-20 lg:self-start"
          >
            <Panel className="overflow-hidden">
              <GridList
                aria-label={k.offene}
                selectionMode="single"
                disallowEmptySelection
                selectedKeys={[selected.id]}
                onSelectionChange={(keys) => {
                  const next = [...keys][0];
                  if (next) router.navigate({ to: "/freigaben", search: { id: String(next) } });
                }}
                className="outline-none"
              >
                {rows.map((f) => (
                  <GridListItem key={f.id} id={f.id} textValue={f.grund} className={ROW}>
                    <span className="min-w-0 flex-1">
                      <span className="block truncate text-body text-text">{f.grund}</span>
                      <span className="block text-caption text-text-2">
                        {eintrag(k.art, f.art)} · PID <span>{f.pid}</span>
                        {f.malo_id && (
                          <>
                            {" · "}
                            <span>{f.malo_id}</span>
                          </>
                        )}
                      </span>
                    </span>
                    {vorschlaege.get(f.id)?.vorschlag && (
                      <span title={k.vorschlagBereitTitel} className="text-ai">
                        <Icon name="sparkle" size={14} />
                        <span className="sr-only">{k.vorschlagBereit}</span>
                      </span>
                    )}
                    <FristUhr due={f.expires_at} />
                  </GridListItem>
                ))}
              </GridList>
            </Panel>
            {list.data?.saturated && (
              <p className="m-0 px-1 text-caption text-text-2">
                {k.ersten(formatCount(rows.length))}
              </p>
            )}
          </section>
          <Detail key={selected.id} f={selected} />
        </div>
      )}
    </Page>
  );
}
