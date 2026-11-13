import { keepPreviousData, useMutation, useQuery } from "@tanstack/react-query";
import { useEffect, useId, useRef, useState } from "react";
import {
  Button as AriaButton,
  Checkbox,
  Disclosure,
  DisclosurePanel,
  Heading,
  Input,
  Label,
  Radio,
  RadioGroup,
  Tab,
  TabList,
  TabPanel,
  Tabs,
  TextField,
} from "react-aria-components";
import { nichtErreichbar } from "../app/dienste";
import { useRolle } from "../app/rollen";
import { AgentAbgelehnt, AgentUnavailable, type Angaben, antwort, vorbelegen } from "../data/agent";
import {
  call,
  ServiceUnavailable,
  type ToolInput,
  type ToolOutput,
  ToolRefusal,
} from "../data/mcp";
import { Verweigert } from "../data/verweigert";
import { AiNote } from "../design/AiNote";
import { Button } from "../design/Button";
import { Icon } from "../design/Icon";
import { EmptyState, Notice, Panel } from "../design/Panel";
import { SourceChip } from "../design/SourceChip";
import { formatCount } from "../format/count";
import { eintrag, useT } from "../i18n";

/** The configuration makod proposes for the interview's answers. */
export type Vorschlag = ToolOutput<"makod", "einrichtung_vorschlag">;
/** The interview's answers, as makod's tool takes them. */
export type Antworten = ToolInput<"makod", "einrichtung_vorschlag">["antworten"];
export type Datei = Vorschlag["dateien"][number];
export type Zeile = Datei["zeilen"][number];
type RolleToken = NonNullable<Antworten["rollen"]>[number]["rolle"];
type SparteToken = NonNullable<Antworten["bilanzkreise"]>[number]["sparte"];
type As4 = NonNullable<Antworten["as4"]>;
type Schluessel = NonNullable<As4["schluessel"]>[number];

const ROLLEN: RolleToken[] = [
  "lf-strom",
  "lf-gas",
  "nb-strom",
  "nb-gas",
  "msb-strom",
  "msb-gas",
  "esa-strom",
];
const LIEFERANT: Partial<Record<RolleToken, SparteToken>> = {
  "lf-strom": "strom",
  "lf-gas": "gas",
};
const SCHLUESSEL_FELDER = [
  "signing_cert_pem_file",
  "signing_key_pem_file",
  "encryption_cert_pem_file",
  "encryption_key_pem_file",
  "tls_cert_pem_file",
  "tls_key_pem_file",
] as const;

/** Where a finding of a Bereich is answered in the interview. */
export const FRAGE_ZUM_BEREICH: Record<string, string> = {
  identitaeten: "rollen",
  bilanzierung: "bilanzkreise",
  zertifikate: "as4",
};

const SPRUNG = "mako-desk:frage";

/** Take the reader to the interview's question `frage` (`rollen.lf-gas`, `as4`, …). */
export function zurFrage(frage: string) {
  window.dispatchEvent(new CustomEvent(SPRUNG, { detail: frage }));
}

/** A list typed as text: the entries between commas, trimmed, empty ones dropped. */
function eintraege(text: string): string[] {
  return text
    .split(",")
    .map((s) => s.trim())
    .filter((s) => s.length > 0);
}

/** `value` after `ms` without a change — what is asked of makod while the reader types. */
function useRuhig<T>(value: T, ms: number): T {
  const [ruhig, setRuhig] = useState(value);
  useEffect(() => {
    const h = setTimeout(() => setRuhig(value), ms);
    return () => clearTimeout(h);
  }, [value, ms]);
  return ruhig;
}

/**
 * The setup interview and the configuration makod proposes from it: one file
 * per service, every line with its source, every unanswered fact a gap. The
 * Desk asks; makod builds the proposal; nothing is written to a server.
 */
