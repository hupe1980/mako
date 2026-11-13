import { useMutation, useQuery } from "@tanstack/react-query";
import { useId, useState } from "react";
import { Label, Link, Radio, RadioGroup, TextArea, TextField } from "react-aria-components";
import { dienstVon, nichtErreichbar } from "../app/dienste";
import { handbuchLink } from "../app/handbuch";
import { useLaufzeit } from "../app/laufzeit";
import { lesen, PROBEN, type ProbeCommand, zeigen } from "../app/proben";
import { useRolle } from "../app/rollen";
import type { DienstBefund, DienstZeile, Punkt } from "../data/gen/makod";
import { call, ServiceUnavailable, type ToolOutput, ToolRefusal } from "../data/mcp";
import { type Probe, ProbeAbgelehnt, proben } from "../data/probe";
import { Verweigert } from "../data/verweigert";
import { Button } from "../design/Button";
import { Icon, type IconName } from "../design/Icon";
import { Kbd } from "../design/Kbd";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SourceChip } from "../design/SourceChip";
import { eintrag, katalog, useT } from "../i18n";
import { Befragung, FRAGE_ZUM_BEREICH, zurFrage } from "./Befragung";
import { QuickLook } from "./QuickLook";

/**
 * Einrichtung: what a deployment needs before go-live — the readiness
 * checklist across its services, then the Probelauf of a first message.
 */
export function EinrichtungView() {
  const t = useT();
  return (
    <div className="mx-auto flex max-w-5xl flex-col gap-6 px-4 py-6 md:px-8 md:py-10">
      <header>
        <h1 className="m-0 text-large font-bold tracking-tight">{t.einrichtung.titel}</h1>
        <p className="m-0 mt-1 text-callout text-text-2">{t.einrichtung.untertitel}</p>
      </header>
      <Bereitschaft />
      <Befragung />
      <Probelauf />
    </div>
  );
}

/** The deployment's checklist, as makod folds it. */
export type Checkliste = ToolOutput<"makod", "einrichtung_checkliste">;

/** While the view is on screen, how often the checklist is asked again (paused in a hidden tab). */
const TAKT = 15_000;

/**
 * The readiness checklist: one item per Bereich with its Ampel and the
 * findings that colour it, each daemon's state beside. makod asks the
 * deployment's services and decides every colour; the Desk shows them.
 */
export function Bereitschaft() {
  const t = useT();
  const c = t.einrichtung.checkliste;
  const { rolle } = useRolle();
  const q = useQuery({
    queryKey: [rolle, "makod", "einrichtung_checkliste"],
    queryFn: () => call("makod", "einrichtung_checkliste", {}),
    refetchInterval: TAKT,
  });
  const titel = useId();
  return (
    <Panel className="flex flex-col gap-5 p-4 md:p-6">
      <section aria-labelledby={titel} className="contents">
        <div className="flex flex-wrap items-start gap-3">
          <div className="min-w-0 flex-1 basis-64">
            <h2 id={titel} className="m-0 text-title font-semibold">
              {c.titel}
            </h2>
            <p className="m-0 mt-1 max-w-3xl text-body text-text-2">{c.text}</p>
          </div>
          <div className="flex items-center gap-2">
            <span className="text-caption text-text-2">{q.isFetching ? c.prueft : c.laufend}</span>
            <Button variant="ghost" onPress={() => q.refetch()} isDisabled={q.isFetching}>
              <Icon name="erneuern" size={14} />
              {c.pruefen}
            </Button>
          </div>
        </div>
        <div aria-busy={q.isPending} className="flex flex-col gap-5">
          {q.isError && <ChecklisteFehler error={q.error} />}
          {q.data && <ChecklisteAnzeige liste={q.data} />}
        </div>
      </section>
    </Panel>
  );
}

function ChecklisteFehler({ error }: { error: Error }) {
  const t = useT();
  const c = t.einrichtung.checkliste;
  if (error instanceof ToolRefusal)
    return (
      <Notice tone="refusal" title={c.abgewiesen}>
        {error.message}
      </Notice>
    );
  return (
    <Notice
      tone="failure"
      title={error instanceof ServiceUnavailable ? nichtErreichbar(error.service) : c.fehler}
    >
      {error.message}
    </Notice>
  );
}

