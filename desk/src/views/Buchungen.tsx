import { useMutation } from "@tanstack/react-query";
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
import { DIENST, nichtErreichbar } from "../app/dienste";
import { ABLEHNEN, FREIGEBEN } from "../app/keys";
import { buchungsart, ErneutGeparkt, neuBuchen, verwerfen } from "../data/buchung";
import type { GeparkteBuchung } from "../data/gen/accountingd";
import { Abgelehnt, SchonErledigt } from "../data/rechnung";
import { useVerweigert, Verweigert } from "../data/verweigert";
import { Notice, Panel } from "../design/Panel";
import type { BusinessDate, Instant } from "../format/brands";
import { formatBusinessDate, formatInstant } from "../format/date";
import { seit, useNow } from "../format/frist";
import { formatCent } from "../format/money";
import { Bestaetigen } from "./Rechnungen";

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
  bestaetigen?: boolean;
  fehler?: unknown;
}

/** accountingd's answer to a resolution it did not take, in its words. */
function Fehler({ error }: { error: unknown }) {
  if (!error || error instanceof Verweigert) return null;
  if (error instanceof ErneutGeparkt)
    return (
      <Notice tone="info" title="Erneut geparkt">
        {error.message} — das Hauptbuch hat auch das neue Datum abgelehnt; die Buchung steht gleich
        wieder in der Liste.
      </Notice>
    );
  if (error instanceof SchonErledigt)
    return (
      <Notice tone="info" title="Schon erledigt">
        {error.message} Es wurde nichts gebucht.
      </Notice>
    );
  return (
    <Notice tone="refusal" title={`${DIENST.accountingd}: Buchung nicht angenommen`}>
      {error instanceof Abgelehnt ? error.message : nichtErreichbar("accountingd")}
    </Notice>
  );
}

/**
 * The decision on a geparkte Buchung: book it again on an open date — the
 * first one the service names is filled in — or discard it with a reason.
 * Either is final and recorded with the person, so each is confirmed.
 */
