import { useEffect, useRef, useState } from "react";
import {
  Button as AriaButton,
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
import { FREIGEBEN, UEBERNEHMEN, UNDO } from "../app/keys";
import type { Vorschlag } from "../data/agent";
import type { Pruefposten } from "../data/gen/billingd";
import type { Disput } from "../data/gen/invoicd";
import { Abgelehnt, SchonErledigt } from "../data/rechnung";
import { useVerweigert, Verweigert } from "../data/verweigert";
import { Icon } from "../design/Icon";
import { Notice, Panel } from "../design/Panel";
import { formatCount } from "../format/count";
import { useVorschlaege } from "../inbox/quellen";
import { Fragen, VorschlagKarte } from "./Fragen";

const WAHL =
  "focus-ring flex h-9 cursor-default items-center rounded-control border border-separator px-4 text-body font-medium text-text-2 data-[disabled]:opacity-40 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:bg-[color-mix(in_srgb,var(--accent)_10%,transparent)] data-[selected]:text-accent";
const PRIMAER =
  "focus-ring h-9 rounded-control bg-accent px-4 text-body font-semibold text-on-accent data-[disabled]:opacity-40";

/** The service's answer to a decision it did not take, in its words. */
function Fehler({ error, dienst }: { error: unknown; dienst: "invoicd" | "billingd" }) {
  if (!error || error instanceof Verweigert) return null;
  if (error instanceof SchonErledigt)
    return (
      <Notice tone="info" title="Schon erledigt">
        {error.message} Ihre Entscheidung wurde nicht gesendet.
      </Notice>
    );
  return (
    <Notice tone="refusal" title={`${DIENST[dienst]} hat die Entscheidung nicht angenommen`}>
      {error instanceof Abgelehnt ? error.message : nichtErreichbar(dienst)}
    </Notice>
  );
}

/** A held decision, until it is sent or taken back. */
export function Gehalten({ text }: { text: string }) {
  return (
    <p role="status" className="m-0 flex items-center gap-2 text-callout text-text">
      <Icon name="check" size={14} className="text-accent" />
      {text} — „Rückgängig" oder {UNDO.keys} nimmt es zurück, bevor etwas gesendet wird.
    </p>
  );
}

type DisputAktion = "beilegen" | "ablehnung";

/**
 * The decision on a Rechnungsdisput: close it (Beilegen, with a Bemerkung —
 * sends nothing), or, while the refusal has not gone out, send it (a REMADV).
 * Either is held for undo and asks no confirmation. The proposal agentd
 * prepared, with the finding it rests on, fills the form; the person sends.
 */
export function DisputEntscheiden({
  d,
  onDone,
  sofort = false,
}: {
  d: Disput;
  onDone: () => void;
  /** Take the waiting proposal into the form as soon as it is there (`U` on a Heute row). */
  sofort?: boolean;
}) {
  const [aktion, setAktion] = useState<DisputAktion | null>(null);
  const [bemerkung, setBemerkung] = useState("");
  const [gehalten, setGehalten] = useState(false);
  const [herkunft, setHerkunft] = useState<{ run: string; v: Vorschlag } | null>(null);
  const verweigert = useVerweigert({ dienst: "invoicd", aktion: "entscheiden" });
  const halte = useHalten();
  const wartend = useVorschlaege({ disput: [d.id] }).disput.get(d.id);
  const wartet = wartend?.vorschlag ? { v: wartend.vorschlag, run: wartend.run } : null;

  /** The form action a proposal names, if any is left to take. */
  const fuer = (v: Vorschlag): DisputAktion | null =>
    v.aktion === "beilegen"
      ? "beilegen"
      : v.aktion === "aufrechterhalten" && !d.abgesendet
        ? "ablehnung"
        : null;

  function uebernehmen(v: Vorschlag, run: string) {
    const a = fuer(v);
    if (!a) return;
    setHerkunft({ run, v });
    setAktion(a);
    // The Begründung stays the model's: the Bemerkung is recorded as the
    // person's own words, so nothing of the proposal is copied into it.
  }

  // Each request and its hold are the action registry's, the ones ⌘K uses too.
  const ding: Entscheidbar = { art: "disput", d };
  const fehler = useFehlschlag(schluessel(ding))?.error ?? null;
  const gewaehlt = aktion ? registriert(ding, aktion) : undefined;

  function senden() {
    if (!gewaehlt) return;
    setGehalten(true);
    halte(
      ding,
      gewaehlt,
      { code: null, text: gewaehlt.id === "beilegen" ? bemerkung : "" },
      {
        herkunft,
        gesendet: onDone,
        zurueck: () => setGehalten(false),
        fehlgeschlagen: () => setGehalten(false),
      },
    );
  }

  useEffect(() => {
    if (verweigert || gehalten) return;
    const onKey = (e: KeyboardEvent) => {
      if (UEBERNEHMEN.match?.(e) && wartet && fuer(wartet.v)) {
        e.preventDefault();
        uebernehmen(wartet.v, wartet.run);
        return;
      }
      const a = perTaste(ding, e)?.id;
      if (a !== "beilegen" && a !== "ablehnung") return;
      e.preventDefault();
      setAktion(a);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  const genommen = useRef(false);
  useEffect(() => {
    if (!sofort || genommen.current || !wartet) return;
    genommen.current = true;
    uebernehmen(wartet.v, wartet.run);
  });

  if (verweigert) {
    return (
      <Panel className="flex flex-col gap-4 p-5">
        <h3 className="m-0 text-callout font-semibold">Entscheidung</h3>
        <Notice tone="refusal" title="Der Rechnungseingang lässt Sie hier nicht entscheiden">
          {verweigert}
        </Notice>
      </Panel>
    );
  }

  return (
    <>
      <Panel className="flex flex-col gap-4 p-5">
        <h3 className="m-0 text-callout font-semibold">Entscheidung</h3>
        {d.befunde.length > 0 && (
          <ul aria-label="Befunde" className="m-0 flex list-none flex-col gap-1.5 p-0">
            {d.befunde.map((b) => (
              <li key={`${b.art}:${b.text}`} className="flex flex-col gap-0.5">
                <span className="text-body font-medium text-text">{b.bezeichnung}</span>
                <span className="text-caption text-text-2">{b.text}</span>
              </li>
            ))}
          </ul>
        )}
        <p className="m-0 text-caption text-text-2">
          {d.abgesendet
            ? "Die Ablehnung (REMADV) ist beim Rechnungssteller."
            : "Die Ablehnung (REMADV) ist noch nicht versandt."}
        </p>
        {wartet && !herkunft && wartend && (
          <VorschlagKarte
            v={wartet.v}
            run={wartet.run}
            disclosure={wartend.disclosure}
            befund={d.befunde.find((b) => b.art === wartet.v.befund)?.bezeichnung}
            onUebernehmen={fuer(wartet.v) ? uebernehmen : undefined}
            hinweis={
              fuer(wartet.v)
                ? undefined
                : "Die Ablehnung ist versandt — bis eine korrigierte Rechnung kommt, ist nichts zu senden."
            }
          />
        )}
        {!wartet && wartend?.status === "running" && (
          <p className="m-0 flex items-center gap-2 text-caption text-ai">
            <Icon name="sparkle" size={12} /> Der Assistent bereitet einen Vorschlag vor …
          </p>
        )}
        {herkunft && (
          <p className="m-0 flex items-center gap-2 text-caption text-ai">
            <Icon name="sparkle" size={12} /> Vom Assistenten vorgeschlagen — prüfen, dann senden.
          </p>
        )}

        {gehalten && gewaehlt ? (
          <Gehalten text={gewaehlt.gehalten} />
        ) : (
          <>
            <RadioGroup
              aria-label="Entscheidung"
              value={aktion}
              onChange={(v) => setAktion(v as DisputAktion)}
              orientation="horizontal"
              className="flex gap-2"
            >
              <Radio value="beilegen" className={WAHL}>
                Beilegen
              </Radio>
              <Radio value="ablehnung" isDisabled={d.abgesendet} className={WAHL}>
                Ablehnung senden
              </Radio>
            </RadioGroup>

            {aktion === "beilegen" && (
              <TextField value={bemerkung} onChange={setBemerkung} className="flex flex-col gap-1">
                <Label className="text-caption text-text-2">
                  Wie wurde er geklärt? (Bemerkung, optional)
                </Label>
                <TextArea
                  rows={2}
                  maxLength={512}
                  className="focus-ring rounded-control border border-separator bg-surface px-3 py-2 text-body text-text outline-none"
                />
              </TextField>
            )}

            <Fehler error={fehler} dienst="invoicd" />

            <div className="flex flex-wrap items-center justify-end gap-3">
              {gewaehlt && <p className="m-0 flex-1 text-caption text-text-2">{gewaehlt.was}</p>}
              <AriaButton isDisabled={!gewaehlt} onPress={senden} className={PRIMAER}>
                {aktion === "ablehnung" ? "Ablehnung senden" : "Beilegen"}
              </AriaButton>
            </div>
          </>
        )}
      </Panel>
      <Fragen anker={{ disput: d.id }} onVorschlag={uebernehmen} />
    </>
  );
}

/**
 * The decision on a Prüfposten. A HELD invoice is released — it is issued. A
 * REVIEW invoice went out when it was banded; „Geprüft" takes it off the list
 * and sends nothing. Either is held for undo and asks no confirmation. The
 * assistant proposes neither: the risk gate's findings are already the evidence.
 */
export function PruefpostenEntscheiden({ r, onDone }: { r: Pruefposten; onDone: () => void }) {
  const gehaltenBand = r.risk_band === "HELD";
  const verweigert = useVerweigert({
    dienst: "billingd",
    aktion: gehaltenBand ? "freigeben" : "pruefen",
  });
  const [gehalten, setGehalten] = useState(false);
  const halte = useHalten();
  // The request and its hold are the action registry's, the ones ⌘K uses too.
  const ding: Entscheidbar = { art: "pruefposten", r };
  const fehler = useFehlschlag(schluessel(ding))?.error ?? null;
  const a = registriert(ding, gehaltenBand ? "freigeben" : "geprueft");

  function senden() {
    if (!a) return;
    setGehalten(true);
    halte(
      ding,
      a,
      { code: null, text: "" },
      {
        gesendet: onDone,
        zurueck: () => setGehalten(false),
        fehlgeschlagen: () => setGehalten(false),
      },
    );
  }

  useEffect(() => {
    if (verweigert || gehalten) return;
    const onKey = (e: KeyboardEvent) => {
      if (!FREIGEBEN.match?.(e)) return;
      e.preventDefault();
      senden();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  return (
    <Panel className="flex flex-col gap-4 p-5">
      <h3 className="m-0 text-callout font-semibold">Entscheidung</h3>
      {r.risk_findings.length > 0 && (
        <ul aria-label="Befunde" className="m-0 flex list-none flex-col gap-1.5 p-0">
          {r.risk_findings.map((f) => (
            <li key={f.code} className="flex items-baseline gap-3 text-body">
              <span className="font-mono text-caption text-text-2">{f.code}</span>
              <span className="flex-1 text-text">{f.message}</span>
              <span className="text-caption text-text-2">+{formatCount(f.weight)}</span>
            </li>
          ))}
        </ul>
      )}
      <p className="m-0 text-caption text-text-2">
        {gehaltenBand
          ? "Angehalten: die Rechnung ist nicht ausgestellt, bis sie freigegeben wird."
          : "Ausgestellt und zur Prüfung vorgelegt; eine Korrektur geht als Stornorechnung."}
      </p>
      {verweigert ? (
        <Notice tone="refusal" title="Die Abrechnung lässt Sie hier nicht entscheiden">
          {verweigert}
        </Notice>
      ) : gehalten && a ? (
        <Gehalten text={a.gehalten} />
      ) : (
        <>
          <Fehler error={fehler} dienst="billingd" />
          <div className="flex flex-wrap items-center justify-end gap-3">
            {a && <p className="m-0 flex-1 text-caption text-text-2">{a.was}</p>}
            <AriaButton onPress={senden} aria-keyshortcuts="A" className={PRIMAER}>
              {gehaltenBand ? "Freigeben" : "Geprüft"}
            </AriaButton>
          </div>
        </>
      )}
    </Panel>
  );
}