/** The checklist as makod answered it; an answer of another shape is shown verbatim. */
export function ChecklisteAnzeige({ liste }: { liste: Checkliste }) {
  const t = useT();
  const c = t.einrichtung.checkliste;
  if (!Array.isArray(liste?.punkte) || !Array.isArray(liste?.dienste))
    return <Roh titel={c.unbekannteForm} text={JSON.stringify(liste, null, 2)} />;
  const unlesbar = (liste.berichte ?? []).filter((b) => b.zustand === "unlesbar");
  return (
    <>
      <div
        role="status"
        className={`squircle flex gap-3 rounded-panel border p-4 ${
          liste.bereit ? "border-done/30 bg-done-fill" : "border-separator bg-fill"
        }`}
      >
        <Icon
          name={liste.bereit ? "check" : "blockiert"}
          size={20}
          className={`shrink-0 ${liste.bereit ? "text-done" : "text-text-2"}`}
        />
        <div className="flex min-w-0 flex-col gap-0.5">
          <p className="m-0 text-callout font-semibold text-text">
            {liste.bereit ? c.bereit : c.nichtBereit}
          </p>
          <p className="m-0 text-body text-text-2">
            {liste.bereit ? c.bereitText : c.nichtBereitText}
          </p>
        </div>
      </div>

      <div className="flex flex-col gap-2">
        <h3 className="m-0 text-caption font-semibold uppercase text-text-2">{c.dienste}</h3>
        <ul aria-label={c.dienste} className="m-0 flex list-none flex-wrap gap-2 p-0">
          {liste.dienste.map((d) => (
            <DienstChip key={d.dienst} zeile={d} />
          ))}
        </ul>
      </div>

      <div className="flex flex-col gap-2">
        <h3 className="m-0 text-caption font-semibold uppercase text-text-2">{c.punkte}</h3>
        <ul aria-label={c.punkte} className="m-0 flex list-none flex-col gap-3 p-0">
          {liste.punkte.map((p) => (
            <PunktZeile key={p.bereich} punkt={p} />
          ))}
        </ul>
      </div>

      {unlesbar.length > 0 && (
        <div className="flex flex-col gap-2">
          <h3 className="m-0 text-caption font-semibold uppercase text-text-2">{c.roh}</h3>
          <p className="m-0 text-body text-text-2">{c.rohText}</p>
          {unlesbar.map((b) => (
            <Roh
              key={b.dienst}
              titel={c.rohDienst(dienstName(b.dienst), b.grund)}
              text={b.zustand === "unlesbar" ? b.roh : ""}
            />
          ))}
        </div>
      )}
    </>
  );
}

/** A service as the operator knows it; a name the Desk does not know, as configured. */
function dienstName(dienst: string): string {
  if (dienst === "deployment") return katalog().einrichtung.checkliste.deployment;
  const name = dienstVon(dienst);
  return name === dienstVon("") ? dienst : name;
}

const AMPEL: Record<string, { icon: IconName; farbe: string }> = {
  gruen: { icon: "check", farbe: "text-done" },
  gelb: { icon: "warn", farbe: "text-soon" },
  rot: { icon: "blockiert", farbe: "text-critical" },
};
const UNBEKANNT = { icon: "unbekannt" as IconName, farbe: "text-text-2" };

/** An Ampel as makod set it: an icon and a word, never colour alone. */
export function AmpelZeichen({ ampel }: { ampel: string }) {
  const t = useT();
  const a = AMPEL[ampel] ?? UNBEKANNT;
  return (
    <span
      className={`inline-flex shrink-0 items-center gap-1 text-caption font-semibold ${a.farbe}`}
    >
      <Icon name={a.icon} size={16} />
      {eintrag(t.einrichtung.checkliste.ampel, ampel)}
    </span>
  );
}

const STATUS: Record<string, { icon: IconName; farbe: string }> = {
  bereit: { icon: "check", farbe: "text-done" },
  blockiert: { icon: "blockiert", farbe: "text-critical" },
  unlesbar: { icon: "warn", farbe: "text-soon" },
};

/** A service's state: unreachable is unknown — neither ready nor failing. */
export function DienstChip({ zeile }: { zeile: DienstZeile }) {
  const t = useT();
  const c = t.einrichtung.checkliste;
  const s = STATUS[zeile.status] ?? UNBEKANNT;
  return (
    <li className="squircle flex min-w-0 items-center gap-2 rounded-control border border-separator px-2.5 py-1.5">
      <Icon name={s.icon} size={14} className={`shrink-0 ${s.farbe}`} />
      <span className="min-w-0 text-body font-medium text-text">{dienstName(zeile.dienst)}</span>
      <span className="text-caption text-text-2">
        {eintrag(c.dienstStatus, zeile.status)}
        {zeile.version ? ` · ${c.version(zeile.version)}` : ""}
      </span>
    </li>
  );
}

