import { useEffect, useState } from "react";
import {
  Button as AriaButton,
  Input,
  Label,
  Radio,
  RadioGroup,
  TextArea,
  TextField,
} from "react-aria-components";
import {
  type Entscheidbar,
  perTaste,
  aktion as registriert,
  schluessel,
  useFehlschlag,
  useHalten,
} from "../app/aktionen";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { UEBERNEHMEN } from "../app/keys";
import type { Answer, Bilanz, Vorschlag } from "../data/agent";
import { buchungsart, ErneutGeparkt } from "../data/buchung";
import type { GeparkteBuchung } from "../data/gen/accountingd";
import { Abgelehnt, SchonErledigt } from "../data/rechnung";
import { useVerweigert, Verweigert } from "../data/verweigert";
import { Icon } from "../design/Icon";
import { Notice, Panel } from "../design/Panel";
import { formatBusinessDate, formatInstant } from "../format/date";
import { seit, useNow } from "../format/frist";
import { formatCent } from "../format/money";
import { useT } from "../i18n";
import { VorschlagKarte } from "./Fragen";
import { Gehalten } from "./Rechnungen";

const WAHL =
  "focus-ring flex h-9 cursor-default items-center rounded-control border border-separator px-4 text-body font-medium text-text-2 data-[disabled]:opacity-40 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:bg-[color-mix(in_srgb,var(--accent)_10%,transparent)] data-[selected]:text-accent";
const PRIMAER =
  "focus-ring h-9 rounded-control bg-accent px-4 text-body font-semibold text-on-accent data-[disabled]:opacity-40";
const FELD =
  "focus-ring rounded-control border border-separator bg-surface px-3 py-2 text-body text-text outline-none";

type Aktion = "buchen" | "verwerfen";

/** Where the decision starts — the gallery shows each state. */
export interface Anfang {
  aktion?: Aktion;
  grund?: string;
  /** Held for undo: the five seconds before it is sent. */
  gehalten?: boolean;
  fehler?: unknown;
}

/** The mark on a field the assistant filled in: its tint and its sparkle, and the words. */
function VomAssistenten({ children }: { children: string }) {
  return (
    <span className="flex items-center gap-1.5 text-caption text-ai">
      <Icon name="sparkle" size={11} className="shrink-0" />
      {children}
    </span>
  );
}

/** accountingd's answer to a resolution it did not take, in its words. */
function Fehler({ error }: { error: unknown }) {
  const t = useT();
  if (!error || error instanceof Verweigert) return null;
  if (error instanceof ErneutGeparkt)
    return (
      <Notice tone="info" title={t.buchungen.erneutGeparkt}>
        {t.buchungen.erneutGeparktText(error.message)}
      </Notice>
    );
  if (error instanceof SchonErledigt)
    return (
      <Notice tone="info" title={t.entscheidung.schonErledigt}>
        {t.buchungen.nichtsGebucht(error.message)}
      </Notice>
    );
  return (
    <Notice tone="refusal" title={t.buchungen.nichtAngenommen(DIENST.accountingd)}>
      {error instanceof Abgelehnt ? error.message : nichtErreichbar("accountingd")}
    </Notice>
  );
}

const ISO_DATUM = /^\d{4}-\d{2}-\d{2}$/;

/** A proposal this form can take: a date to book on, or a reason to discard. */
function nehmbar(a: Answer | undefined): { run: string; v: Vorschlag } | null {
  const v = a?.vorschlag;
  if (!a || !v) return null;
  if (v.aktion === "neu_buchen" && v.buchungsdatum && ISO_DATUM.test(v.buchungsdatum))
    return { run: a.run, v };
  if (v.aktion === "verwerfen" && v.grund?.trim()) return { run: a.run, v };
  return null;
}

/**
 * The decision on a geparkte Buchung: book it again on an open date — the
 * first one the service names is filled in — or discard it with a reason.
 * Either is final and recorded with the person; each is held for undo before
 * it is sent and asks no confirmation.
 *
 * The assistant's proposal, when one waits, is taken in at once — its action
 * chosen, its date or Grund filled in, each marked as its suggestion beside
 * the Begründung, the read it rests on and agentd's check of it. Nothing is
 * sent until the person sends it; what became of it is recorded beside its run.
 */
