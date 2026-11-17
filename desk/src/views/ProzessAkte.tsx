import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { useId, useState } from "react";
import {
  Button as AriaButton,
  Label,
  Slider,
  SliderOutput,
  SliderThumb,
  SliderTrack,
} from "react-aria-components";
import { family } from "../app/codes";
import { DIENST } from "../app/dienste";
import { disclosure } from "../app/disclosure";
import { useImDrift } from "../app/imDrift";
import { prozessTitel, zustand } from "../app/prozesse";
import { nachweise, offeneFristen, type Schritt, schritte } from "../app/prozessStand";
import { ROLLEN, useRolle } from "../app/rollen";
import { AKTION, type Answer } from "../data/agent";
import type { HistoryEntry, ProcessView } from "../data/gen/makod";
import { call } from "../data/mcp";
import { Button } from "../design/Button";
import { FristUhr } from "../design/FristUhr";
import { Icon } from "../design/Icon";
import { Angabe, AngabenListe } from "../design/ListDetail";
import { Panel } from "../design/Panel";
import { SourceChip } from "../design/SourceChip";
import { formatInstant } from "../format/date";
import { useT } from "../i18n";
import { useVorschlaege } from "../inbox/quellen";
import { useDarstellbar } from "./entscheidungsprobe";
import { Entscheiden } from "./Freigaben";
import { MarktpartnerAntwort } from "./MarktpartnerAntwort";
import { useHistorie, Verlauf } from "./Verlauf";

/** processd's state, in its status colour: done, refused, or still in progress. */
const TON: Record<string, string> = {
  completed: "bg-done-fill text-done",
  rejected: "bg-critical-fill text-critical",
  failed: "bg-critical-fill text-critical",
  antwortfrist_abgelaufen: "bg-critical-fill text-critical",
};

function Zustand({ state }: { state: string }) {
  return (
    <span
      className={`ml-3 inline-flex h-6 items-center rounded-full px-2.5 align-middle text-body font-semibold ${TON[state] ?? "bg-info-fill text-info"}`}
    >
      {zustand(state)}
    </span>
  );
}

function Schrittleiste({ liste }: { liste: Schritt[] }) {
  const t = useT();
  const k = t.prozessAnsicht;
  return (
    <ol aria-label={k.schritte} className="m-0 flex list-none flex-wrap items-center gap-y-2 p-0">
      {liste.map((s, i) => (
        <li
          key={s.id}
          data-stand={s.stand}
          {...(s.stand === "jetzt" ? { "aria-current": "step" as const } : {})}
          className="flex items-center"
        >
          {i > 0 && <span aria-hidden="true" className="mx-3 h-px w-6 bg-separator" />}
          <span
            aria-hidden="true"
            className={`mr-2 inline-flex size-4 shrink-0 items-center justify-center rounded-full ${
              s.stand === "erledigt"
                ? "bg-done-fill text-done"
                : s.stand === "jetzt"
                  ? "bg-accent ring-4 ring-[color-mix(in_srgb,var(--accent)_18%,transparent)]"
                  : "border border-separator"
            }`}
          >
            {s.stand === "erledigt" && <Icon name="check" size={11} />}
          </span>
          <span className="sr-only">{k.schrittStand[s.stand]}: </span>
          <span
            data-text
            className={`text-body ${s.stand === "jetzt" ? "font-semibold text-text" : s.stand === "erledigt" ? "text-text-2" : "text-text-2"}`}
          >
            {s.text}
          </span>
        </li>
      ))}
    </ol>
  );
}

