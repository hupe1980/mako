import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useParams } from "@tanstack/react-router";
import { useEffect, useId, useRef, useState } from "react";
import { Label, Radio, RadioGroup, TextArea, TextField } from "react-aria-components";
import { klaerfallAktion, klaerfallHerkunft } from "../app/codes";
import { nichtErreichbar } from "../app/dienste";
import { useGehalten, useHold } from "../app/Holds";
import { useImDrift } from "../app/imDrift";
import { rollenName, useRolle } from "../app/rollen";
import type { Klaerfall } from "../data/gen/processd";
import { KlaerfallAbgelehnt, type KlaerfallTuer, klaerfallSenden } from "../data/klaerfall";
import { call, ServiceUnavailable } from "../data/mcp";
import { Verweigert } from "../data/verweigert";
import { Button } from "../design/Button";
import { FristUhr } from "../design/FristUhr";
import { Page } from "../design/Page";
import { Notice, Panel } from "../design/Panel";
import { Refusal } from "../design/Refusal";
import { SourceChip } from "../design/SourceChip";
import { formatInstant } from "../format/date";
import { eintrag, useT } from "../i18n";

/** A lost reply or an intermediary failure cannot attest to the write's outcome. */
function ausgangUnklar(error: unknown): boolean {
  return (
    !(error instanceof Verweigert) &&
    (!(error instanceof KlaerfallAbgelehnt) || error.status >= 500 || error.status === 408)
  );
}