export function GeparktEntscheiden({
  b,
  onDone,
  anfang = {},
  vorschlag,
  bilanz,
}: {
  b: GeparkteBuchung;
  onDone: () => void;
  anfang?: Anfang;
  /** The proposal agentd prepared for this booking, read with the list's; none while unread. */
  vorschlag?: Answer | undefined;
  /** The assistant's record for the card's line; read from the catalog when absent. */
  bilanz?: Bilanz | null | undefined;
}) {
  const t = useT();
  const k = t.buchungen;
  const verweigert = useVerweigert({ dienst: "accountingd", aktion: "buchen" });
  const erster = b.vorschlag_datum ?? null;
  const [aktion, setAktion] = useState<Aktion | null>(anfang.aktion ?? (erster ? "buchen" : null));
  const [datum, setDatum] = useState<string>(erster ?? "");
  const [grund, setGrund] = useState(anfang.grund ?? "");
  const [gehalten, setGehalten] = useState(anfang.gehalten ?? false);
  // The person chose or typed: a proposal read later no longer overwrites it.
  const [beruehrt, setBeruehrt] = useState(Boolean(anfang.aktion));
  const [herkunft, setHerkunft] = useState<{ run: string; v: Vorschlag } | null>(null);
  const [beiseite, setBeiseite] = useState(false);
  const now = useNow();
  const was = k.was(buchungsart(b.art), formatCent(b.betrag_ct));
  const wartet = nehmbar(vorschlag);

  /** The form says what the proposal says — nothing of it was changed. */
  const stimmt = (v: Vorschlag) =>
    v.aktion === "neu_buchen"
      ? aktion === "buchen" && datum === v.buchungsdatum
      : aktion === "verwerfen" && grund.trim() === v.grund?.trim();
  const vomAssistenten = herkunft !== null && stimmt(herkunft.v);

  function uebernehmen(v: Vorschlag, run: string) {
    setHerkunft({ run, v });
    if (v.aktion === "neu_buchen") {
      setAktion("buchen");
      setDatum(v.buchungsdatum ?? "");
    } else {
      setAktion("verwerfen");
      setGrund(v.grund ?? "");
    }
  }

  // Taken in once it is read, unless the person already decided otherwise.
  useEffect(() => {
    if (wartet && !herkunft && !beruehrt && !beiseite) uebernehmen(wartet.v, wartet.run);
  });

  /** The person set the proposal aside: the form returns to the service's own default. */
  function beiseiteLegen(ja: boolean) {
    setBeiseite(ja);
    if (!ja || !herkunft) return;
    if (stimmt(herkunft.v)) {
      setAktion(erster ? "buchen" : null);
      setDatum(erster ?? "");
      setGrund("");
    }
    setHerkunft(null);
  }

  // The request and its hold are the action registry's, the ones ⌘K uses too.
  const ding: Entscheidbar = { art: "buchung", b };
  const halte = useHalten();
  const fehler = useFehlschlag(schluessel(ding))?.error ?? anfang.fehler ?? null;
  const gewaehlt = aktion ? registriert(ding, aktion) : undefined;

  function senden() {
    if (!gewaehlt || !bereit) return;
    setGehalten(true);
    halte(
      ding,
      gewaehlt,
      { code: null, text: aktion === "verwerfen" ? grund : datum },
      {
        herkunft,
        gesendet: onDone,
        zurueck: () => setGehalten(false),
        fehlgeschlagen: () => setGehalten(false),
      },
    );
  }

  const bereit =
    aktion === "buchen"
      ? ISO_DATUM.test(datum)
      : aktion === "verwerfen"
        ? grund.trim().length > 0
        : false;

  function waehlen(a: Aktion) {
    setAktion(a);
    setBeruehrt(true);
  }

  useEffect(() => {
    if (verweigert || gehalten) return;
    const onKey = (e: KeyboardEvent) => {
      if (UEBERNEHMEN.match?.(e) && wartet && !beiseite && !stimmt(wartet.v)) {
        e.preventDefault();
        uebernehmen(wartet.v, wartet.run);
        return;
      }
      const a = perTaste(ding, e)?.id;
      if (a !== "buchen" && a !== "verwerfen") return;
      e.preventDefault();
      waehlen(a);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <Panel className="flex flex-col gap-4 p-5">
      <h3 className="m-0 text-callout font-semibold">{t.entscheidung.titel}</h3>
      <p className="m-0 text-body text-text">{b.hinweis}</p>
      <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-body">
        <dt className="text-text-2">{k.buchung}</dt>
        <dd className="m-0 text-text">
          {was}
          {b.beleg && <span className="ml-2 font-mono text-caption text-text-2">{b.beleg}</span>}
        </dd>
        <dt className="text-text-2">{k.abgelehntFuer}</dt>
        <dd className="m-0 text-text">
          {formatBusinessDate(b.buchungsdatum)}
          <span className="ml-2 text-caption text-text-2">
            {b.meldung}
            {b.code !== null && k.code(`${b.code}`)}
          </span>
        </dd>
        <dt className="text-text-2">{k.geparkt}</dt>
        <dd className="m-0 text-text">
          {seit(b.seit, now)}
          <span className="ml-2 text-caption text-text-2">{formatInstant(b.seit)}</span>
        </dd>
      </dl>
      {verweigert ? (
        <Notice tone="refusal" title={k.nichtErlaubt(DIENST.accountingd)}>
          {verweigert}
        </Notice>
      ) : gehalten && gewaehlt ? (
        <Gehalten text={gewaehlt.gehalten} />
      ) : (
        <>
          {wartet && !beiseite && vorschlag && (
            <VorschlagKarte
              v={wartet.v}
              run={wartet.run}
              disclosure={vorschlag.disclosure}
              bilanz={bilanz}
              onUebernehmen={stimmt(wartet.v) ? undefined : uebernehmen}
              onVerwerfen={beiseiteLegen}
              hinweis={stimmt(wartet.v) ? k.uebernommen : undefined}
            />
          )}
          {!wartet && vorschlag?.status === "running" && (
            <p role="status" className="m-0 flex items-center gap-2 text-caption text-ai">
              <Icon name="sparkle" size={12} className="animate-pulse" />
              {k.prueft}
            </p>
          )}
          <RadioGroup
            aria-label={t.entscheidung.titel}
            value={aktion}
            onChange={(v) => waehlen(v as Aktion)}
            orientation="horizontal"
            className="flex gap-2"
          >
            <Radio value="buchen" className={WAHL}>
              {k.neuBuchen}
            </Radio>
            <Radio value="verwerfen" className={WAHL}>
              {k.verwerfen}
            </Radio>
          </RadioGroup>

          {aktion === "buchen" && (
            <TextField
              value={datum}
              onChange={(v) => {
                setDatum(v);
                setBeruehrt(true);
              }}
              isRequired
              className="flex flex-col gap-1"
            >
              <Label className="text-caption text-text-2">{k.neuBuchenAm}</Label>
              <Input type="date" className={`${FELD} w-48`} />
              {vomAssistenten && <VomAssistenten>{k.datumVorgeschlagen}</VomAssistenten>}
              {erster && (
                <span className="text-caption text-text-2">
                  {datum === erster ? k.ersterOffen : k.ersterOffenAm(formatBusinessDate(erster))}
                </span>
              )}
            </TextField>
          )}

          {aktion === "verwerfen" && (
            <TextField
              value={grund}
              onChange={(v) => {
                setGrund(v);
                setBeruehrt(true);
              }}
              isRequired
              className="flex flex-col gap-1"
            >
              <Label className="text-caption text-text-2">{k.warumVerworfen}</Label>
              <TextArea rows={2} maxLength={512} className={FELD} />
              {vomAssistenten && <VomAssistenten>{k.grundVorgeschlagen}</VomAssistenten>}
            </TextField>
          )}

          <Fehler error={fehler} />

          <div className="flex flex-wrap items-center justify-end gap-3">
            {gewaehlt && <p className="m-0 flex-1 text-caption text-text-2">{gewaehlt.was}</p>}
            <AriaButton isDisabled={!bereit} onPress={senden} className={PRIMAER}>
              {aktion === "verwerfen" ? k.buchungVerwerfen : k.neuBuchen}
            </AriaButton>
          </div>
        </>
      )}
    </Panel>
  );
}