export function Befragung() {
  const t = useT();
  const b = t.einrichtung.befragung;
  const { rolle } = useRolle();
  const titel = useId();
  const [antworten, setAntworten] = useState<Antworten | null>(null);
  const [as4Offen, setAs4Offen] = useState(false);
  const wurzel = useRef<HTMLDivElement>(null);
  const gefragt = useRuhig(antworten, 250);

  const q = useQuery({
    queryKey: [rolle, "makod", "einrichtung_vorschlag", gefragt ?? {}],
    queryFn: () => call("makod", "einrichtung_vorschlag", { antworten: gefragt ?? {} }),
    placeholderData: keepPreviousData,
  });

  // The interview starts from what the deployment already runs with.
  useEffect(() => {
    if (antworten === null && q.data && !q.isPlaceholderData) setAntworten(q.data.vorbelegt);
  }, [antworten, q.data, q.isPlaceholderData]);

  useEffect(() => {
    const springen = (e: Event) => {
      const frage = (e as CustomEvent<string>).detail;
      if (frage.startsWith("as4")) setAs4Offen(true);
      requestAnimationFrame(() => {
        const ziel =
          wurzel.current?.querySelector<HTMLElement>(`[data-frage="${frage}"]`) ??
          wurzel.current?.querySelector<HTMLElement>(`[data-frage="${frage.split(".")[0]}"]`);
        if (!ziel) return;
        ziel.scrollIntoView?.({ block: "center" });
        (ziel.querySelector<HTMLElement>("input, button") ?? ziel).focus();
      });
    };
    window.addEventListener(SPRUNG, springen);
    return () => window.removeEventListener(SPRUNG, springen);
  }, []);

  const a = antworten ?? {};
  const setze = (f: (a: Antworten) => Antworten) => setAntworten((alt) => f(alt ?? {}));

  return (
    <Panel className="flex flex-col gap-5 p-4 md:p-6">
      <section aria-labelledby={titel} className="contents">
        <div>
          <h2 id={titel} className="m-0 text-title font-semibold">
            {b.titel}
          </h2>
          <p className="m-0 mt-1 max-w-3xl text-body text-text-2">{b.text}</p>
        </div>
        <div className="grid gap-6 lg:grid-cols-[minmax(0,5fr)_minmax(0,6fr)]">
          <div ref={wurzel} className="flex min-w-0 flex-col gap-5">
            <Vorbelegen onUebernehmen={setze} />
            <Fragen a={a} setze={setze} as4Offen={as4Offen} setAs4Offen={setAs4Offen} />
          </div>
          <div className="min-w-0 lg:sticky lg:top-4 lg:self-start">
            <Ergebnis
              vorschlag={antworten === null ? undefined : q.data}
              laedt={q.isFetching}
              fehler={q.error}
            />
          </div>
        </div>
      </section>
    </Panel>
  );
}

// ── The questions ───────────────────────────────────────────────────────────

function Abschnitt({
  frage,
  titel,
  text,
  children,
}: {
  frage: string;
  titel: string;
  text?: string;
  children: React.ReactNode;
}) {
  const id = useId();
  return (
    <fieldset
      data-frage={frage}
      tabIndex={-1}
      aria-labelledby={id}
      className="m-0 flex min-w-0 flex-col gap-2.5 border-0 p-0 outline-none"
    >
      <legend id={id} className="contents">
        <span className="text-callout font-semibold text-text">{titel}</span>
      </legend>
      {text && <p className="m-0 text-caption text-text-2">{text}</p>}
      {children}
    </fieldset>
  );
}

const FELD =
  "focus-ring squircle h-8 w-full min-w-0 rounded-control border border-separator bg-surface px-2.5 font-mono text-body text-text outline-none placeholder:text-text-2/60";

function Feld({
  label,
  value,
  onChange,
  frage,
}: {
  label: string;
  value: string;
  onChange: (v: string) => void;
  frage?: string | undefined;
}) {
  return (
    <TextField
      value={value}
      onChange={onChange}
      className="flex min-w-0 flex-col gap-1"
      data-frage={frage}
    >
      <Label className="text-caption font-medium text-text-2">{label}</Label>
      <Input className={FELD} spellCheck={false} autoComplete="off" />
    </TextField>
  );
}

/** A comma list kept as the reader types it, handed on as its entries. */
function ListenFeld({
  label,
  werte,
  onChange,
  frage,
}: {
  label: string;
  werte: string[];
  onChange: (w: string[]) => void;
  frage?: string | undefined;
}) {
  const [text, setText] = useState(werte.join(", "));
  useEffect(() => {
    setText((alt) => (eintraege(alt).join("\n") === werte.join("\n") ? alt : werte.join(", ")));
  }, [werte]);
  return (
    <Feld
      label={label}
      value={text}
      frage={frage}
      onChange={(v) => {
        setText(v);
        onChange(eintraege(v));
      }}
    />
  );
}

