import { useQuery } from "@tanstack/react-query";
import { lazy, Suspense, useEffect, useRef, useState } from "react";
import {
  Button as AriaButton,
  Label,
  Radio,
  RadioGroup,
  TextArea,
  TextField,
} from "react-aria-components";
import {
  aktion,
  type Entscheidbar,
  perTaste,
  schluessel,
  useFehlschlag,
  useHalten,
} from "../app/aktionen";
import { DIENST, nichtErreichbar } from "../app/dienste";
import { FaktFelder } from "../app/FaktFelder";
import { useImDrift } from "../app/imDrift";
import { UEBERNEHMEN } from "../app/keys";
import { ROLLEN, useRolle } from "../app/rollen";
import type { Pruefvermerk, Vorschlag } from "../data/agent";
import { Abgelehnt, SchonEntschieden } from "../data/freigabe";
import type { Freigabe } from "../data/gen/processd";
import { call } from "../data/mcp";
import { useVerweigert, Verweigert } from "../data/verweigert";
import { Icon } from "../design/Icon";
import { Notice, Panel } from "../design/Panel";
import { SourceChip } from "../design/SourceChip";
import type { Instant } from "../format/brands";
import { formatInstant } from "../format/date";
import { type Katalog, useT } from "../i18n";
import { useVorschlaege } from "../inbox/quellen";
import { useDarstellbar, useEntscheidungsprobe } from "./entscheidungsprobe";
import { Fragen, VorschlagKarte } from "./Fragen";
import { Gehalten } from "./Rechnungen";

export const ROW =
  "focus-ring flex cursor-default items-center gap-3 border-b border-separator px-4 py-3 outline-none last:border-b-0 data-[focused]:bg-fill data-[hovered]:bg-fill data-[selected]:bg-fill-strong";

export function useFreigaben() {
  const { rolle } = useRolle();
  const marktrolle = ROLLEN[rolle].marktrolle;
  return useQuery({
    queryKey: [rolle, "processd", "list_pending_approvals", marktrolle],
    queryFn: () => call("processd", "list_pending_approvals", { marktrolle }),
    refetchInterval: 30_000,
  });
}

/** The count the sidebar shows beside „Freigaben" — processd's, not a page's. */
export function useFreigabenZahl(): number | undefined {
  return useFreigaben().data?.count;
}

// The rendering brings makod's message table; it loads with the first choice.
const FreigabeVorschau = lazy(() =>
  import("./FreigabeVorschau").then((m) => ({ default: m.FreigabeVorschau })),
);

/**
 * processd's refusal of a decision: the catalogue's sentence for its code,
 * the rules and the stage it names, the service's own text under „Details".
 */
export function EntscheidungAbgelehnt({ fehler }: { fehler: Abgelehnt }) {
  const t = useT();
  const satz = fehler.code ? t.codes[fehler.code as keyof typeof t.codes] : undefined;
  return (
    <>
      {satz ?? fehler.message}
      {fehler.regeln.length > 0 && (
        <span className="mt-1 flex flex-wrap gap-1.5">
          {fehler.regeln.map((r) => (
            <SourceChip key={r} quelle={{ dokument: r }} />
          ))}
        </span>
      )}
      {satz && (
        <details className="mt-1">
          <summary className="cursor-default text-caption">{t.refusal.details}</summary>
          <span lang="en" className="block font-mono text-callout">
            {[fehler.stufe, fehler.grund, fehler.message].filter(Boolean).join(" · ")}
          </span>
        </details>
      )}
    </>
  );
}

/** Why a proposal cannot be taken in unchanged, in the gate's terms. */
function pruefSatz(pv: Pruefvermerk | null, t: Katalog): string {
  const k = t.prozessAnsicht;
  if (!pv) return k.nichtGeprueft(null);
  const abgelehnt = pv.stufen.find((s) => s.urteil === "abgelehnt");
  return pv.urteil === "abgelehnt"
    ? k.abgelehntWeil([abgelehnt?.regel, abgelehnt?.text].filter(Boolean).join(": ") || k.ohneGrund)
    : k.nichtGeprueft(pv.grund);
}

/** Where a proposal came from, so sending it can say whether it was changed. */
interface Herkunft {
  run: string;
  v: Vorschlag;
}

/**
 * The decision on one Freigabe — action, code, Bemerkung — with the assistant
 * beside it. Freigaben and Heute both decide through it. Sending holds it for
 * undo, the market message included: no dialog asks, the hold is the way back.
 */