/** The process as of an earlier event: one thumb over the history's events. */
function StandRegler({
  eintraege,
  bis,
  onBis,
}: {
  eintraege: HistoryEntry[];
  bis: number | null;
  onBis: (bis: number | null) => void;
}) {
  const t = useT();
  const k = t.prozessAnsicht;
  const erstes = eintraege[0]?.sequence;
  const letztes = eintraege.at(-1)?.sequence;
  if (erstes === undefined || letztes === undefined || erstes === letztes) return null;
  const wert = bis ?? letztes;
  const at = eintraege.find((e) => e.sequence === wert)?.recorded_at;
  return (
    <Slider
      minValue={erstes}
      maxValue={letztes}
      step={1}
      value={wert}
      onChange={(v) => onBis(v === letztes ? null : v)}
      className="mb-4 flex flex-col gap-1.5"
    >
      <div className="flex items-baseline gap-3">
        <Label className="sr-only">{k.stand}</Label>
        <SliderOutput className="text-caption font-semibold text-text">
          {wert === letztes ? k.standJetzt : at ? k.standZu(formatInstant(at)) : k.stand}
        </SliderOutput>
      </div>
      <SliderTrack className="relative h-5 w-full">
        <span
          aria-hidden="true"
          className="absolute top-1/2 h-1 w-full -translate-y-1/2 rounded-full bg-fill-strong"
        />
        <SliderThumb className="focus-ring top-1/2 size-4 rounded-full border border-separator bg-surface-raised shadow-popover data-[dragging]:bg-accent" />
      </SliderTrack>
    </Slider>
  );
}

function Nachweise({
  eintraege,
  process,
  bis,
  ercCode,
}: {
  eintraege: HistoryEntry[];
  process: ProcessView;
  bis: number | null;
  ercCode: string | null;
}) {
  const t = useT();
  const k = t.prozessAnsicht;
  const id = useId();
  const rows = nachweise(
    eintraege,
    bis === null ? process.deadlines : [],
    bis === null ? ercCode : null,
  );
  return (
    <section aria-labelledby={id} className="border-t border-separator px-6 py-5">
      <h2 id={id} className="m-0 mb-2 text-headline">
        {k.nachweise}
      </h2>
      <ol aria-label={k.nachweise} className="m-0 list-none p-0">
        {rows.map((n) => (
          <li
            key={n.id}
            data-belegt={n.belegt}
            className="flex items-center gap-3 border-b border-separator py-2.5 last:border-b-0"
          >
            <span
              aria-hidden="true"
              className={`inline-flex size-5 shrink-0 items-center justify-center rounded-full ${
                n.belegt
                  ? "bg-done-fill text-done"
                  : n.abgelehnt
                    ? "bg-critical-fill text-critical"
                    : "border border-dashed border-text-2"
              }`}
            >
              {n.belegt && <Icon name="check" size={12} />}
              {n.abgelehnt && <Icon name="warn" size={12} />}
            </span>
            <span className="min-w-0 flex-1">
              <span className={`block text-body ${n.belegt ? "text-text" : "text-text"}`}>
                {n.text}
                {!n.belegt && !n.abgelehnt && (
                  <>
                    {" "}
                    <span className="ml-1 text-text-2">{k.stehtAus}</span>
                  </>
                )}
                {/* The history records the message; whether it was delivered is the
                    Zustellquittung's, which the history does not carry (SR-045). */}
                {n.belegt && <span className="ml-2 text-caption text-text-2">{k.belegt}</span>}
              </span>
              {n.ref && <span className="block text-caption text-text-2">{n.ref}</span>}
            </span>
            {n.zeit && (
              <span className="shrink-0 text-caption text-text-2">{formatInstant(n.zeit)}</span>
            )}
            {n.frist && <FristUhr due={n.frist} />}
          </li>
        ))}
      </ol>
    </section>
  );
}

/** What processd holds for a person on this process, if anything — asked by the process itself. */
function useGehalteneFreigabe(processId: string) {
  const { rolle } = useRolle();
  const marktrolle = ROLLEN[rolle].marktrolle;
  const q = useQuery({
    queryKey: [rolle, "processd", "list_pending_approvals", marktrolle, processId],
    queryFn: () =>
      call("processd", "list_pending_approvals", { marktrolle, process_id: processId }),
  });
  return q.data?.freigaben?.find((x) => x.process_id === processId) ?? null;
}

/**
 * The assistant's proposal under the decision bar, and what the gate made of
 * it — agentd's Prüfvermerk, said as the service said it: passed, refused
 * with the rule and the service's sentence, or not checked and why.
 */