function Haken({
  isSelected,
  onChange,
  children,
}: {
  isSelected: boolean;
  onChange: (v: boolean) => void;
  children: React.ReactNode;
}) {
  return (
    <Checkbox
      isSelected={isSelected}
      onChange={onChange}
      className="group focus-ring flex min-h-8 cursor-default items-center gap-2 rounded-control text-body text-text"
    >
      <span className="squircle flex size-4.5 shrink-0 items-center justify-center rounded-[5px] border border-separator bg-surface text-on-accent group-data-[selected]:border-accent group-data-[selected]:bg-accent">
        <Icon name="check" size={12} className="opacity-0 group-data-[selected]:opacity-100" />
      </span>
      {children}
    </Checkbox>
  );
}

function Fragen({
  a,
  setze,
  as4Offen,
  setAs4Offen,
}: {
  a: Antworten;
  setze: (f: (a: Antworten) => Antworten) => void;
  as4Offen: boolean;
  setAs4Offen: (v: boolean) => void;
}) {
  const t = useT();
  const b = t.einrichtung.befragung;
  const rollen = a.rollen ?? [];
  const gewaehlt = (r: RolleToken) => rollen.find((x) => x.rolle === r);
  const mpIds = [...new Set(rollen.map((r) => r.mp_id?.trim() ?? "").filter((m) => m.length > 0))];
  const lieferanten = ROLLEN.filter((r) => LIEFERANT[r] && gewaehlt(r)).map(
    (r) => LIEFERANT[r] as SparteToken,
  );
  const bk = (s: SparteToken) => (a.bilanzkreise ?? []).find((x) => x.sparte === s)?.standard ?? [];
  const as4 = a.as4 ?? null;
  const schluessel = (mp: string): Schluessel =>
    as4?.schluessel?.find((k) => k.mp_id === mp) ?? { mp_id: mp };
  const setzeAs4 = (f: (x: As4) => As4) => setze((x) => ({ ...x, as4: f(x.as4 ?? {}) }));

  return (
    <>
      <Abschnitt frage="rollen" titel={b.rollen.titel} text={b.rollen.text}>
        <ul className="m-0 flex list-none flex-col gap-1.5 p-0">
          {ROLLEN.map((r) => {
            const g = gewaehlt(r);
            const name = eintrag(b.rollen.namen, r);
            return (
              <li key={r} data-frage={`rollen.${r}`} className="flex flex-col gap-1.5">
                <Haken
                  isSelected={g !== undefined}
                  onChange={(an) =>
                    setze((x) => ({
                      ...x,
                      rollen: an
                        ? [...(x.rollen ?? []), { rolle: r, mp_id: null }]
                        : (x.rollen ?? []).filter((y) => y.rolle !== r),
                    }))
                  }
                >
                  {name}
                </Haken>
                {g && (
                  <div className="pl-6.5">
                    <Feld
                      label={b.rollen.mpId(name)}
                      value={g.mp_id ?? ""}
                      onChange={(v) =>
                        setze((x) => ({
                          ...x,
                          rollen: (x.rollen ?? []).map((y) =>
                            y.rolle === r ? { ...y, mp_id: v.trim() === "" ? null : v } : y,
                          ),
                        }))
                      }
                    />
                  </div>
                )}
              </li>
            );
          })}
        </ul>
      </Abschnitt>

      {mpIds.length > 1 && (
        <Abschnitt frage="primaer" titel={b.primaer.titel} text={b.primaer.text}>
          <RadioGroup
            aria-label={b.primaer.titel}
            value={a.primaer ?? null}
            onChange={(v) => setze((x) => ({ ...x, primaer: v }))}
            className="flex flex-wrap gap-2"
          >
            {mpIds.map((m) => (
              <Radio
                key={m}
                value={m}
                className="focus-ring flex h-8 cursor-default items-center rounded-control border border-separator bg-surface px-3 font-mono text-body text-text-2 data-[hovered]:bg-fill data-[selected]:border-accent data-[selected]:text-accent"
              >
                {m}
              </Radio>
            ))}
          </RadioGroup>
        </Abschnitt>
      )}

      <Abschnitt frage="bilanzkreise" titel={b.bilanzkreise.titel} text={b.bilanzkreise.text}>
        {lieferanten.length === 0 && (
          <p className="m-0 text-caption text-text-2">{b.bilanzkreise.keinLieferant}</p>
        )}
        {lieferanten.map((s) => (
          <ListenFeld
            key={s}
            frage={`bilanzkreise.${s}`}
            label={b.bilanzkreise.feld(eintrag(b.bilanzkreise.sparten, s))}
            werte={bk(s)}
            onChange={(w) =>
              setze((x) => ({
                ...x,
                bilanzkreise: [
                  ...(x.bilanzkreise ?? []).filter((y) => y.sparte !== s),
                  { sparte: s, standard: w },
                ],
              }))
            }
          />
        ))}
      </Abschnitt>

      <Abschnitt frage="verbundene_lieferanten" titel={b.verbunden.titel} text={b.verbunden.text}>
        <Haken
          isSelected={
            Array.isArray(a.verbundene_lieferanten) && a.verbundene_lieferanten.length === 0
          }
          onChange={(keine) => setze((x) => ({ ...x, verbundene_lieferanten: keine ? [] : null }))}
        >
          {b.verbunden.keine}
        </Haken>
        <ListenFeld
          label={b.verbunden.feld}
          werte={a.verbundene_lieferanten ?? []}
          onChange={(w) =>
            setze((x) => ({ ...x, verbundene_lieferanten: w.length > 0 ? w : null }))
          }
        />
      </Abschnitt>

      <div data-frage="as4" tabIndex={-1} className="outline-none">
        <Disclosure isExpanded={as4Offen} onExpandedChange={setAs4Offen}>
          <Heading className="m-0">
            <AriaButton
              slot="trigger"
              className="focus-ring flex w-full cursor-default items-center gap-2 rounded-control py-1 text-left text-callout font-semibold text-text"
            >
              <Icon
                name="chevron"
                size={14}
                className={`shrink-0 text-text-2 ${as4Offen ? "rotate-90" : ""}`}
              />
              {b.as4.titel}
            </AriaButton>
          </Heading>
          <DisclosurePanel className="flex flex-col gap-2.5 pt-1.5">
            <p className="m-0 text-caption text-text-2">{b.as4.text}</p>
            <Haken
              isSelected={as4 !== null}
              onChange={(an) => setze((x) => ({ ...x, as4: an ? (x.as4 ?? {}) : null }))}
            >
              {b.as4.angeben}
            </Haken>
            {as4 !== null && (
              <>
                <Feld
                  label={b.as4.domain}
                  value={as4.message_id_domain ?? ""}
                  onChange={(v) => setzeAs4((x) => ({ ...x, message_id_domain: v || null }))}
                />
                <Feld
                  label={b.as4.anker}
                  value={as4.trust_anchor_pem_file ?? ""}
                  onChange={(v) => setzeAs4((x) => ({ ...x, trust_anchor_pem_file: v || null }))}
                />
                <Feld
                  label={b.as4.sperrliste}
                  value={as4.crl_signer_pem_file ?? ""}
                  onChange={(v) => setzeAs4((x) => ({ ...x, crl_signer_pem_file: v || null }))}
                />
                {mpIds.length === 0 && (
                  <p className="m-0 text-caption text-text-2">{b.as4.keineMpId}</p>
                )}
                {mpIds.map((mp) => (
                  <fieldset key={mp} className="m-0 flex flex-col gap-2 border-0 p-0">
                    <legend className="contents">
                      <span className="text-caption font-semibold text-text">
                        {b.as4.schluessel(mp)}
                      </span>
                    </legend>
                    <div className="grid gap-2 sm:grid-cols-2">
                      {SCHLUESSEL_FELDER.map((f) => (
                        <Feld
                          key={f}
                          label={eintrag(b.as4.felder, f)}
                          value={schluessel(mp)[f] ?? ""}
                          onChange={(v) =>
                            setzeAs4((x) => ({
                              ...x,
                              schluessel: [
                                ...(x.schluessel ?? []).filter((k) => k.mp_id !== mp),
                                { ...schluessel(mp), [f]: v || null },
                              ],
                            }))
                          }
                        />
                      ))}
                    </div>
                  </fieldset>
                ))}
              </>
            )}
          </DisclosurePanel>
        </Disclosure>
      </div>

      <Abschnitt frage="dienste" titel={b.dienste.titel} text={b.dienste.text}>
        <ul className="m-0 flex list-none flex-col gap-2 p-0">
          {(a.dienste ?? []).map((d, i) => (
            // A service row has no id until it is named; its place names it.
            // biome-ignore lint/suspicious/noArrayIndexKey: the row's place
            <li key={i} className="grid items-end gap-2 sm:grid-cols-[8rem_minmax(0,1fr)_auto]">
              <Feld
                label={b.dienste.name}
                value={d.name}
                onChange={(v) =>
                  setze((x) => ({
                    ...x,
                    dienste: (x.dienste ?? []).map((y, j) => (j === i ? { ...y, name: v } : y)),
                  }))
                }
              />
              <Feld
                label={b.dienste.url}
                value={d.url}
                onChange={(v) =>
                  setze((x) => ({
                    ...x,
                    dienste: (x.dienste ?? []).map((y, j) => (j === i ? { ...y, url: v } : y)),
                  }))
                }
              />
              <Button
                variant="ghost"
                aria-label={b.dienste.entfernen(d.name)}
                onPress={() =>
                  setze((x) => ({ ...x, dienste: (x.dienste ?? []).filter((_, j) => j !== i) }))
                }
                className="h-8"
              >
                <Icon name="stop" size={14} />
              </Button>
              <div className="sm:col-span-3">
                <Feld
                  label={b.dienste.schluessel}
                  value={d.api_key_file ?? ""}
                  onChange={(v) =>
                    setze((x) => ({
                      ...x,
                      dienste: (x.dienste ?? []).map((y, j) =>
                        j === i ? { ...y, api_key_file: v || null } : y,
                      ),
                    }))
                  }
                />
              </div>
            </li>
          ))}
        </ul>
        <div>
          <Button
            variant="ghost"
            onPress={() =>
              setze((x) => ({ ...x, dienste: [...(x.dienste ?? []), { name: "", url: "" }] }))
            }
          >
            {b.dienste.hinzufuegen}
          </Button>
        </div>
      </Abschnitt>
    </>
  );
}