export function GeparktEntscheiden({
  b,
  onDone,
  anfang = {},
}: {
  b: GeparkteBuchung;
  onDone: () => void;
  anfang?: Anfang;
}) {
  const verweigert = useVerweigert({ dienst: "accountingd", aktion: "buchen" });
  const vorschlag = b.vorschlag_datum ?? null;
  const [aktion, setAktion] = useState<Aktion | null>(
    anfang.aktion ?? (vorschlag ? "buchen" : null),
  );
  const [datum, setDatum] = useState<string>(vorschlag ?? "");
  const [grund, setGrund] = useState(anfang.grund ?? "");
  const [bestaetigen, setBestaetigen] = useState(anfang.bestaetigen ?? false);
  const [fehler, setFehler] = useState<unknown>(anfang.fehler ?? null);
  const now = useNow();
  const was = `${buchungsart(b.art)} über ${formatCent(b.betrag_ct)}`;

  const senden = useMutation({
    mutationFn: () =>
      aktion === "verwerfen" ? verwerfen(b.key, grund) : neuBuchen(b.key, datum as BusinessDate),
    onSuccess: onDone,
    onError: (e) => {
      setBestaetigen(false);
      setFehler(e);
    },
  });

  const bereit =
    aktion === "buchen"
      ? /^\d{4}-\d{2}-\d{2}$/.test(datum)
      : aktion === "verwerfen"
        ? grund.trim().length > 0
        : false;

  function waehlen(a: Aktion) {
    setAktion(a);
    setBestaetigen(false);
    setFehler(null);
  }

  useEffect(() => {
    if (verweigert || senden.isPending) return;
    const onKey = (e: KeyboardEvent) => {
      const a = FREIGEBEN.match?.(e) ? "buchen" : ABLEHNEN.match?.(e) ? "verwerfen" : null;
      if (!a) return;
      e.preventDefault();
      waehlen(a);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <Panel className="flex flex-col gap-4 p-5">
      <h3 className="m-0 text-callout font-semibold">Entscheidung</h3>
      <p className="m-0 text-body text-text">{b.hinweis}</p>
      <dl className="m-0 grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-body">
        <dt className="text-text-2">Buchung</dt>
        <dd className="m-0 text-text">
          {was}
          {b.beleg && <span className="ml-2 font-mono text-caption text-text-2">{b.beleg}</span>}
        </dd>
        <dt className="text-text-2">Abgelehnt für</dt>
        <dd className="m-0 text-text">
          {formatBusinessDate(b.buchungsdatum)}
          <span className="ml-2 text-caption text-text-2">
            {b.meldung}
            {b.code !== null && ` (Code ${b.code})`}
          </span>
        </dd>
        <dt className="text-text-2">Geparkt</dt>
        <dd className="m-0 text-text">
          {seit(b.seit, now)}
          <span className="ml-2 text-caption text-text-2">{formatInstant(b.seit)}</span>
        </dd>
      </dl>
      {verweigert ? (
        <Notice tone="refusal" title={`${DIENST.accountingd}: Sie dürfen hier nicht buchen`}>
          {verweigert}
        </Notice>
      ) : (
        <>
          <RadioGroup
            aria-label="Entscheidung"
            value={aktion}
            onChange={(v) => waehlen(v as Aktion)}
            orientation="horizontal"
            className="flex gap-2"
          >
            <Radio value="buchen" className={WAHL}>
              Neu buchen
            </Radio>
            <Radio value="verwerfen" className={WAHL}>
              Verwerfen
            </Radio>
          </RadioGroup>

          {aktion === "buchen" && (
            <TextField
              value={datum}
              onChange={(v) => {
                setDatum(v);
                setBestaetigen(false);
              }}
              isRequired
              className="flex flex-col gap-1"
            >
              <Label className="text-caption text-text-2">Neu buchen am</Label>
              <Input type="date" className={`${FELD} w-48`} />
              {vorschlag && (
                <span className="text-caption text-text-2">
                  {datum === vorschlag
                    ? "Der erste offene Tag im Hauptbuch."
                    : `Der erste offene Tag ist der ${formatBusinessDate(vorschlag)}.`}
                </span>
              )}
            </TextField>
          )}

          {aktion === "verwerfen" && (
            <TextField
              value={grund}
              onChange={(v) => {
                setGrund(v);
                setBestaetigen(false);
              }}
              isRequired
              className="flex flex-col gap-1"
            >
              <Label className="text-caption text-text-2">
                Warum wird sie verworfen? (Grund, Pflicht)
              </Label>
              <TextArea rows={2} maxLength={512} className={FELD} />
            </TextField>
          )}

          <Fehler error={fehler} />

          {bestaetigen && aktion ? (
            <Bestaetigen
              frage={aktion === "buchen" ? "Buchung bestätigen" : "Verwerfen bestätigen"}
              text={
                aktion === "buchen"
                  ? `Das bucht die ${was} am ${formatBusinessDate(datum as BusinessDate)} ins Hauptbuch — zurück nimmt sie nur eine Gegenbuchung.`
                  : `Das verwirft die ${was} endgültig; sie wird nicht gebucht, und der Grund bleibt mit ihr im Prüfpfad.`
              }
              los={aktion === "buchen" ? "Jetzt buchen" : "Jetzt verwerfen"}
              laeuft={aktion === "buchen" ? "Bucht …" : "Verwirft …"}
              pending={senden.isPending}
              onAbbrechen={() => setBestaetigen(false)}
              onSenden={() => senden.mutate()}
            />
          ) : (
            <div className="flex justify-end">
              <AriaButton
                isDisabled={!bereit}
                onPress={() => {
                  setFehler(null);
                  setBestaetigen(true);
                }}
                className={PRIMAER}
              >
                {aktion === "verwerfen" ? "Verwerfen …" : "Neu buchen …"}
              </AriaButton>
            </div>
          )}
        </>
      )}
    </Panel>
  );
}