function Pruefzeile({ a }: { a: Answer }) {
  const t = useT();
  const k = t.prozessAnsicht;
  const v = a.vorschlag;
  const pv = a.pruefvermerk ?? null;
  if (!v) return null;
  const abgelehnt = pv?.stufen.find((s) => s.urteil === "abgelehnt");
  return (
    <p className="m-0 flex w-full gap-2 border-l-2 border-ai pl-3 text-body text-text">
      <Icon name="sparkle" size={14} className="mt-0.5 shrink-0 text-ai" />
      <span className="min-w-0">
        <span className="font-semibold">
          {disclosure(a.disclosure)} {t.heute.vorschlag(AKTION[v.aktion])}
          {v.antwortcode && <span className="ml-1">{v.antwortcode}</span>}
          {" — "}
        </span>
        {v.begruendung}{" "}
        <span
          className={`text-caption font-semibold ${pv?.urteil === "bestanden" ? "text-done" : pv?.urteil === "abgelehnt" ? "text-critical" : "text-text-2"}`}
        >
          {pv?.urteil === "bestanden"
            ? k.geprueft
            : pv?.urteil === "abgelehnt"
              ? k.abgelehntWeil(
                  [abgelehnt?.regel, abgelehnt?.text].filter(Boolean).join(": ") || k.ohneGrund,
                )
              : k.nichtGeprueft(pv?.grund ?? null)}
        </span>
      </span>
    </p>
  );
}

function Entscheidungsleiste({ processId }: { processId: string }) {
  const t = useT();
  const k = t.prozessAnsicht;
  const id = useId();
  const client = useQueryClient();
  const f = useGehalteneFreigabe(processId);
  const [offen, setOffen] = useState<"leer" | "uebernommen" | null>(null);
  // The assistant's proposal, read as agentd keeps it — never asked for here.
  const wartend = useVorschlaege({ freigabe: f ? [f.id] : [] }).freigabe.get(f?.id ?? "");
  const vorschlag = wartend?.vorschlag ?? null;
  const darstellbar = useDarstellbar(f, vorschlag);
  // Unchanged only where the gate passed it and its message can be rendered (M84 SR-035).
  const urteil =
    wartend?.pruefvermerk?.urteil === "bestanden" && darstellbar !== "ja"
      ? "nicht_geprueft"
      : (wartend?.pruefvermerk?.urteil ?? "nicht_geprueft");
  // A processd in drift may answer a decision differently than the Desk
  // reads it: no decision is offered until the Desk is current.
  const drift = useImDrift("processd");
  if (!f) return null;
  const ebd = f.antwort?.ebd ?? f.codes_freigeben?.ebd ?? f.codes_ablehnen?.ebd ?? null;
  return (
    <>
      <section
        aria-labelledby={id}
        className="squircle flex flex-wrap items-center gap-4 rounded-panel border border-[color-mix(in_srgb,var(--accent)_30%,transparent)] bg-[color-mix(in_srgb,var(--accent)_7%,var(--surface))] px-6 py-4"
      >
        <div className="min-w-0 flex-1">
          <h2 id={id} className="m-0 text-headline">
            {k.entscheidung}
          </h2>
          <p className="m-0 mt-1 flex flex-wrap items-center gap-2 text-body text-text-2">
            <span>{f.grund}</span>
            {ebd && <SourceChip quelle={{ dokument: ebd }} />}
          </p>
        </div>
        <FristUhr due={f.expires_at} />
        {drift ? (
          <p className="m-0 text-body text-text-2">{t.drift.keineEntscheidung(DIENST.processd)}</p>
        ) : (
          // One action. Without a proposal it is the decision itself; with
          // one, only a proposal the gate passed can be taken in — any other
          // opens the decision empty, the proposal beside it to edit from.
          <Button
            variant="primary"
            aria-expanded={offen !== null}
            onPress={() =>
              setOffen(offen ? null : vorschlag && urteil === "bestanden" ? "uebernommen" : "leer")
            }
          >
            {!vorschlag ? k.entscheiden : urteil === "bestanden" ? k.uebernehmen : k.bearbeiten}
          </Button>
        )}
        {vorschlag && wartend && <Pruefzeile a={wartend} />}
      </section>
      {offen && !drift && (
        <Entscheiden
          key={offen}
          f={f}
          sofort={offen === "uebernommen"}
          onDone={() => {
            setOffen(null);
            void client.invalidateQueries();
          }}
        />
      )}
    </>
  );
}