/** The bar's one decision: send the correction, or — where processd proposes none — close or take the case. */
function Entscheidung({
  k: fall,
  geschlossen,
  onGeschlossen,
}: {
  k: Klaerfall;
  geschlossen: boolean;
  onGeschlossen: () => void;
}) {
  const t = useT();
  const k = t.klaerfall;
  const id = useId();
  const client = useQueryClient();
  const hold = useHold();
  const { rolle } = useRolle();
  const drift = useImDrift("processd");
  const [befehl, setBefehl] = useState(fall.korrektur?.befehl ?? null);
  const [begruendung, setBegruendung] = useState("");
  const key = `klaerfall/${fall.id}`;
  const gehalten = useGehalten(key);
  const [stand, setStand] = useState<"bereit" | "undo" | "sendet" | KlaerfallTuer>("bereit");
  const [fehler, setFehler] = useState<unknown>(null);
  const ursprung = useRef<KlaerfallTuer>("dispatch");
  const abschnitt = useRef<HTMLElement>(null);
  const anderer = fall.korrektur !== null && befehl !== fall.korrektur.befehl;
  const offen = fall.zustand === "open" && !geschlossen;
  const unklar = fehler !== null && ausgangUnklar(fehler);
  const entscheidbar = offen && !unklar;
  const wartet =
    Boolean(gehalten) || stand === "sendet" || stand === "dispatch" || stand === "resolve";

  useEffect(() => {
    if (stand === "undo")
      abschnitt.current?.querySelector<HTMLElement>(`[data-tuer="${ursprung.current}"]`)?.focus();
  }, [stand]);

  function senden(tuer: KlaerfallTuer, body: Record<string, unknown>) {
    setFehler(null);
    setStand("bereit");
    ursprung.current = tuer;
    hold(
      key,
      k.vorgemerkt,
      async (v) => {
        setStand("sendet");
        try {
          await klaerfallSenden(fall.id, tuer, body, v);
          setStand(tuer);
          void client.invalidateQueries();
        } catch (e) {
          if (e instanceof KlaerfallAbgelehnt && e.code === "case_closed") {
            onGeschlossen();
            void client.invalidateQueries({
              queryKey: [rolle, "processd", "get_klaerfall", fall.id],
            });
          }
          setFehler(e);
          setStand("bereit");
          if (ausgangUnklar(e)) throw e;
        }
      },
      () => setStand("undo"),
      rolle,
    );
  }

  const ausgang = {
    bereit: "",
    undo: k.zurueckgenommen,
    sendet: k.uebermittlung,
    dispatch: fall.zustand === "dispatched" ? k.gesendet : k.uebermittelt,
    resolve: fall.zustand === "resolved" ? k.erledigt : k.uebermittelt,
    assign: k.uebermittelt,
  }[stand];

  function befehlName(command: string, index: number): string {
    const verb = command.split(".").at(-1) ?? "";
    const name = eintrag(k.antwort, verb, `${k.alternative} ${index + 1}`);
    return command === fall.korrektur?.befehl ? `${name} (${k.vorgeschlagen})` : name;
  }
  if (!offen && !ausgang && fehler === null) return null;
  return (
    <section
      ref={abschnitt}
      aria-labelledby={id}
      className="squircle flex flex-col gap-3 rounded-panel border border-[color-mix(in_srgb,var(--accent)_30%,transparent)] bg-[color-mix(in_srgb,var(--accent)_7%,var(--surface))] px-6 py-4"
    >
      <div className="flex flex-col items-start gap-3 sm:flex-row sm:flex-wrap sm:items-center">
        <div className="min-w-0 flex-1">
          <h2 id={id} className="m-0 text-headline">
            {wartet || unklar
              ? t.entscheidung.titel
              : entscheidbar
                ? t.prozessAnsicht.entscheidung
                : k.ergebnis}
          </h2>
          {entscheidbar &&
            (fall.aktion !== "retry_with_correction" || !fall.korrektur || wartet || drift) && (
              <p className="m-0 mt-1 text-body text-text-2">{klaerfallAktion(fall.aktion)}</p>
            )}
        </div>
        {!entscheidbar ? null : drift ? (
          <p className="m-0 text-body text-text-2">
            {t.drift.keineEntscheidung(t.dienstNamen.processd)}
          </p>
        ) : wartet ? null : fall.korrektur ? (
          <Button
            variant="primary"
            data-tuer="dispatch"
            isDisabled={anderer && !begruendung.trim()}
            onPress={() =>
              senden("dispatch", anderer ? { befehl, begruendung: begruendung.trim() } : {})
            }
          >
            {k.korrigieren}
          </Button>
        ) : (
          <>
            <Button data-tuer="assign" onPress={() => senden("assign", {})}>
              {k.zuweisen}
            </Button>
            <Button
              variant="primary"
              data-tuer="resolve"
              isDisabled={!begruendung.trim()}
              onPress={() => senden("resolve", { begruendung: begruendung.trim() })}
            >
              {k.erledigtKnopf}
            </Button>
          </>
        )}
      </div>
      {entscheidbar && fall.korrektur && fall.korrektur.erlaubt.length > 1 && !wartet && (
        <RadioGroup
          aria-label={k.befehl}
          value={befehl}
          onChange={setBefehl}
          orientation="horizontal"
          className="flex flex-wrap gap-2"
        >
          {fall.korrektur.erlaubt.map((b, index) => (
            <Radio
              key={b}
              value={b}
              className="focus-ring flex h-8 cursor-default items-center rounded-control border border-separator px-3 text-callout text-text-2 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:text-accent"
            >
              {befehlName(b, index)}
            </Radio>
          ))}
        </RadioGroup>
      )}
      {(entscheidbar || (fehler !== null && begruendung)) &&
        (anderer || !fall.korrektur) &&
        !wartet && (
          <TextField
            isReadOnly={!entscheidbar}
            value={begruendung}
            onChange={setBegruendung}
            className="flex flex-col gap-1"
          >
            <Label className="text-caption text-text-2">
              {anderer && entscheidbar ? k.begruendungAnderer : k.begruendung}
            </Label>
            <TextArea
              rows={2}
              maxLength={512}
              className="focus-ring rounded-control border border-separator bg-surface px-3 py-2 text-body text-text outline-none"
            />
          </TextField>
        )}
      {offen && (
        <details className="min-w-0 break-words text-callout text-text-2">
          <summary className="focus-ring cursor-default">{k.aktionsdetails}</summary>
          <p className="m-0 mt-1" lang="en">
            {fall.hinweis}
          </p>
          <code>{fall.aktion}</code>
          {fall.korrektur?.erlaubt.map((b, index) => (
            <div key={b}>
              {befehlName(b, index)} — <code>{b}</code>
            </div>
          ))}
        </details>
      )}
      <p role="status" className={ausgang ? "m-0" : "sr-only"}>
        {ausgang}
      </p>
      {fehler !== null && (
        <Refusal
          title={unklar ? k.ergebnis : k.abgelehnt}
          {...(unklar ? { message: k.unbekannt } : {})}
          raw={fehler instanceof Error ? fehler.message : nichtErreichbar("processd")}
          focusDetails
        />
      )}
    </section>
  );
}