// ── The pre-fill ─────────────────────────────────────────────────────────────

interface Angebot {
  key: string;
  text: string;
  anwenden: (a: Antworten) => Antworten;
}

/** Each answer the pre-fill read, as one offer the reader takes or discards. */
function angebote(angaben: Angaben, namen: (r: string) => string, b: Befragungstexte): Angebot[] {
  const out: Angebot[] = [];
  for (const r of angaben.rollen) {
    const rolle = r.rolle as RolleToken;
    out.push({
      key: `rolle:${rolle}`,
      text: b.satz.rolle(namen(rolle), r.mp_id),
      anwenden: (a) => ({
        ...a,
        rollen: [
          ...(a.rollen ?? []).filter((x) => x.rolle !== rolle),
          {
            rolle,
            mp_id: r.mp_id ?? (a.rollen ?? []).find((x) => x.rolle === rolle)?.mp_id ?? null,
          },
        ],
      }),
    });
  }
  if (angaben.primaer) {
    const p = angaben.primaer;
    out.push({ key: "primaer", text: b.satz.primaer(p), anwenden: (a) => ({ ...a, primaer: p }) });
  }
  for (const k of angaben.bilanzkreise) {
    const sparte = k.sparte as SparteToken;
    out.push({
      key: `bk:${sparte}`,
      text: b.satz.bilanzkreis(eintrag(b.bilanzkreise.sparten, sparte), k.standard.join(", ")),
      anwenden: (a) => ({
        ...a,
        bilanzkreise: [
          ...(a.bilanzkreise ?? []).filter((x) => x.sparte !== sparte),
          { sparte, standard: k.standard },
        ],
      }),
    });
  }
  if (angaben.verbundene_lieferanten) {
    const v = angaben.verbundene_lieferanten;
    out.push({
      key: "verbunden",
      text: b.satz.verbunden(v.join(", ")),
      anwenden: (a) => ({ ...a, verbundene_lieferanten: v }),
    });
  }
  return out;
}

