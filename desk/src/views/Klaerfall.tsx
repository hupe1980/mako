import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useId, useState } from "react";
import { Label, Radio, RadioGroup, TextArea, TextField } from "react-aria-components";
import { klaerfallHerkunft } from "../app/codes";
import { nichtErreichbar } from "../app/dienste";
import { useHold } from "../app/Holds";
import { useImDrift } from "../app/imDrift";
import { useRolle } from "../app/rollen";
import type { Klaerfall } from "../data/gen/processd";
import { KlaerfallAbgelehnt, type KlaerfallTuer, klaerfallSenden } from "../data/klaerfall";
import { call, ServiceUnavailable } from "../data/mcp";
import { Button } from "../design/Button";
import { FristUhr } from "../design/FristUhr";
import { Page } from "../design/Page";
import { Notice, Panel } from "../design/Panel";
import { Refusal } from "../design/Refusal";
import { SourceChip } from "../design/SourceChip";
import { formatInstant } from "../format/date";
import { eintrag, useT } from "../i18n";

/** The bar's one decision: send the correction, or — where processd proposes none — close or take the case. */
function Entscheidung({ k: fall }: { k: Klaerfall }) {
  const t = useT();
  const k = t.klaerfall;
  const id = useId();
  const client = useQueryClient();
  const hold = useHold();
  const drift = useImDrift("processd");
  const [befehl, setBefehl] = useState(fall.korrektur?.befehl ?? null);
  const [begruendung, setBegruendung] = useState("");
  const [gehalten, setGehalten] = useState(false);
  const [fehler, setFehler] = useState<unknown>(null);
  const anderer = fall.korrektur !== null && befehl !== fall.korrektur.befehl;

  function senden(tuer: KlaerfallTuer, body: Record<string, unknown>, label: string) {
    setFehler(null);
    setGehalten(true);
    hold(
      `klaerfall/${fall.id}`,
      label,
      async (v) => {
        try {
          await klaerfallSenden(fall.id, tuer, body, v);
          void client.invalidateQueries();
        } catch (e) {
          setFehler(e);
          setGehalten(false);
        }
      },
      () => setGehalten(false),
    );
  }

  const codeSatz =
    fehler instanceof KlaerfallAbgelehnt && fehler.code
      ? t.codes[fehler.code as keyof typeof t.codes]
      : undefined;
  return (
    <section
      aria-labelledby={id}
      className="squircle flex flex-col gap-3 rounded-panel border border-[color-mix(in_srgb,var(--accent)_30%,transparent)] bg-[color-mix(in_srgb,var(--accent)_7%,var(--surface))] px-6 py-4"
    >
      <div className="flex flex-wrap items-center gap-4">
        <div className="min-w-0 flex-1">
          <h2 id={id} className="m-0 text-headline">
            {t.prozessAnsicht.entscheidung}
          </h2>
          <p className="m-0 mt-1 text-body text-text-2">
            {eintrag(t.values.klaerfallAktion, fall.aktion)} — <span lang="en">{fall.hinweis}</span>
          </p>
        </div>
        {fall.due_at && <FristUhr due={fall.due_at} />}
        {drift ? (
          <p className="m-0 text-body text-text-2">
            {t.drift.keineEntscheidung(t.dienstNamen.processd)}
          </p>
        ) : gehalten ? (
          <span className="text-body text-text-2">{k.gehalten}</span>
        ) : fall.korrektur ? (
          <Button
            variant="primary"
            isDisabled={anderer && !begruendung.trim()}
            onPress={() =>
              senden(
                "dispatch",
                anderer ? { befehl, begruendung: begruendung.trim() } : {},
                k.gesendet,
              )
            }
          >
            {k.korrigieren}
          </Button>
        ) : (
          <>
            <Button onPress={() => senden("assign", {}, k.zugewiesen)}>{k.zuweisen}</Button>
            <Button
              variant="primary"
              isDisabled={!begruendung.trim()}
              onPress={() => senden("resolve", { begruendung: begruendung.trim() }, k.erledigt)}
            >
              {k.erledigtKnopf}
            </Button>
          </>
        )}
      </div>
      {fall.korrektur && fall.korrektur.erlaubt.length > 1 && !gehalten && (
        <RadioGroup
          aria-label={k.befehl}
          value={befehl}
          onChange={setBefehl}
          orientation="horizontal"
          className="flex flex-wrap gap-2"
        >
          {fall.korrektur.erlaubt.map((b) => (
            <Radio
              key={b}
              value={b}
              className="focus-ring flex h-8 cursor-default items-center rounded-control border border-separator px-3 font-mono text-callout text-text-2 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:text-accent"
            >
              {b}
            </Radio>
          ))}
        </RadioGroup>
      )}
      {(anderer || !fall.korrektur) && !gehalten && (
        <TextField value={begruendung} onChange={setBegruendung} className="flex flex-col gap-1">
          <Label className="text-caption text-text-2">
            {anderer ? k.begruendungAnderer : k.begruendung}
          </Label>
          <TextArea
            rows={2}
            maxLength={512}
            className="focus-ring rounded-control border border-separator bg-surface px-3 py-2 text-body text-text outline-none"
          />
        </TextField>
      )}
      {fehler !== null && (
        <Notice tone="refusal" title={codeSatz ?? k.abgelehnt}>
          {fehler instanceof Error ? fehler.message : nichtErreichbar("processd")}
        </Notice>
      )}
    </section>
  );
}