export function Entscheiden({
  f,
  onDone,
  sofort = false,
}: {
  f: Freigabe;
  onDone: () => void;
  /** Take the waiting proposal into the form as soon as it is there (`U` on a Heute row). */
  sofort?: boolean;
}) {
  const t = useT();
  const k = t.freigabenAnsicht;
  const [gewaehlt, setAktion] = useState<"freigeben" | "ablehnen" | null>(null);
  const [code, setCode] = useState<string | null>(null);
  const [bemerkung, setBemerkung] = useState("");
  const [fakten, setFakten] = useState<Record<string, string>>({});
  const [gehalten, setGehalten] = useState(false);
  const [herkunft, setHerkunft] = useState<Herkunft | null>(null);
  // processd's policy refused this person a decision once: the reason stands
  // where the controls were, for every Freigabe, for the rest of the session.
  const verweigert = useVerweigert({ dienst: "processd", aktion: "entscheiden" });
  // processd in drift: its decision might not be the one the form shows.
  const drift = useImDrift("processd");
  // The proposal agentd computed when the Freigabe was queued — read, never
  // asked for: nobody waits on a model here.
  const wartend = useVorschlaege({ freigabe: [f.id] }).freigabe.get(f.id);
  const wartet = wartend?.vorschlag ? { v: wartend.vorschlag, run: wartend.run } : null;

  // processd states, per action, the codes pressing it needs — none where the
  // answer goes as held — by the same rule it checks the code against.
  const codes =
    gewaehlt === "ablehnen"
      ? f.codes_ablehnen
      : gewaehlt === "freigeben"
        ? f.codes_freigeben
        : null;
  const brauchtCode = Boolean(codes);
  const ding: Entscheidbar = { art: "freigabe", f };
  const gewaehlteAktion = gewaehlt ? aktion(ding, gewaehlt) : undefined;
  const vollstaendig = Boolean(gewaehlteAktion?.bereit({ code, text: bemerkung, fakten }));
  // What the choice would send, as processd rehearses it; its hash goes with the decision.
  const probe = useEntscheidungsprobe(f, gewaehlt, code, codes);
  const koerper = probe.data?.body_sha256 ?? null;
  // The gate's verdict on the waiting proposal: only a passed one is taken in
  // unchanged — and only for the body it was checked for.
  const pv = wartend?.pruefvermerk ?? null;
  // Unchanged only where the gate passed it and the services can render what
  // it sends (M84 SR-035): a body nobody can show is never approved as is.
  const darstellbar = useDarstellbar(f, wartend?.vorschlag ?? null);
  const darfUebernehmen = pv?.urteil === "bestanden" && darstellbar === "ja";

  // The request and its hold are the action registry's, the ones ⌘K uses too.
  const halte = useHalten();
  const fehler = useFehlschlag(schluessel(ding))?.error ?? null;
  function senden() {
    if (!gewaehlteAktion || !vollstaendig) return;
    setGehalten(true);
    halte(
      ding,
      gewaehlteAktion,
      { code, text: bemerkung, fakten, koerper },
      {
        herkunft,
        gesendet: onDone,
        zurueck: () => setGehalten(false),
        fehlgeschlagen: () => setGehalten(false),
      },
    );
  }

  // A and R choose the action from the keyboard; the code and the send
  // follow with the arrow keys and ↵, as everywhere else.
  useEffect(() => {
    if (verweigert || gehalten) return;
    const onKey = (e: KeyboardEvent) => {
      if (UEBERNEHMEN.match?.(e) && wartet && darfUebernehmen) {
        e.preventDefault();
        uebernehmen(wartet.v, wartet.run);
        return;
      }
      const a = perTaste(ding, e)?.id;
      if (a !== "freigeben" && a !== "ablehnen") return;
      e.preventDefault();
      setAktion(a);
      setCode(null);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  function uebernehmen(v: Vorschlag, run: string) {
    // agentd answers a Freigabe only with its two actions; anything else is
    // not this form's to take.
    if (v.aktion !== "freigeben" && v.aktion !== "ablehnen") return;
    setHerkunft({ run, v });
    setAktion(v.aktion);
    setCode(v.antwortcode);
  }

  // `U` on the Heute row opened this form: take the proposal once it is read.
  const genommen = useRef(false);
  useEffect(() => {
    if (!sofort || genommen.current || !wartet || !darfUebernehmen) return;
    genommen.current = true;
    uebernehmen(wartet.v, wartet.run);
  });

  if (drift) {
    return (
      <Panel className="flex flex-col gap-4 p-5">
        <h3 className="m-0 text-body font-semibold">{k.entscheidung}</h3>
        <Notice tone="refusal" title={t.drift.keineEntscheidung(DIENST.processd)} />
      </Panel>
    );
  }

  if (verweigert) {
    return (
      <>
        <Panel className="flex flex-col gap-4 p-5">
          <h3 className="m-0 text-body font-semibold">{k.entscheidung}</h3>
          <Notice tone="refusal" title={k.verweigert(DIENST.processd)}>
            {verweigert}
          </Notice>
        </Panel>
        <Fragen anker={{ freigabe: f.id }} />
      </>
    );
  }

  return (
    <>
      <Panel className="flex flex-col gap-4 p-5">
        <h3 className="m-0 text-body font-semibold">{k.entscheidung}</h3>
        {wartet && !herkunft && wartend && (
          <VorschlagKarte
            v={wartet.v}
            run={wartet.run}
            disclosure={wartend.disclosure}
            onUebernehmen={darfUebernehmen ? uebernehmen : undefined}
            hinweis={
              darfUebernehmen
                ? undefined
                : pv?.urteil === "bestanden" && darstellbar === "nein"
                  ? k.keineDarstellung
                  : pv?.urteil === "bestanden"
                    ? k.darstellungLaeuft
                    : pruefSatz(pv, t)
            }
          />
        )}
        {!wartet && wartend?.status === "running" && (
          <p className="m-0 flex items-center gap-2 text-caption text-ai">
            <Icon name="sparkle" size={12} /> {t.heute.bereitetVor}
          </p>
        )}
        {herkunft && (
          <p className="m-0 flex items-center gap-2 text-caption text-ai">
            <Icon name="sparkle" size={12} /> {k.vorgeschlagen}
          </p>
        )}
        {gehalten && gewaehlteAktion ? (
          <Gehalten text={gewaehlteAktion.gehalten} />
        ) : (
          <>
            <RadioGroup
              aria-label={k.entscheidung}
              value={gewaehlt}
              onChange={(v) => {
                setAktion(v as "freigeben" | "ablehnen");
                setCode(null);
              }}
              orientation="horizontal"
              className="flex gap-2"
            >
              {(["freigeben", "ablehnen"] as const).map((a) => (
                <Radio
                  key={a}
                  value={a}
                  isDisabled={a === "freigeben" ? !f.kann_freigeben : !f.kann_ablehnen}
                  className="focus-ring flex h-9 cursor-default items-center rounded-control border border-separator px-4 text-body text-text-2 data-[disabled]:opacity-40 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:bg-[color-mix(in_srgb,var(--accent)_10%,transparent)] data-[selected]:text-accent"
                >
                  {a === "freigeben" ? k.freigeben : k.ablehnen}
                </Radio>
              ))}
            </RadioGroup>

            {gewaehlt === "freigeben" && f.art === "gehalten" && f.antwort && (
              <p className="m-0 text-body text-text">
                {k.festgehalten} <span>{f.antwort.code}</span>
                {f.antwort.bedeutung && <> — {f.antwort.bedeutung}</>}
              </p>
            )}

            {brauchtCode && codes && (
              <RadioGroup value={code} onChange={setCode} className="flex flex-col gap-1.5">
                <Label className="text-caption text-text-2">
                  {k.antwortcodeAus} <span>{codes.ebd}</span>
                </Label>
                {codes.codes.map((c) => (
                  <Radio
                    key={c.code}
                    value={c.code}
                    className="focus-ring flex cursor-default items-start gap-3 rounded-[10px] border border-separator px-3 py-2 data-[hovered]:bg-fill data-[selected]:border-accent"
                  >
                    <span className="text-body font-semibold text-text">{c.code}</span>
                    <span className="text-body text-text-2">{c.bedeutung}</span>
                  </Radio>
                ))}
              </RadioGroup>
            )}

            {gewaehlteAktion?.fakten && gewaehlteAktion.fakten.length > 0 && (
              <FaktFelder
                fakten={gewaehlteAktion.fakten}
                werte={fakten}
                onChange={(feld, wert) => setFakten({ ...fakten, [feld]: wert })}
              />
            )}

            {gewaehlt && (
              <TextField value={bemerkung} onChange={setBemerkung} className="flex flex-col gap-1">
                <Label className="text-caption text-text-2">{k.bemerkung}</Label>
                <TextArea
                  rows={2}
                  maxLength={512}
                  className="focus-ring rounded-control border border-separator bg-surface px-3 py-2 text-body text-text outline-none"
                />
              </TextField>
            )}

            {fehler &&
              !(fehler instanceof Verweigert) &&
              (fehler instanceof SchonEntschieden ? (
                <Notice tone="info" title={k.schonEntschieden}>
                  {k.schonText(
                    fehler.decidedBy ?? t.halten.jemand,
                    fehler.decidedAt ? formatInstant(fehler.decidedAt as Instant) : null,
                    fehler.status === "Rejected",
                  )}
                </Notice>
              ) : (
                <Notice tone="refusal" title={k.nichtAngenommen(DIENST.processd)}>
                  {fehler instanceof Abgelehnt ? (
                    <EntscheidungAbgelehnt fehler={fehler} />
                  ) : (
                    nichtErreichbar("processd")
                  )}
                </Notice>
              ))}

            {probe.data && (
              <Suspense fallback={<div className="skeleton h-24" />}>
                <FreigabeVorschau probe={probe.data} />
              </Suspense>
            )}
            {herkunft && pv?.body_sha256 && koerper && pv.body_sha256 !== koerper && (
              <p className="m-0 text-caption text-soon">{k.andererKoerper}</p>
            )}

            <div className="flex flex-wrap items-center justify-end gap-3">
              {gewaehlteAktion && (
                <p className="m-0 flex-1 text-caption text-text-2">{gewaehlteAktion.was}</p>
              )}
              <AriaButton
                isDisabled={!vollstaendig}
                onPress={senden}
                className="focus-ring h-9 rounded-control bg-accent px-4 text-body font-semibold text-on-accent data-[disabled]:opacity-40"
              >
                {gewaehlt === "ablehnen" ? k.ablehnen : k.freigeben}
              </AriaButton>
            </div>
          </>
        )}
      </Panel>
      <Fragen anker={{ freigabe: f.id }} onVorschlag={uebernehmen} />
    </>
  );
}