type Befragungstexte = ReturnType<typeof useT>["einrichtung"]["befragung"];

function Vorbelegen({
  onUebernehmen,
}: {
  onUebernehmen: (f: (a: Antworten) => Antworten) => void;
}) {
  const t = useT();
  const b = t.einrichtung.befragung;
  const [satz, setSatz] = useState("");
  const [erledigt, setErledigt] = useState<Record<string, "uebernommen" | "verworfen">>({});
  const lauf = useMutation<string, Error, string>({ mutationFn: vorbelegen });
  const run = lauf.data;
  const answer = useQuery({
    queryKey: ["agentd", "vorbelegen", run],
    queryFn: () => antwort(run ?? ""),
    enabled: run !== undefined,
    refetchInterval: (q) => (q.state.data?.status === "running" ? 500 : false),
  });
  const fertig = answer.data && answer.data.status !== "running" ? answer.data : undefined;
  const offen = fertig?.angaben
    ? angebote(fertig.angaben, (r) => eintrag(b.rollen.namen, r), b).filter(
        (x) => erledigt[x.key] === undefined,
      )
    : [];
  const laeuft =
    lauf.isPending || answer.data?.status === "running" || (answer.isPending && run !== undefined);
  const fehler = lauf.error ?? answer.error;

  const starten = () => {
    if (satz.trim() && !laeuft) {
      setErledigt({});
      lauf.mutate(satz.trim());
    }
  };
  const nehmen = (xs: Angebot[]) => {
    onUebernehmen((a) => xs.reduce((acc, x) => x.anwenden(acc), a));
    setErledigt((e) => ({ ...e, ...Object.fromEntries(xs.map((x) => [x.key, "uebernommen"])) }));
  };

  return (
    <div className="squircle flex flex-col gap-3 rounded-panel border border-ai-border bg-ai-fill/40 p-3.5">
      <form
        className="flex flex-col gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          starten();
        }}
      >
        <TextField value={satz} onChange={setSatz} className="flex flex-col gap-1">
          <Label className="flex items-center gap-1.5 text-caption font-semibold text-ai">
            <Icon name="sparkle" size={13} />
            {b.satz.titel}
          </Label>
          <Input
            aria-description={b.satz.hinweis}
            placeholder={b.satz.platzhalter}
            className="focus-ring squircle h-9 w-full min-w-0 rounded-control border border-separator bg-surface px-3 text-body text-text outline-none placeholder:text-text-2/60"
          />
        </TextField>
        <div className="flex flex-wrap items-center gap-2">
          <Button type="submit" variant="secondary" isDisabled={!satz.trim() || laeuft}>
            <Icon name="sparkle" size={13} />
            {laeuft ? b.satz.liest : b.satz.vorbelegen}
          </Button>
          <span className="text-caption text-text-2">{b.satz.hinweis}</span>
        </div>
      </form>
      <div aria-live="polite" className="flex flex-col gap-2">
        {fehler && (
          <Notice
            tone="refusal"
            title={
              fehler instanceof AgentUnavailable
                ? b.satz.nichtErreichbar
                : fehler instanceof AgentAbgelehnt || fehler instanceof Verweigert
                  ? b.satz.abgelehnt
                  : b.satz.nichtErreichbar
            }
          >
            {b.satz.nichtErreichbarText}
          </Notice>
        )}
        {fertig && (
          <AiNote
            text={`${fertig.disclosure} ${fertig.angaben && offen.length + Object.keys(erledigt).length > 0 ? b.satz.vorgeschlagen : b.satz.keine}`}
            footer={
              offen.length > 0 ? (
                <div className="flex flex-col gap-2">
                  <ul
                    aria-label={b.satz.vorgeschlagen}
                    className="m-0 flex list-none flex-col gap-1.5 p-0"
                  >
                    {offen.map((x) => (
                      <li key={x.key} className="flex flex-wrap items-center gap-2">
                        <span className="min-w-0 flex-1 basis-40 text-body text-text">
                          {x.text}
                        </span>
                        <div className="flex shrink-0 gap-1">
                          <Button variant="secondary" onPress={() => nehmen([x])}>
                            <Icon name="check" size={13} />
                            {b.satz.uebernehmen}
                          </Button>
                          <Button
                            variant="ghost"
                            onPress={() => setErledigt((e) => ({ ...e, [x.key]: "verworfen" }))}
                          >
                            {b.satz.verwerfen}
                          </Button>
                        </div>
                      </li>
                    ))}
                  </ul>
                  {offen.length > 1 && (
                    <div>
                      <Button variant="primary" onPress={() => nehmen(offen)}>
                        {b.satz.alleUebernehmen}
                      </Button>
                    </div>
                  )}
                </div>
              ) : undefined
            }
          />
        )}
      </div>
    </div>
  );
}