/**
 * A Klärfall: the cause in the words of the list that defines its code,
 * where it came from, when it is due, what happened to it — and the one
 * decision processd allows on it. Every word about the case is processd's.
 */
export function KlaerfallView() {
  const t = useT();
  const k = t.klaerfall;
  const { id } = useParams({ from: "/klaerfall/$id" });
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "processd", "get_klaerfall", id],
    queryFn: () => call("processd", "get_klaerfall", { id }),
  });
  const verlaufId = useId();
  const f = q.data?.klaerfall;
  return (
    <Page width="work">
      {q.isPending && <div className="skeleton h-40" />}
      {q.isError &&
        (q.error instanceof ServiceUnavailable ? (
          <Notice tone="failure" title={nichtErreichbar("processd")}>
            {q.error.message}
          </Notice>
        ) : (
          <Refusal title={k.keiner} raw={q.error.message} />
        ))}
      {f && q.data && (
        <>
          <Panel className="px-6 pt-5 pb-5">
            <nav aria-label={t.prozessAnsicht.pfad} className="text-caption text-text-2">
              <Link
                to="/heute"
                search={{ quelle: "klaerfaelle" }}
                className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
              >
                {t.quellen.klaerfaelle}
              </Link>
              <span aria-hidden="true" className="mx-1.5">
                /
              </span>
              <span>{f.message_ref}</span>
            </nav>
            <div className="flex flex-wrap items-start gap-6">
              <div className="min-w-0 flex-1">
                <h1 className="m-0 mt-1 text-large-title font-semibold">
                  {f.bedeutung ?? t.quellen.ursacheOhneWortlaut(f.ursache_code, f.ursache_liste)}
                  <span
                    className={`ml-3 inline-flex h-6 items-center rounded-full px-2.5 align-middle text-body font-semibold ${f.zustand === "open" ? "bg-info-fill text-info" : "bg-done-fill text-done"}`}
                  >
                    {eintrag(k.zustand, f.zustand)}
                  </span>
                </h1>
                <ul
                  aria-label={t.prozessAnsicht.eckdaten}
                  className="m-0 mt-1.5 flex list-none flex-wrap gap-x-3 p-0 text-body"
                >
                  <li>
                    {f.ursache_code} · <SourceChip quelle={{ dokument: f.ursache_liste }} />
                  </li>
                  {f.pid && <li>PID {f.pid}</li>}
                  <li>{f.origins.map(klaerfallHerkunft).join(", ")}</li>
                  {f.occurrences > 1 && <li>{t.quellen.mal(`${f.occurrences}`)}</li>}
                  {f.process_id && (
                    <li>
                      <Link
                        to="/prozess"
                        search={{ key: f.process_id }}
                        className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
                      >
                        {k.prozess}
                      </Link>
                    </li>
                  )}
                  {f.assignee && <li>{k.bei(f.assignee)}</li>}
                </ul>
              </div>
              {f.due_at && (
                // biome-ignore lint/a11y/useSemanticElements: a labelled figure and its caption
                <div
                  role="group"
                  aria-label={t.prozessAnsicht.naechsteFrist}
                  className="flex flex-col items-end gap-1"
                >
                  <FristUhr due={f.due_at} gross />
                  {f.due_source && <SourceChip quelle={{ dokument: f.due_source }} />}
                </div>
              )}
            </div>
          </Panel>
          {f.zustand === "open" && <Entscheidung k={f} />}
          <Panel className="overflow-hidden">
            <section aria-labelledby={verlaufId}>
              <h2 id={verlaufId} className="m-0 px-6 pt-5 pb-3 text-headline">
                {t.verlauf.titel}
              </h2>
              <ol aria-label={t.verlauf.titel} className="m-0 list-none p-0">
                {q.data.verlauf.map((v) => (
                  <li
                    key={`${v.at}${v.schritt}`}
                    className="grid grid-cols-[10rem_minmax(0,1fr)] gap-3 border-t border-separator px-6 py-3 text-body"
                  >
                    <span className="text-caption text-text-2 tabular-nums">
                      {formatInstant(v.at)}
                    </span>
                    <span>
                      <span className="font-semibold">{eintrag(k.schritt, v.schritt)}</span>
                      <span className="ml-2 text-caption text-text-2">
                        {eintrag(t.verlauf.akteur, v.akteur)}
                      </span>
                      {(v.befehl ?? v.vorgeschlagen) && (
                        <span className="block font-mono text-callout text-text-2">
                          {v.befehl ?? v.vorgeschlagen}
                        </span>
                      )}
                      {v.begruendung && <span className="block text-text">{v.begruendung}</span>}
                    </span>
                  </li>
                ))}
              </ol>
            </section>
          </Panel>
        </>
      )}
    </Page>
  );
}