/** One Bereich: its Ampel, its question, the findings that colour it. */
export function PunktZeile({ punkt }: { punkt: Punkt }) {
  const t = useT();
  const c = t.einrichtung.checkliste;
  const titel = eintrag(c.bereiche, punkt.bereich);
  const frage = c.fragen[punkt.bereich] ?? punkt.frage;
  const befunde = punkt.befunde ?? [];
  return (
    <li className="squircle flex flex-col gap-3 rounded-panel border border-separator p-4">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h4 className="m-0 text-callout font-semibold text-text">{titel}</h4>
        <AmpelZeichen ampel={punkt.ampel} />
        <p className="m-0 basis-full text-body text-text-2">{frage}</p>
      </div>
      {befunde.length > 0 && (
        <ul aria-label={c.befunde(titel)} className="m-0 flex list-none flex-col gap-2 p-0">
          {befunde.map((b, i) => (
            // A finding has no id; makod's order names it.
            // biome-ignore lint/suspicious/noArrayIndexKey: makod's order
            <BefundZeile key={i} befund={b} />
          ))}
        </ul>
      )}
    </li>
  );
}

/** A finding in the reporting service's own words, with its path, manual entry and source. */
export function BefundZeile({ befund }: { befund: DienstBefund }) {
  const t = useT();
  const c = t.einrichtung.checkliste;
  const blockiert = befund.schwere === "blockiert";
  const { handbuch } = useLaufzeit();
  const link = befund.referenz ? handbuchLink(befund.referenz, handbuch) : null;
  const frage = FRAGE_ZUM_BEREICH[befund.bereich];
  const chip =
    "inline-flex min-h-5 max-w-full items-center gap-1 rounded-[10px] border border-separator px-2 py-px text-caption text-text-2";
  return (
    <li className="flex gap-2.5 rounded-control bg-fill px-3 py-2.5">
      <Icon
        name={blockiert ? "blockiert" : "warn"}
        size={16}
        className={`mt-0.5 shrink-0 ${blockiert ? "text-critical" : "text-soon"}`}
      />
      <div className="flex min-w-0 flex-1 flex-col gap-1.5">
        <p className="m-0 text-body break-words text-text">
          <span className="sr-only">{`${eintrag(c.schwere, befund.schwere)}: `}</span>
          {befund.text}
        </p>
        <div className="flex flex-wrap items-center gap-1.5">
          <span className="text-caption font-medium text-text-2">{dienstName(befund.dienst)}</span>
          <code className={`${chip} font-mono break-all`} title={c.pfad}>
            {befund.pfad || c.ganzeDatei}
          </code>
          {befund.referenz &&
            (link ? (
              <Link
                href={link}
                target="_blank"
                rel="noreferrer"
                aria-label={c.referenzKnopf(befund.pfad || befund.referenz)}
                className={`focus-ring ${chip} font-medium no-underline data-[hovered]:bg-fill data-[hovered]:text-text`}
              >
                <Icon name="doc" size={12} className="shrink-0" />
                {c.referenz}
              </Link>
            ) : (
              <span className={chip}>{befund.referenz}</span>
            ))}
          {befund.quelle && <SourceChip quelle={{ dokument: befund.quelle }} />}
          {frage && (
            <Link
              onPress={() => zurFrage(frage)}
              className={`focus-ring ${chip} cursor-default border-accent/40 font-medium text-accent no-underline data-[hovered]:bg-fill`}
            >
              <Icon name="tune" size={12} className="shrink-0" />
              {t.einrichtung.befragung.ergebnis.imVorschlag}
            </Link>
          )}
        </div>
      </div>
    </li>
  );
}

/** An answer of a shape the Desk does not know, as it came. */
function Roh({ titel, text }: { titel: string; text: string }) {
  return (
    <div className="flex flex-col gap-1.5">
      <p className="m-0 text-body font-medium text-text">{titel}</p>
      <pre className="m-0 max-h-60 overflow-auto rounded-control bg-fill p-3 font-mono text-caption whitespace-pre-wrap break-all text-text">
        {text}
      </pre>
    </div>
  );
}

/**
 * The Probelauf: one outbound message built through the door a real one
 * takes, held against its profile and the pre-send check, shown with its
 * verdict and segment by segment. makod writes nothing; the verdict is its.
 */