function Copy({ value, label }: { value: string; label: string }) {
  const t = useT();
  const [done, setDone] = useState(false);
  return (
    <AriaButton
      aria-label={t.prozessAnsicht.kopieren(label)}
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

function Wert({ value, label, copy }: { value: string; label: string; copy?: boolean }) {
  return (
    <span className="flex min-w-0 items-center gap-1">
      <span className="min-w-0 truncate" title={value}>
        {value}
      </span>
      {copy && <Copy value={value} label={label} />}
    </span>
  );
}

/**
 * One process as a record, top to bottom: crumbs, the title with processd's
 * state, one line of facts, the next Frist large on the right and the steps
 * the history states; the decision bar while processd holds a Freigabe for
 * it; the Verlauf with its Nachweise beside the details. The Stand control
 * replays the history to any of its events — the same reads, fewer rows.
 */
export function ProzessAkte({
  businessKey,
  process,
}: {
  businessKey: string;
  process: ProcessView;
}) {
  const t = useT();
  const k = t.prozessAnsicht;
  const { rolle } = useRolle();
  const [bis, setBis] = useState<number | null>(null);
  const history = useHistorie(process.process_id);
  const projektion = useQuery({
    queryKey: [rolle, "processd", "get_process", process.process_id],
    queryFn: () => call("processd", "get_process", { process_id: process.process_id }),
  });
  const p = projektion.data;
  const partner = p?.partner_mp_id ?? null;
  const namen = useQuery({
    queryKey: [rolle, "marktd", "partner_namen", partner ? [partner] : []],
    queryFn: () => call("marktd", "partner_namen", { mp_ids: partner ? [partner] : [] }),
    enabled: Boolean(partner),
    staleTime: 300_000,
  });
  const partnerName = namen.data?.partner?.find((n) => n.mp_id === partner)?.name ?? partner;
  const alle = history.data?.eintraege ?? [];
  const eintraege = alle.filter((e) => bis === null || e.sequence <= bis);
  const naechste = offeneFristen(process.deadlines)[0];
  // The Gegenpartei's record is processd's, by MP-ID and the role it held;
  // linked only where processd lists exactly one such role for the MP-ID.
  const gegenparteien = useQuery({
    queryKey: [rolle, "processd", "list_counterparties"],
    queryFn: () => call("processd", "list_counterparties", {}),
    enabled: Boolean(partner),
    staleTime: 300_000,
  });
  const akten = (gegenparteien.data?.gegenparteien ?? []).filter((g) => g.mp_id === partner);
  const akte = akten.length === 1 ? akten[0] : undefined;
  const fakten = [
    p?.malo_id ? `MaLo ${p.malo_id}` : null,
    p ? `PID ${p.pid}` : null,
    partnerName,
    p ? family(p.family) : null,
  ].filter((f): f is string => Boolean(f));

  return (
    <div className="flex flex-col gap-4">
      <Panel className="px-6 pt-5 pb-5">
        <div className="flex flex-wrap items-start gap-6">
          <div className="min-w-0 flex-1">
            <nav aria-label={k.pfad} className="text-caption text-text-2">
              <Link
                to="/prozess"
                search={{}}
                className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
              >
                {k.titel}
              </Link>
              <span aria-hidden="true" className="mx-1.5">
                /
              </span>
              <span>{businessKey}</span>
            </nav>
            <h1 className="m-0 mt-1 text-large-title font-semibold">
              {prozessTitel(process.workflow_name)}
              {p && <Zustand state={p.state} />}
            </h1>
            {fakten.length > 0 && (
              <ul
                aria-label={k.eckdaten}
                className="m-0 mt-1.5 flex list-none flex-wrap gap-x-2 p-0 text-body text-text"
              >
                {fakten.map((f, i) => (
                  <li key={f}>
                    {i > 0 && (
                      <span aria-hidden="true" className="mr-2 text-text-2">
                        ·
                      </span>
                    )}
                    {p?.malo_id && f === `MaLo ${p.malo_id}` ? (
                      <Link
                        to="/lokation/$id"
                        params={{ id: p.malo_id }}
                        className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
                      >
                        {f}
                      </Link>
                    ) : akte && f === partnerName ? (
                      <Link
                        to="/gegenpartei/$mpId/$rolle"
                        params={{ mpId: akte.mp_id, rolle: akte.rolle }}
                        className="focus-ring rounded-[4px] text-accent underline underline-offset-2"
                      >
                        {f}
                      </Link>
                    ) : (
                      <span>{f}</span>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </div>
          {naechste && (
            // biome-ignore lint/a11y/useSemanticElements: a labelled group of a figure and its caption, not a form fieldset
            <div
              role="group"
              aria-label={k.naechsteFrist}
              className="flex flex-col items-end gap-1"
            >
              <FristUhr due={naechste.due_at} gross />
              <span className="text-caption text-text-2" title={naechste.label}>
                {t.prozess.fristArt[naechste.art as keyof typeof t.prozess.fristArt] ??
                  naechste.art}
              </span>
            </div>
          )}
        </div>
        <div className="mt-5 border-t border-separator pt-4">
          <Schrittleiste
            liste={schritte(
              eintraege,
              bis === null ? process.deadlines : [],
              bis === null ? (p?.state ?? null) : null,
            )}
          />
        </div>
      </Panel>

      <Entscheidungsleiste processId={process.process_id} />

      <div className="grid grid-cols-1 gap-4 lg:grid-cols-[minmax(0,1fr)_minmax(0,22rem)]">
        <Panel className="overflow-hidden">
          <Verlauf
            processId={process.process_id}
            deadlines={process.deadlines}
            bis={bis}
            vorspann={<StandRegler eintraege={alle} bis={bis} onBis={setBis} />}
          />
          {history.data && (
            <Nachweise
              eintraege={eintraege}
              process={process}
              bis={bis}
              ercCode={p?.erc_code ?? null}
            />
          )}
        </Panel>
        <Panel className="self-start overflow-hidden">
          <div className="px-6 pt-5 pb-4">
            <h2 className="m-0 mb-3 text-headline">{k.details}</h2>
            <AngabenListe>
              <Angabe name={k.workflow}>{process.workflow_name}</Angabe>
              <Angabe name={k.prozessId}>
                <Wert value={process.process_id} label={k.prozessId} copy />
              </Angabe>
              <Angabe name={k.stream}>
                <Wert value={process.stream_id} label={k.stream} copy />
              </Angabe>
              {p && <Angabe name={k.familie}>{family(p.family)}</Angabe>}
              {partner && (
                <Angabe name={k.marktpartner}>
                  <Wert value={partner} label={k.marktpartner} copy />
                </Angabe>
              )}
              {p?.started_at && <Angabe name={k.begonnen}>{formatInstant(p.started_at)}</Angabe>}
              {p?.last_event_at && (
                <Angabe name={k.letztes}>{formatInstant(p.last_event_at)}</Angabe>
              )}
              {p?.deadline_at && (
                <Angabe name={k.antwortfrist}>
                  <span className="inline-flex flex-wrap items-center gap-2">
                    {formatInstant(p.deadline_at)}
                    {p.deadline_source && <SourceChip quelle={{ dokument: p.deadline_source }} />}
                  </span>
                </Angabe>
              )}
            </AngabenListe>
          </div>
          <MarktpartnerAntwort processId={process.process_id} />
        </Panel>
      </div>
    </div>
  );
}