// ── The proposal ────────────────────────────────────────────────────────────

function Ergebnis({
  vorschlag,
  laedt,
  fehler,
}: {
  vorschlag: Vorschlag | undefined;
  laedt: boolean;
  fehler: Error | null;
}) {
  const t = useT();
  const e = t.einrichtung.befragung.ergebnis;
  const c = t.einrichtung.checkliste;
  if (fehler && !vorschlag)
    return (
      <Notice
        tone={fehler instanceof ToolRefusal ? "refusal" : "failure"}
        title={fehler instanceof ServiceUnavailable ? nichtErreichbar(fehler.service) : e.fehler}
      >
        {fehler.message}
      </Notice>
    );
  if (!vorschlag) return <div aria-busy className="min-h-40" />;
  if (!Array.isArray(vorschlag.dateien))
    return (
      <pre className="m-0 overflow-auto rounded-control bg-fill p-3 font-mono text-caption">
        {JSON.stringify(vorschlag, null, 2)}
      </pre>
    );
  const titel = e.titel;
  return (
    <section aria-label={titel} aria-busy={laedt} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-baseline gap-x-3 gap-y-1">
        <h3 className="m-0 text-callout font-semibold text-text">{titel}</h3>
        <span className="text-caption text-text-2">{laedt ? e.rechnet : e.nichtsAngewendet}</span>
      </div>
      {vorschlag.dateien.length === 0 ? (
        <EmptyState title={e.leer}>{e.leerText}</EmptyState>
      ) : (
        <>
          <div
            role="status"
            data-vollstaendig={vorschlag.vollstaendig}
            className={`squircle flex items-center gap-2.5 rounded-panel border px-3.5 py-2.5 ${
              vorschlag.vollstaendig ? "border-done/30 bg-done-fill" : "border-separator bg-fill"
            }`}
          >
            <Icon
              name={vorschlag.vollstaendig ? "check" : "warn"}
              size={16}
              className={vorschlag.vollstaendig ? "text-done" : "text-soon"}
            />
            <p className="m-0 text-body text-text">
              <span className="font-semibold">
                {vorschlag.vollstaendig ? e.vollstaendig : e.unvollstaendig}
              </span>
              {vorschlag.vollstaendig ? ` — ${e.vollstaendigText}` : ""}
            </p>
          </div>
          {vorschlag.abgelehnt.length > 0 && (
            <div className="flex flex-col gap-2">
              <h4 className="m-0 text-caption font-semibold uppercase text-text-2">
                {e.abgelehnt}
              </h4>
              <ul aria-label={e.abgelehnt} className="m-0 flex list-none flex-col gap-2 p-0">
                {vorschlag.abgelehnt.map((x) => (
                  <li
                    key={`${x.pfad}:${x.text}`}
                    className="flex gap-2.5 rounded-control border border-critical/25 bg-critical-fill px-3 py-2.5"
                  >
                    <Icon name="blockiert" size={16} className="mt-0.5 shrink-0 text-critical" />
                    <div className="flex min-w-0 flex-1 flex-col gap-1.5">
                      <p className="m-0 text-body break-words text-text">{x.text}</p>
                      <div className="flex flex-wrap items-center gap-1.5">
                        <code
                          className="rounded-[10px] border border-separator px-2 py-px font-mono text-caption text-text-2"
                          title={c.pfad}
                        >
                          {x.pfad}
                        </code>
                        {x.quelle && <SourceChip quelle={{ dokument: x.quelle }} />}
                      </div>
                    </div>
                  </li>
                ))}
              </ul>
            </div>
          )}
          <Tabs className="flex flex-col gap-2">
            <TabList
              aria-label={e.dateien}
              className="flex flex-wrap gap-1 border-b border-separator"
            >
              {vorschlag.dateien.map((d) => (
                <Tab
                  key={d.datei}
                  id={d.datei}
                  className="focus-ring -mb-px flex cursor-default flex-col items-start border-b-2 border-transparent px-3 py-1.5 font-mono text-body text-text-2 outline-none data-[hovered]:text-text data-[selected]:border-accent data-[selected]:text-text"
                >
                  {d.datei}
                  <span className="font-sans text-caption leading-tight text-text-2">
                    {e.summe(
                      d.aenderungen,
                      d.luecken,
                      formatCount(d.aenderungen),
                      formatCount(d.luecken),
                    )}
                  </span>
                </Tab>
              ))}
            </TabList>
            {vorschlag.dateien.map((d) => (
              <TabPanel key={d.datei} id={d.datei} className="outline-none">
                <DateiAnsicht datei={d} />
              </TabPanel>
            ))}
          </Tabs>
        </>
      )}
    </section>
  );
}