export function Probelauf() {
  const t = useT();
  const p = t.einrichtung.probe;
  const hilfe = useId();
  const [command, setCommand] = useState<ProbeCommand>("gpke.lieferbeginn.anmelden");
  const art = PROBEN.find((a) => a.command === command) ?? PROBEN[0];
  const [angaben, setAngaben] = useState(() => zeigen(art?.beispiel ?? {}));
  const payload = lesen(angaben);

  const lauf = useMutation<Probe, Error, { command: string; payload: Record<string, unknown> }>({
    mutationFn: (v) => proben(v.command, v.payload),
  });

  const starten = () => {
    if (payload && !lauf.isPending) lauf.mutate({ command, payload });
  };

  const waehlen = (c: string) => {
    const neu = PROBEN.find((a) => a.command === c);
    if (!neu) return;
    setCommand(neu.command);
    setAngaben(zeigen(neu.beispiel));
    lauf.reset();
  };

  return (
    <Panel className="flex flex-col gap-5 p-6">
      <div>
        <h2 className="m-0 text-title font-semibold">{p.titel}</h2>
        <p className="m-0 mt-1 max-w-3xl text-body text-text-2">{p.text}</p>
      </div>

      <form
        className="flex flex-col gap-4"
        onSubmit={(e) => {
          e.preventDefault();
          starten();
        }}
      >
        <RadioGroup
          value={command}
          onChange={waehlen}
          orientation="horizontal"
          className="flex flex-col gap-1.5"
        >
          <Label className="text-caption font-medium text-text-2">{p.nachricht}</Label>
          <div className="flex flex-wrap gap-2">
            {PROBEN.map((a) => (
              <Radio
                key={a.command}
                value={a.command}
                className="focus-ring flex h-9 cursor-default items-center gap-2 rounded-control border border-separator bg-surface px-3 text-body text-text-2 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:text-accent"
              >
                {p.art(p.arten[a.command], a.pid)}
              </Radio>
            ))}
          </div>
        </RadioGroup>

        <TextField
          value={angaben}
          onChange={setAngaben}
          isInvalid={payload === null}
          aria-describedby={hilfe}
          className="flex flex-col gap-1.5"
        >
          <Label className="text-caption font-medium text-text-2">{p.angaben}</Label>
          <TextArea
            rows={12}
            spellCheck={false}
            onKeyDown={(e) => {
              if (e.key === "Enter" && (e.metaKey || e.ctrlKey)) {
                e.preventDefault();
                starten();
              }
            }}
            className="focus-ring squircle min-h-40 resize-y rounded-control border border-separator bg-surface p-3 font-mono text-body text-text outline-none data-[invalid]:border-critical"
          />
          <p id={hilfe} className="m-0 text-caption text-text-2">
            {payload === null ? (
              <span className="text-critical">{p.keinJson}</span>
            ) : (
              p.angabenHilfe
            )}
          </p>
        </TextField>

        <div className="flex flex-wrap items-center gap-2">
          <Button type="submit" variant="primary" isDisabled={payload === null || lauf.isPending}>
            <Icon name="check" size={14} />
            {lauf.isPending ? p.laeuft : p.starten}
            <Kbd>⌘↵</Kbd>
          </Button>
          <Button
            variant="ghost"
            onPress={() => {
              if (art) setAngaben(zeigen(art.beispiel));
              lauf.reset();
            }}
          >
            {p.beispielwerte}
          </Button>
        </div>
      </form>

      <div aria-live="polite" aria-busy={lauf.isPending} className="flex flex-col gap-5">
        {lauf.isError && <Fehler error={lauf.error} />}
        {lauf.data && <Ergebnis probe={lauf.data} />}
        {!lauf.data && !lauf.isError && !lauf.isPending && (
          <EmptyState title={p.leer}>{p.leerText}</EmptyState>
        )}
      </div>
    </Panel>
  );
}

function Fehler({ error }: { error: Error }) {
  const t = useT();
  const p = t.einrichtung.probe;
  if (error instanceof ProbeAbgelehnt || error instanceof Verweigert)
    return (
      <Notice tone="refusal" title={p.abgewiesen}>
        {error.message}
      </Notice>
    );
  return (
    <Notice tone="failure" title={error instanceof TypeError ? nichtErreichbar("makod") : p.fehler}>
      {error.message}
    </Notice>
  );
}

/** The verdict as makod gave it, then each message it built. */
function Ergebnis({ probe }: { probe: Probe }) {
  const t = useT();
  const p = t.einrichtung.probe;
  const besteht = probe.urteil === "besteht";
  return (
    <>
      <div
        role="status"
        data-urteil={probe.urteil}
        className={`squircle flex gap-3 rounded-panel border p-4 ${
          besteht ? "border-done/30 bg-done-fill" : "border-critical/30 bg-critical-fill"
        }`}
      >
        <Icon
          name={besteht ? "check" : "warn"}
          size={20}
          className={`shrink-0 ${besteht ? "text-done" : "text-critical"}`}
        />
        <div className="flex min-w-0 flex-col gap-1">
          <p className="m-0 text-callout font-semibold text-text">
            {besteht ? p.besteht : p.abgelehnt}
            <span className="font-normal text-text-2">
              {" — "}
              {besteht ? p.bestehtText : probe.stufe ? p.stufe[probe.stufe] : ""}
            </span>
          </p>
          {probe.grund && <p className="m-0 text-body break-words text-text">{probe.grund}</p>}
          <p className="m-0 text-caption text-text-2">{p.nichtsGesendet}</p>
        </div>
      </div>
      {probe.nachrichten.map((n, i) => (
        // A rehearsal's messages carry no id; the door's order names them.
        // biome-ignore lint/suspicious/noArrayIndexKey: the door's order
        <QuickLook key={i} nachricht={n} />
      ))}
    </>
  );
}