/**
 * A Klärfall: the cause in the words of the list that defines its code,
 * where it came from, when it is due, what happened to it — and the one
 * decision processd allows on it. The service owns the cause, role, Frist and allowed commands.
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
  const [geschlossenId, setGeschlossenId] = useState<string | null>(null);
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
            <nav
              aria-label={t.prozessAnsicht.pfad}
              className="text-caption text-text-2 [overflow-wrap:anywhere]"
            >
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
            <div className="flex flex-col items-start gap-4 sm:flex-row sm:gap-6">
              <div className="min-w-0 flex-1">
                <h1 className="m-0 mt-1 text-large-title font-semibold [overflow-wrap:anywhere]">
                  {f.bedeutung ? (
                    <span lang="de">{f.bedeutung}</span>
                  ) : (
                    k.ursacheCode(f.ursache_code)
                  )}
                  <span
                    className={`ml-3 inline-flex h-6 items-center rounded-full px-2.5 align-middle text-body font-semibold ${f.zustand === "open" && geschlossenId !== f.id ? "bg-info-fill text-info" : "bg-done-fill text-done"}`}
                  >
                    {geschlossenId === f.id && f.zustand === "open"
                      ? t.prozessAnsicht.abgeschlossen
                      : eintrag(k.zustand, f.zustand)}
                  </span>
                </h1>
                {!f.bedeutung && <p className="m-0 mt-1 text-body text-text-2">{k.ohneWortlaut}</p>}
                <ul
                  aria-label={t.prozessAnsicht.eckdaten}
                  className="m-0 mt-1.5 flex list-none flex-wrap gap-x-3 p-0 text-body"
                >
                  <li>
                    {f.bedeutung && <>{f.ursache_code} · </>}
                    <SourceChip quelle={{ dokument: f.ursache_liste }} />
                  </li>
                  <li>
                    {k.rolle} · {f.owner_role ? rollenName(f.owner_role) : k.rolleOhne}
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
              {f.due_at ? (
                // biome-ignore lint/a11y/useSemanticElements: a labelled figure and its caption
                <div
                  role="group"
                  aria-label={t.prozessAnsicht.naechsteFrist}
                  className="flex min-w-0 flex-col items-start gap-1 self-stretch sm:items-end sm:self-start"
                >
                  <FristUhr due={f.due_at} gross />
                  {f.due_source && <SourceChip quelle={{ dokument: f.due_source }} />}
                </div>
              ) : (
                <p className="m-0 text-body text-text-2">{k.fristUnbekannt}</p>
              )}
            </div>
          </Panel>
          <Entscheidung
            key={f.id}
            k={f}
            geschlossen={geschlossenId === f.id}
            onGeschlossen={() => setGeschlossenId(f.id)}
          />
          <Panel className="overflow-hidden">
            <section aria-labelledby={verlaufId}>
              <h2 id={verlaufId} className="m-0 px-6 pt-5 pb-3 text-headline">
                {t.verlauf.titel}
              </h2>
              <ol aria-label={t.verlauf.titel} className="m-0 list-none p-0">
                {q.data.verlauf.map((v) => (
                  <li
                    key={`${v.at}${v.schritt}`}
                    className="grid gap-1 sm:grid-cols-[10rem_minmax(0,1fr)] sm:gap-3 border-t border-separator px-6 py-3 text-body"
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
                        <details className="text-callout break-words text-text-2">
                          <summary className="focus-ring">{k.befehlsdetails}</summary>
                          <code>{v.befehl ?? v.vorgeschlagen}</code>
                        </details>
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