const ZEICHEN: Record<string, string> = {
  neu: "+",
  weg: "−",
  gleich: "",
  fehlt: "?",
  abschnitt: "",
};
const ZEILE: Record<string, string> = {
  neu: "bg-done-fill text-text",
  weg: "bg-critical-fill text-text-2",
  gleich: "text-text-2",
  fehlt: "bg-soon-fill text-text",
  abschnitt: "text-text font-semibold",
};

/** One proposed file: its lines as makod marked them, with copy and download. */
export function DateiAnsicht({ datei }: { datei: Datei }) {
  const t = useT();
  const e = t.einrichtung.befragung.ergebnis;
  const [kopiert, setKopiert] = useState(false);
  let vorige: string | undefined;
  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        <Button
          variant="secondary"
          aria-label={e.kopieren(datei.datei)}
          onPress={() => {
            navigator.clipboard?.writeText(datei.toml).then(() => {
              setKopiert(true);
              setTimeout(() => setKopiert(false), 1500);
            });
          }}
        >
          <Icon name={kopiert ? "check" : "copy"} size={13} />
          {kopiert ? e.kopiert : e.kopieren(datei.datei)}
        </Button>
        <Button
          variant="ghost"
          aria-label={e.herunterladen(datei.datei)}
          onPress={() => herunterladen(datei.datei, datei.toml)}
        >
          <Icon name="download" size={13} />
          {e.herunterladen(datei.datei)}
        </Button>
      </div>
      {!datei.abgeglichen && (
        <p className="m-0 text-caption text-text-2">{e.nichtAbgeglichen(datei.dienst)}</p>
      )}
      <ol
        aria-label={e.zeilen(datei.datei)}
        className="squircle m-0 flex list-none flex-col overflow-x-auto rounded-control border border-separator bg-surface p-0 py-1.5 font-mono text-caption"
      >
        {datei.zeilen.map((z, i) => {
          const quelle = z.quelle && z.quelle !== vorige ? z.quelle : undefined;
          if (z.art !== "abschnitt") vorige = z.quelle ?? undefined;
          else vorige = undefined;
          return (
            <li
              // A line has no id; makod's order names it.
              // biome-ignore lint/suspicious/noArrayIndexKey: makod's order
              key={i}
              data-art={z.art}
              className={`flex flex-col gap-0.5 px-2.5 py-0.5 ${ZEILE[z.art] ?? ""} ${z.art === "abschnitt" && i > 0 ? "mt-2" : ""}`}
            >
              {quelle && z.art !== "fehlt" && (
                <span className="font-sans text-text-2"># {quelle}</span>
              )}
              <span className="flex min-w-0 items-start gap-2">
                <span aria-hidden className="w-3 shrink-0 select-none text-center text-text-2">
                  {ZEICHEN[z.art] ?? ""}
                </span>
                <span className="sr-only">{`${eintrag(e.art, z.art)}: `}</span>
                <span
                  className={`min-w-0 flex-1 whitespace-pre-wrap break-words ${z.art === "weg" ? "line-through" : ""}`}
                >
                  {z.text}
                </span>
                {z.art === "fehlt" && z.frage && (
                  <AriaButton
                    onPress={() => zurFrage(z.frage ?? "")}
                    className="focus-ring shrink-0 rounded-[6px] px-1.5 font-sans text-caption font-medium text-accent data-[hovered]:bg-fill"
                  >
                    {e.zurFrage}
                  </AriaButton>
                )}
              </span>
            </li>
          );
        })}
      </ol>
    </div>
  );
}

/** Hand the reader `text` as the file `name`. */
function herunterladen(name: string, text: string) {
  const url = URL.createObjectURL(new Blob([text], { type: "application/toml" }));
  const a = document.createElement("a");
  a.href = url;
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(url), 0);
}
